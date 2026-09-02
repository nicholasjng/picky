//! `picky init [<path>…]`: reconstruct sparse checkouts from committed config.
//! The `init-duckdb.sh` equivalent. With no paths, every declared submodule.

use anyhow::Result;
use std::path::Path;

use crate::config::{self, Submodule};
use crate::console::Console;
use crate::{hook, sparse};

pub fn run(root: &Path, paths: &[String], con: &Console) -> Result<()> {
    let cfg = config::Config::load(root)?;
    let targets: Vec<Submodule> = if paths.is_empty() {
        cfg.all()?
    } else {
        paths.iter().map(|p| cfg.find(p)).collect::<Result<_>>()?
    };

    if targets.is_empty() {
        con.warn("no submodules declared in .gitmodules");
        return Ok(());
    }

    for sm in &targets {
        con.heading(format!("submodule {}", sm.path));
        // Registration and relative-`url` resolution happen in
        // `sparse::prepare`, below.
        let sha = sparse::pinned_sha(root, &sm.path)?;
        con.detail(format!("pinned at {sha}"));
        let gitdir = sparse::prepare(root, sm, con)?;
        con.detail(format!("git dir {}", gitdir.display()));
        sparse::ensure_commit(root, sm, &sha, con)?;
        sparse::checkout(root, sm, &sha, con)?;
        hook::run_post_update(root, sm, con)?;

        match sparse::worktree_size(root, sm) {
            Some(size) => con.success(format!("{} ready ({})", sm.path, sparse::human_size(size))),
            None => con.success(format!("{} ready", sm.path)),
        }
    }
    Ok(())
}
