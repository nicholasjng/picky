//! Cache of a remote's ref names for fast `<ref>` completion. `git ls-remote`
//! is too slow for a TAB press, so results are cached per-remote with a TTL: a
//! stale cache is still served (and a background refresh kicked); with no cache,
//! one timeout-bounded `ls-remote` runs, decaying to local refs on failure.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

/// How long a cached ref list is considered fresh.
pub const TTL: Duration = Duration::from_secs(3600);

/// A cached ref list and whether it is still within [`TTL`].
pub struct Cached {
    pub refs: Vec<String>,
    pub fresh: bool,
}

/// `${XDG_CACHE_HOME:-~/.cache}/picky/refs`.
fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("picky").join("refs"))
}

fn cache_file(url: &str) -> Option<PathBuf> {
    Some(cache_dir()?.join(fnv1a(url)))
}

/// FNV-1a 64-bit of the URL as a stable filename — avoids a hashing crate.
fn fnv1a(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Read the cached ref list for `url`, if any.
pub fn read(url: &str) -> Option<Cached> {
    let file = cache_file(url)?;
    let meta = std::fs::metadata(&file).ok()?;
    let fresh = meta
        .modified()
        .ok()
        .and_then(|m| SystemTime::now().duration_since(m).ok())
        .is_some_and(|age| age < TTL);
    let body = std::fs::read_to_string(&file).ok()?;
    let refs = body
        .lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    Some(Cached { refs, fresh })
}

/// Atomically write the ref list for `url`.
pub fn write(url: &str, refs: &[String]) -> std::io::Result<()> {
    let Some(file) = cache_file(url) else {
        return Ok(());
    };
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Per-pid temp name so concurrent refreshes don't clobber each other.
    let tmp = file.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, refs.join("\n"))?;
    std::fs::rename(&tmp, &file)
}

/// `git ls-remote` the remote's heads + tags, optionally bounded by a timeout
/// (the child is killed if it overruns). Returns bare ref names (no
/// `refs/heads/` or `refs/tags/` prefix), or `None` on any failure.
pub fn ls_remote(url: &str, timeout: Option<Duration>) -> Option<Vec<String>> {
    let mut child = Command::new("git")
        .args(["ls-remote", "--heads", "--tags", "--refs", url])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0") // never block on a credential prompt
        .spawn()
        .ok()?;

    // Drain stdout on a thread so a large ref list can't deadlock the pipe
    // while we poll for the timeout.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });

    let status = match timeout {
        None => child.wait().ok()?,
        Some(dur) => {
            let start = std::time::Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => break status,
                    Ok(None) if start.elapsed() > dur => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return None;
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(40)),
                    Err(_) => return None,
                }
            }
        }
    };

    let out = reader.join().ok()?;
    status.success().then(|| parse(&out))
}

fn parse(out: &str) -> Vec<String> {
    out.lines()
        .filter_map(|line| strip(line.split('\t').nth(1)?))
        .collect()
}

fn strip(refname: &str) -> Option<String> {
    refname
        .strip_prefix("refs/tags/")
        .or_else(|| refname.strip_prefix("refs/heads/"))
        .map(str::to_string)
}

/// Offline fallback: ref names already present in the submodule's own git dir
/// (tags + remote-tracking branches `picky update` fetches). Empty if the
/// submodule isn't checked out.
pub fn local_refs(root: &Path, path: &str) -> Vec<String> {
    let wt = root.join(path);
    let Ok(out) = crate::git::capture(
        &wt,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/tags",
            "refs/remotes/origin",
        ],
    ) else {
        return Vec::new();
    };
    out.lines()
        .filter_map(|r| {
            if let Some(t) = r.strip_prefix("refs/tags/") {
                Some(t.to_string())
            } else {
                r.strip_prefix("refs/remotes/origin/")
                    .filter(|b| *b != "HEAD")
                    .map(str::to_string)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_strips_prefixes_and_skips_other_refs() {
        let out = "deadbeef\trefs/heads/main\n\
                   cafef00d\trefs/tags/v1.6.3\n\
                   0badf00d\trefs/pull/7/head\n";
        assert_eq!(parse(out), vec!["main", "v1.6.3"]);
    }

    #[test]
    fn fnv_is_stable_and_hex() {
        assert_eq!(fnv1a("https://example.com/x.git").len(), 16);
        assert_eq!(fnv1a("a"), fnv1a("a"));
        assert_ne!(fnv1a("a"), fnv1a("b"));
    }
}
