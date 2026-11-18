//! A minimal bounded parallel `map` over a slice, so commands whose per-item
//! cost is spent waiting — on `git`, on a disk walk, on a remote — can overlap
//! that wait without a runtime or a thread-pool dependency.

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Apply `f` to every item, running up to [`available_parallelism`] calls at a
/// time, and return the results in input order.
///
/// Bounded rather than one thread per item: `status` walks two directory
/// trees per submodule and `refresh` runs one `ls-remote` per remote, so a
/// repo with dozens of submodules would otherwise start dozens at once.
///
/// A fixed set of workers pulls the next index from a shared counter, rather
/// than running fixed-size chunks one after another: per-item cost is very
/// uneven (an LLVM checkout's directory walk takes seconds, a small
/// submodule's milliseconds), and a chunk would idle every other slot until
/// its slowest item finished.
///
/// [`available_parallelism`]: std::thread::available_parallelism
pub fn map<T, R>(items: &[T], f: impl Fn(&T) -> R + Send + Sync) -> Vec<R>
where
    T: Sync,
    R: Send,
{
    let width = std::thread::available_parallelism()
        .map_or(4, NonZeroUsize::get)
        .min(items.len());
    let next = AtomicUsize::new(0);
    let mut out: Vec<Option<R>> = items.iter().map(|_| None).collect();
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..width)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        done.push((i, f(item)));
                    }
                    done
                })
            })
            .collect();
        for worker in workers {
            // Re-raise a worker panic here rather than dropping its items and
            // returning a short result set.
            let done = worker
                .join()
                .unwrap_or_else(|e| std::panic::resume_unwind(e));
            for (i, r) in done {
                out[i] = Some(r);
            }
        }
    });
    out.into_iter()
        .map(|r| r.expect("every index is claimed by exactly one worker"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::map;

    #[test]
    fn results_keep_input_order_across_workers() {
        let items: Vec<usize> = (0..100).collect();
        assert_eq!(
            map(&items, |n| n * 2),
            (0..100).map(|n| n * 2).collect::<Vec<_>>()
        );
    }

    #[test]
    fn an_empty_slice_maps_to_an_empty_vec() {
        let items: Vec<usize> = Vec::new();
        assert!(map(&items, |n| *n).is_empty());
    }
}
