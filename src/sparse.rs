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

/// Absolute git dir where `git submodule` expects this submodule's repository:
/// `<root>/.git/modules/<path>`.
pub fn gitdir(root: &Path, sm: &Submodule) -> Result<PathBuf> {
    let rel = git::capture(
        root,
        &["rev-parse", "--git-path", &format!("modules/{}", sm.path)],
    )?;
    Ok(root.join(rel))
}

/// Build the submodule git dir + worktree link (only if absent) and apply the
/// partial-clone, promisor and sparse-checkout configuration. Returns the git
/// dir. Idempotent.
pub fn prepare(root: &Path, sm: &Submodule, con: &Console) -> Result<PathBuf> {
    let gitdir = gitdir(root, sm)?;
    let worktree = root.join(&sm.path);

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
    if git::ok(wt, &["remote", "get-url", "origin"])? {
        git::run(wt, &["remote", "set-url", "origin", &sm.url])?;
    } else {
        git::run(wt, &["remote", "add", "origin", &sm.url])?;
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

/// `du -sh`-style working-tree size, or `None` if unavailable.
pub fn worktree_size(root: &Path, sm: &Submodule) -> Option<String> {
    let path = root.join(&sm.path);
    let out = std::process::Command::new("du")
        .arg("-sh")
        .arg(&path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(str::to_string)
}
