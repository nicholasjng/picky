//! Submodule git-dir construction plus partial-clone, promisor and
//! sparse-checkout configuration, the reusable core of `init-duckdb.sh`. Kept
//! idempotent so any prior state converges to the same checkout.

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Submodule;
use crate::console::Console;
use crate::git;

/// The gitlink revision the superproject pins `path` to.
pub fn pinned_sha(root: &Path, path: &str) -> Result<String> {
    let out = git::capture(root, &["ls-files", "-s", "--", path])?;
    // `<mode> <sha> <stage>\t<path>`; `split_whitespace` also splits the tab.
    //
    // Match the gitlink mode explicitly rather than trusting the first row: for
    // a path that is a plain tracked directory instead of a submodule,
    // `ls-files` lists every file *under* it, and taking the first row hands
    // back a blob SHA that then fails far away as "git upload-pack: not our
    // ref <blob>".
    let mut tracked = false;
    for line in out.lines() {
        tracked = true;
        let mut parts = line.split_whitespace();
        let (Some(mode), Some(sha)) = (parts.next(), parts.next()) else {
            continue;
        };
        if mode == GITLINK_MODE {
            return Ok(sha.to_string());
        }
    }
    if tracked {
        bail!(
            "'{path}' is tracked but is not a submodule gitlink; the superproject has no commit pinned there"
        );
    }
    bail!("no gitlink for '{path}' in the index")
}

/// The index mode `git` records a submodule gitlink under.
const GITLINK_MODE: &str = "160000";

/// Absolute `.git/modules` directory, the parent of every submodule git dir.
///
/// Asked of git rather than assumed: the literal `.git/modules` is wrong under
/// a linked worktree or an explicit `GIT_DIR`. Separate from [`gitdir`] so a
/// caller measuring every submodule resolves it once.
pub fn modules_dir(root: &Path) -> Result<PathBuf> {
    let rel = git::capture(root, &["rev-parse", "--git-path", "modules"])?;
    Ok(root.join(rel))
}

/// Absolute git dir where `git submodule` expects this submodule's repository:
/// `<root>/.git/modules/<path>`.
pub fn gitdir(root: &Path, sm: &Submodule) -> Result<PathBuf> {
    Ok(modules_dir(root)?.join(&sm.path))
}

/// Whether a `.gitmodules` URL is one git resolves against the superproject's
/// own remote. Git treats exactly a leading `./` or `../` this way; anything
/// else (a scheme, an scp-like host, an absolute path) is used verbatim.
fn is_relative_url(url: &str) -> bool {
    url.starts_with("./") || url.starts_with("../")
}

/// The URL to configure as the submodule's `origin`.
///
/// A relative `.gitmodules` URL (`url = ../sibling.git`) is resolved by git
/// against the *superproject's* remote. picky configures `origin` from inside
/// the submodule worktree, where the same string would resolve against
/// `<root>/<path>/` instead, so it must be resolved first.
///
/// The resolution comes from git rather than from reimplementing its rules:
/// `git submodule init` writes the resolved URL to the superproject's local
/// config as `submodule.<name>.url`, and [`prepare`] runs it beforehand.
///
/// Read-only, so `status`/`refresh` can key the ref cache off the same URL a
/// fetch uses. Falls back to the declared value when git has no resolved one
/// (no gitlink in the index yet), leaving git to report the failure.
pub fn origin_url(root: &Path, sm: &Submodule) -> Result<String> {
    if !is_relative_url(&sm.url) {
        return Ok(sm.url.clone());
    }
    let key = format!("submodule.{}.url", sm.name);
    match git::capture_unless_unset(root, &["config", "--get", &key])? {
        Some(url) if !is_relative_url(&url) && !url.is_empty() => Ok(url),
        _ => Ok(sm.url.clone()),
    }
}

/// Build the submodule git dir + worktree link (only if absent) and apply the
/// partial-clone, promisor and sparse-checkout configuration. Returns the git
/// dir. Idempotent.
pub fn prepare(root: &Path, sm: &Submodule, con: &Console) -> Result<PathBuf> {
    let gitdir = gitdir(root, sm)?;
    let worktree = root.join(&sm.path);

    // Register the submodule so `git status` tracks it, and so git resolves a
    // relative `url` into local config for [`origin_url`]. Tolerate failure:
    // there is no gitlink to register during `add`, and picky configures the
    // remote itself either way.
    let _ = git::ok(root, &["submodule", "init", "--", &sm.path]);
    if is_relative_url(&sm.url) {
        // `init` leaves an already-set local url alone, so an edited relative
        // url needs `sync` to re-resolve it from `.gitmodules`.
        let _ = git::ok(root, &["submodule", "sync", "--", &sm.path]);
    }

    // Create the git dir when the worktree has no usable repo. A plain `.git`
    // existence check is fooled by a *dangling* gitlink (git dir deleted, file
    // left behind), so validate the link with `rev-parse --resolve-git-dir`
    // (which doesn't walk up to the superproject) and rebuild if it's stale.
    let dotgit = worktree.join(".git");
    let dotgit_s = dotgit.to_str().context("`.git` path is not UTF-8")?;
    let valid = dotgit.exists() && git::ok(root, &["rev-parse", "--resolve-git-dir", dotgit_s])?;
    if !valid {
        if dotgit.exists() {
            con.step("Rebuilding git dir (stale gitlink)");
            if dotgit.is_dir() {
                fs::remove_dir_all(&dotgit)?;
            } else {
                fs::remove_file(&dotgit)?;
            }
        } else {
            con.step("Creating git dir");
        }
        if let Some(parent) = gitdir.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::create_dir_all(&worktree)?;
        let gitdir_s = gitdir.to_str().context("git dir path is not UTF-8")?;
        let worktree_s = worktree.to_str().context("worktree path is not UTF-8")?;
        git::run(
            root,
            &["init", "-q", "--separate-git-dir", gitdir_s, worktree_s],
        )?;
    }

    con.step("Configuring remote and sparse checkout");
    let wt = worktree.as_path();
    let url = origin_url(root, sm)?;
    if git::ok(wt, &["remote", "get-url", "origin"])? {
        git::run(wt, &["remote", "set-url", "--", "origin", &url])?;
    } else {
        git::run(wt, &["remote", "add", "--", "origin", &url])?;
    }
    git::run(wt, &["config", "extensions.partialClone", "origin"])?;
    git::run(wt, &["config", "remote.origin.promisor", "true"])?;
    if let Some(filter) = sm.effective_filter() {
        git::run(wt, &["config", "remote.origin.partialclonefilter", filter])?;
    }

    set_sparse_state(wt, sm)?;
    Ok(gitdir)
}

/// Drive the submodule's sparse state through `git sparse-checkout`.
///
/// Deliberately not a hand-written `core.sparseCheckout` + `info/sparse-checkout`:
///
/// - The porcelain sets `extensions.worktreeConfig` and keeps
///   `core.sparseCheckout` in `config.worktree`, which outranks the plain
///   config a manual `git config` write touches. Mixing the two leaves the key
///   set in both files with the worktree copy winning, and every later
///   `sparse-checkout reapply` dies with "must be in a sparse-checkout".
/// - Dropping every pattern needs `disable`, which clears the index's
///   `skip-worktree` bits and re-materializes the excluded files. Setting
///   `core.sparseCheckout=false` alone leaves those bits set, so the files
///   stay missing from a checkout that now claims to be full.
fn set_sparse_state(wt: &Path, sm: &Submodule) -> Result<()> {
    if sm.sparse.is_empty() {
        return git::run(wt, &["sparse-checkout", "disable"]).context("disabling sparse checkout");
    }
    // `--skip-checks` keeps git from second-guessing non-cone patterns (it
    // warns about leading `/` and `*`, which is exactly what these are).
    let mut args = vec!["sparse-checkout", "set", "--no-cone", "--skip-checks", "--"];
    args.extend(sm.sparse.iter().map(String::as_str));
    git::run(wt, &args).context("applying sparse-checkout patterns")
}

/// Fetch `committish` shallow + blobless (per the submodule's filter/depth) if
/// it is not already present locally.
pub fn ensure_commit(root: &Path, sm: &Submodule, committish: &str, con: &Console) -> Result<()> {
    let wt = root.join(&sm.path);
    // GIT_NO_LAZY_FETCH so the promisor doesn't full-fetch the object we are
    // only probing for; if it's missing we fetch it shallow+blobless ourselves.
    if git::ok_env(
        &wt,
        &["cat-file", "-e", &format!("{committish}^{{commit}}")],
        &[("GIT_NO_LAZY_FETCH", "1")],
    )? {
        return Ok(());
    }
    con.step(format!("Fetching {committish} (shallow, blobless)"));
    fetch(&wt, sm, committish)
}

/// `git fetch` honoring the submodule's filter and depth.
fn fetch(wt: &Path, sm: &Submodule, refspec: &str) -> Result<()> {
    let filter = sm.effective_filter().map(|f| format!("--filter={f}"));
    let depth = sm.effective_depth().map(|d| format!("--depth={d}"));
    let mut args = vec!["fetch", "-q"];
    if let Some(f) = &filter {
        args.push(f);
    }
    if let Some(d) = &depth {
        args.push(d);
    }
    args.push("origin");
    args.push(refspec);
    git::run(wt, &args)
}

/// Fetch a ref into the submodule and report the resolved commit (used by
/// `picky add`, where there is no gitlink yet to read a pin from).
pub fn fetch_ref(root: &Path, sm: &Submodule, refspec: &str, con: &Console) -> Result<()> {
    let wt = root.join(&sm.path);
    con.step(format!("Fetching {refspec} (shallow, blobless)"));
    fetch(&wt, sm, refspec)
}

/// `checkout -f --detach` then trim any out-of-sparse paths a pre-existing full
/// checkout left behind.
pub fn checkout(root: &Path, sm: &Submodule, refish: &str, con: &Console) -> Result<()> {
    let wt = root.join(&sm.path);
    con.step(format!("Checking out {refish}"));
    // `-f` repopulates the tree even when the index already matches but files
    // are missing (a wiped or half-built tree).
    git::run(&wt, &["checkout", "-q", "-f", "--detach", refish])?;
    // Reconcile the sparse state rather than a bare `sparse-checkout reapply`:
    // `update` never runs `prepare`, so this is the only place a checkout that
    // drifted (patterns edited in `.gitmodules`, or `git sparse-checkout`
    // toggled by hand inside the submodule) converges back onto the declared
    // patterns. `reapply` alone dies with "must be in a sparse-checkout" the
    // moment sparsity was disabled underneath us.
    set_sparse_state(&wt, sm)?;
    Ok(())
}

/// Recursive disk usage of `path` in **bytes**, or `None` when it can't be
/// read at all.
///
/// Hand-rolled rather than shelling out to `du -sk`: no external tool, byte
/// resolution instead of 1 KiB blocks, and one less process spawn per
/// `status` row.
///
/// Counts *allocated* blocks where the platform reports them (`st_blocks`),
/// matching `du`; apparent length elsewhere. Symlinks are counted but never
/// followed, so a link out of the tree neither inflates the total nor escapes
/// it. Unlike `du`, a hardlinked file counts once per link.
pub fn dir_size(path: &Path) -> Option<u64> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_dir() {
        return Some(entry_size(&meta));
    }
    let mut total = entry_size(&meta);
    // An explicit stack, so a deep tree costs heap rather than stack.
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        // An unreadable subdirectory contributes nothing rather than failing
        // the whole measurement.
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            // `DirEntry::metadata` does not traverse symlinks: this is the
            // link's own size, and `is_dir` is false for a link to a directory.
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            total += entry_size(&meta);
            if meta.is_dir() {
                stack.push(entry.path());
            }
        }
    }
    Some(total)
}

/// One entry's contribution to [`dir_size`]: allocated blocks on unix,
/// apparent length elsewhere.
#[cfg(unix)]
fn entry_size(meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    // `st_blocks` is in 512-byte units, whatever the filesystem's block size.
    meta.blocks() * 512
}

#[cfg(not(unix))]
fn entry_size(meta: &std::fs::Metadata) -> u64 {
    meta.len()
}

/// Working-tree disk usage of a submodule, in bytes.
pub fn worktree_size(root: &Path, sm: &Submodule) -> Option<u64> {
    dir_size(&root.join(&sm.path))
}

/// Disk usage of the submodule's git dir — the partial clone's object store —
/// in bytes.
///
/// Reported separately from [`worktree_size`], which cannot see it: the
/// worktree's `.git` is a link file, not the repository. It is usually the
/// larger half (a sparse duckdb checkout measures 77M of working tree over a
/// 316M object store) and the half that grows without bound, since `git gc`
/// can never reclaim a partial clone's old packs — the growth
/// `picky update --fresh` resets.
///
/// Takes the `.git/modules` directory rather than resolving it per call, so a
/// caller measuring every submodule resolves it once ([`modules_dir`]).
pub fn store_size(modules: &Path, sm: &Submodule) -> Option<u64> {
    let gitdir = modules.join(&sm.path);
    if !gitdir.is_dir() {
        return None;
    }
    dir_size(&gitdir)
}

/// How the checkout's actual sparse state differs from what `sm` declares, or
/// `None` when they agree (or the submodule isn't checked out).
///
/// Drift comes from a `git sparse-checkout` run by hand inside the submodule,
/// or a `.gitmodules` edit that was never reconciled. [`checkout`] fixes it
/// silently on the next `init`/`update`; `doctor` reports it.
///
/// Order is compared, not just membership: these are non-cone gitignore-style
/// patterns, where a later line overrides an earlier one, so a reordering can
/// select a different tree.
pub fn sparse_drift(root: &Path, sm: &Submodule) -> Option<String> {
    let wt = root.join(&sm.path);
    if !wt.join(".git").exists() {
        return None;
    }
    // Unset (`--get` exits 1) is how a non-sparse checkout reads.
    let enabled = git::capture_unless_unset(&wt, &["config", "--get", "core.sparseCheckout"])
        .ok()?
        .is_some_and(|v| v == "true");

    if sm.sparse.is_empty() {
        return enabled.then(|| {
            "declares no sparse patterns (a full checkout) but the checkout is sparse".to_string()
        });
    }
    if !enabled {
        return Some(format!(
            "declares {} sparse pattern(s) but the checkout is not sparse",
            sm.sparse.len()
        ));
    }

    // Only valid once sparsity is known to be enabled: on a full checkout
    // `sparse-checkout list` fails with "this worktree is not sparse".
    let listed = git::capture(&wt, &["sparse-checkout", "list"]).ok()?;
    let actual: Vec<&str> = listed.lines().collect();
    if actual == sm.sparse.iter().map(String::as_str).collect::<Vec<_>>() {
        return None;
    }

    let missing: Vec<&str> = sm
        .sparse
        .iter()
        .map(String::as_str)
        .filter(|p| !actual.contains(p))
        .collect();
    let extra: Vec<&str> = actual
        .iter()
        .copied()
        .filter(|p| !sm.sparse.iter().any(|d| d == p))
        .collect();
    let mut parts = Vec::new();
    if !missing.is_empty() {
        parts.push(format!(
            "declared but not in the checkout: {}",
            missing.join(", ")
        ));
    }
    if !extra.is_empty() {
        parts.push(format!(
            "in the checkout but not declared: {}",
            extra.join(", ")
        ));
    }
    if parts.is_empty() {
        parts.push("the same patterns in a different order".to_string());
    }
    Some(format!(
        "sparse patterns drifted from .gitmodules ({})",
        parts.join("; ")
    ))
}

/// Render a byte count the way `du -h` does: `812K`, `77M`, `1.2G`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    match unit {
        0 => format!("{bytes}B"),
        // One decimal below 10 (`1.2G`), none above (`316M`), like `du -h`.
        _ if value < 10.0 => format!("{value:.1}{}", UNITS[unit]),
        _ => format!("{value:.0}{}", UNITS[unit]),
    }
}

#[cfg(test)]
mod tests {
    use super::{dir_size, human_size, is_relative_url};

    /// A throwaway directory, removed on drop.
    struct TmpDir(std::path::PathBuf);
    impl TmpDir {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("picky-sparse-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TmpDir(dir)
        }
    }
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn dir_size_walks_subdirectories() {
        let tmp = TmpDir::new("walk");
        std::fs::write(tmp.0.join("a.txt"), vec![b'x'; 4096]).unwrap();
        std::fs::create_dir(tmp.0.join("sub")).unwrap();
        std::fs::write(tmp.0.join("sub/b.txt"), vec![b'x'; 4096]).unwrap();

        let size = dir_size(&tmp.0).unwrap();
        assert!(
            size >= 8192,
            "should count both files across the nesting, got {size}"
        );
        // A missing path has no size at all, rather than zero.
        assert_eq!(dir_size(&tmp.0.join("nope")), None);
    }

    #[cfg(unix)]
    #[test]
    fn dir_size_does_not_follow_symlinks() {
        let tmp = TmpDir::new("links");
        let real = tmp.0.join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("big.bin"), vec![b'x'; 256 * 1024]).unwrap();
        let measured = dir_size(&real).unwrap();

        // A tree whose only content is a link *to* `real` must not inherit
        // `real`'s size — otherwise a link out of a submodule would inflate
        // its reported footprint.
        let linked = tmp.0.join("linked");
        std::fs::create_dir(&linked).unwrap();
        std::os::unix::fs::symlink(&real, linked.join("ptr")).unwrap();
        let via_link = dir_size(&linked).unwrap();
        assert!(
            via_link < measured,
            "symlink was followed: {via_link} vs {measured}"
        );
    }

    #[test]
    fn human_size_matches_du_h_style() {
        assert_eq!(human_size(0), "0B");
        assert_eq!(human_size(999), "999B");
        assert_eq!(human_size(1024), "1.0K");
        assert_eq!(human_size(77 * 1024 * 1024), "77M");
        assert_eq!(human_size(316 * 1024 * 1024), "316M");
        assert_eq!(human_size(1024 * 1024 * 1024 + 1024 * 1024 * 200), "1.2G");
    }

    #[test]
    fn only_dot_prefixed_urls_are_relative() {
        for rel in ["./sibling.git", "../sibling.git", "../../up/x.git"] {
            assert!(is_relative_url(rel), "{rel} should be relative");
        }
        for abs in [
            "https://example.com/x.git",
            "git@example.com:o/x.git",
            "/srv/git/x.git",
            "file:///srv/git/x.git",
            ".hidden/x.git",
        ] {
            assert!(!is_relative_url(abs), "{abs} should not be relative");
        }
    }
}
