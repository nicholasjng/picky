//! `picky remove <path>…`: the inverse of `picky add`. Undeclares a submodule
//! and deletes its checkout (working tree, submodule git dir, gitlink, and
//! `.gitmodules` entry). No implicit "remove all"; paths must be explicit.

use anyhow::{Result, bail};
use std::fs;
use std::io::IsTerminal;
use std::path::Path;

use crate::config::{self, Submodule};
use crate::console::{self, Console};
use crate::{git, sparse};

pub fn run(root: &Path, paths: &[String], yes: bool, con: &Console) -> Result<()> {
    if paths.is_empty() {
        bail!("no submodule specified, pass one or more paths (there is no \"remove all\")");
    }
    let targets: Vec<Submodule> = paths
        .iter()
        .map(|p| config::find(root, p))
        .collect::<Result<_>>()?;

    if !yes {
        con.warn(format!(
            "about to delete the working tree and git dir for: {}",
            targets
                .iter()
                .map(|sm| sm.path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        if !std::io::stdin().is_terminal() {
            bail!("refusing to remove non-interactively without --yes");
        }
        if !console::confirm("  remove? [y/N] ")? {
            bail!("aborted, pass --yes to skip confirmation");
        }
    }

    for sm in &targets {
        con.heading(format!("removing submodule {}", sm.path));

        let worktree = root.join(&sm.path);
        if worktree.join(".git").exists()
            && let Ok(status) = git::capture(&worktree, &["status", "--porcelain"])
            && !status.is_empty()
        {
            con.warn(format!(
                "{} has uncommitted changes that will be discarded",
                sm.path
            ));
        }

        // Best-effort: undo `git submodule init`'s local registration in
        // `.git/config`; a no-op if it was never registered.
        let _ = git::ok(root, &["submodule", "deinit", "-f", &sm.path]);

        if worktree.exists() {
            con.step("Removing working tree");
            fs::remove_dir_all(&worktree)?;
        }

        if let Ok(gitdir) = sparse::gitdir(root, sm)
            && gitdir.exists()
        {
            con.step("Removing git dir");
            fs::remove_dir_all(&gitdir)?;
        }

        // Drop the gitlink from the index, if one was ever staged/committed.
        let _ = git::ok(root, &["rm", "--cached", "-f", "-q", "--", &sm.path]);

        con.step("Removing .gitmodules entry");
        config::remove(root, &sm.name)?;
        git::run(root, &["add", ".gitmodules"])?;

        con.success(format!("{} removed", sm.path));
    }
    con.plain("  staged .gitmodules + index removal, commit them to record it");
    Ok(())
}
