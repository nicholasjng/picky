//! `picky init [<path>…]` — reconstruct sparse checkouts from committed config.
//! The `init-duckdb.sh` equivalent. With no paths, every declared submodule.

use anyhow::Result;
use std::path::Path;

use crate::config::{self, Submodule};
use crate::console::Console;
use crate::{git, hook, sparse};

pub fn run(root: &Path, paths: &[String], con: &Console) -> Result<()> {
    let targets: Vec<Submodule> = if paths.is_empty() {
        config::load_all(root)?
    } else {
        paths
            .iter()
            .map(|p| config::find(root, p))
            .collect::<Result<_>>()?
    };

    if targets.is_empty() {
        con.warn("no submodules declared in .gitmodules");
        return Ok(());
    }

    for sm in &targets {
        con.heading(format!("submodule {}", sm.path));
        // Register the submodule so `git status` tracks it; tolerate the case
        // where it is not yet committed (picky configures the remote itself).
        let _ = git::ok(root, &["submodule", "init", &sm.path]);

        let sha = sparse::pinned_sha(root, &sm.path)?;
        con.detail(format!("pinned at {sha}"));
        let gitdir = sparse::prepare(root, sm, con)?;
        con.detail(format!("git dir {}", gitdir.display()));
        sparse::ensure_commit(root, sm, &sha, con)?;
        sparse::checkout(root, sm, &sha, con)?;
        hook::run_post_update(root, sm, con)?;

        match sparse::worktree_size(root, sm) {
            Some(size) => con.success(format!("{} ready ({size})", sm.path)),
            None => con.success(format!("{} ready", sm.path)),
        }
    }
    Ok(())
}
