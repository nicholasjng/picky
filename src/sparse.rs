//! Submodule git-dir construction plus partial-clone, promisor and
//! sparse-checkout configuration, the reusable core of `init-duckdb.sh`. Kept
//! idempotent so any prior state converges to the same checkout.

use anyhow::{Context, Result, anyhow};
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::Submodule;
use crate::console::Console;
use crate::git;

/// The gitlink revision the superproject pins `path` to.
pub fn pinned_sha(root: &Path, path: &str) -> Result<String> {
    let out = git::capture(root, &["ls-files", "-s", path])?;
    // `<mode> <sha> <stage>\t<path>`; `split_whitespace` also splits the tab.
    out.split_whitespace()
        .nth(1)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("no gitlink for '{path}' in the index"))
}

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

    let sparse_on = !sm.sparse.is_empty();
    git::run(
        wt,
        &[
            "config",
            "core.sparseCheckout",
            if sparse_on { "true" } else { "false" },
        ],
    )?;
    git::run(wt, &["config", "core.sparseCheckoutCone", "false"])?;
    if sparse_on {
        let info = gitdir.join("info");
        fs::create_dir_all(&info)?;
        let mut body = sm.sparse.join("\n");
        body.push('\n');
        fs::write(info.join("sparse-checkout"), body)
            .context("writing sparse-checkout patterns")?;
    }
    Ok(gitdir)
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
    fetch(&wt, sm, committish, con)
}

/// `git fetch` honoring the submodule's filter and depth. If `PICKY_AUTO_GC`
/// is set, follows up with a best-effort `git gc --auto` (threshold-gated, so
/// a no-op on most runs) — a power-user knob for repos updated often enough
/// that shallow-graft churn piles up between runs (e.g. an LLVM-sized
/// submodule bumped in a loop). Off by default so a plain `fetch` stays
/// side-effect-free; see `picky gc` (`src/commands/gc.rs`) for a deterministic
/// `--prune=now` run instead.
fn fetch(wt: &Path, sm: &Submodule, refspec: &str, con: &Console) -> Result<()> {
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
    git::run(wt, &args)?;
    if std::env::var_os("PICKY_AUTO_GC").is_some() {
        if let Err(e) = git::run(wt, &["gc", "--auto", "--quiet"]) {
            con.warn(format!("auto-gc failed: {e:#}"));
        }
    }
    Ok(())
}

/// Fetch a ref into the submodule and report the resolved commit (used by
/// `picky add`, where there is no gitlink yet to read a pin from).
pub fn fetch_ref(root: &Path, sm: &Submodule, refspec: &str, con: &Console) -> Result<()> {
    let wt = root.join(&sm.path);
    con.step(format!("Fetching {refspec} (shallow, blobless)"));
    fetch(&wt, sm, refspec, con)
}

/// `checkout -f --detach` then trim any out-of-sparse paths a pre-existing full
/// checkout left behind.
pub fn checkout(root: &Path, sm: &Submodule, refish: &str, con: &Console) -> Result<()> {
    let wt = root.join(&sm.path);
    con.step(format!("Checking out {refish}"));
    // `-f` repopulates the tree even when the index already matches but files
    // are missing (a wiped or half-built tree).
    git::run(&wt, &["checkout", "-q", "-f", "--detach", refish])?;
    if !sm.sparse.is_empty() {
        git::run(&wt, &["sparse-checkout", "reapply"])?;
    }
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
