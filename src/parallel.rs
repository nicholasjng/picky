//! A minimal bounded parallel `map` over a slice, so commands whose per-item
//! cost is spent waiting — on `git`, on a disk walk, on a remote — can overlap
//! that wait without a runtime or a thread-pool dependency.

use std::num::NonZeroUsize;

/// Apply `f` to every item, running up to [`available_parallelism`] calls at a
/// time, and return the results in input order.
///
/// Chunked rather than one thread per item: `status` walks two directory trees
/// per submodule and `refresh` runs one `ls-remote` per remote, so a repo with
/// dozens of submodules would otherwise start dozens of those at once.
///
/// [`available_parallelism`]: std::thread::available_parallelism
pub fn map<T, R>(items: &[T], f: impl Fn(&T) -> R + Send + Sync) -> Vec<R>
where
    T: Sync,
    R: Send,
{
    let width = std::thread::available_parallelism().map_or(4, NonZeroUsize::get);
    let mut out = Vec::with_capacity(items.len());
    for chunk in items.chunks(width) {
        std::thread::scope(|scope| {
            let handles: Vec<_> = chunk.iter().map(|item| scope.spawn(|| f(item))).collect();
            for handle in handles {
                // Re-raise a worker panic here rather than dropping the item
                // and returning a short result set.
                out.push(
                    handle
                        .join()
                        .unwrap_or_else(|e| std::panic::resume_unwind(e)),
                );
            }
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::map;

    #[test]
    fn results_keep_input_order_across_chunks() {
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
