//! `picky init [<path>…]`: reconstruct sparse checkouts from committed config.
//! The `init-duckdb.sh` equivalent. With no paths, every declared submodule.

use anyhow::{Result, bail};
use std::path::Path;

use crate::commands;
use crate::config::{self, Submodule};
use crate::console::Console;
use crate::{hook, parallel, sparse};

pub fn run(root: &Path, paths: &[String], con: &Console) -> Result<()> {
    let cfg = config::Config::load(root)?;
    let targets: Vec<Submodule> = if paths.is_empty() {
        cfg.all()?
    } else {
        paths.iter().map(|p| cfg.find(p)).collect::<Result<_>>()?
    };

    match targets.as_slice() {
        [] => {
            con.warn("no submodules declared in .gitmodules");
            Ok(())
        }
        [sm] => init_one(root, sm, con),
        _ => init_many(root, &targets, con),
    }
}

/// One submodule, every step reported as it happens.
fn init_one(root: &Path, sm: &Submodule, con: &Console) -> Result<()> {
    con.heading(format!("submodule {}", sm.path));
    let sha = prepare(root, sm, con)?;
    sparse::ensure_commit(root, sm, &sha, con)?;
    sparse::checkout(root, sm, &sha, con)?;
    finish(root, sm, con)
}

/// Several submodules, with the network-bound middle run concurrently.
///
/// Three phases, because only the middle one is safe to overlap:
///
/// 1. Pin + git dir setup, in order. `prepare` registers each submodule with
///    `git submodule init`/`sync`, which write the superproject's
///    `.git/config`, and git refuses a second writer while one holds its lock.
/// 2. Fetch + checkout, concurrently. Each touches only its own submodule
///    repository, and on a blobless clone this is where the time goes: the
///    commit fetch, then the lazy blob fetch the checkout triggers.
/// 3. Post-update hook + report, in order. A hook may prompt for approval and
///    records it in `.git/config`, so these run one at a time again.
///
/// A failure drops that submodule from the later phases; the rest carry on,
/// and the run fails at the end with the list (as `update --all` does).
fn init_many(root: &Path, targets: &[Submodule], con: &Console) -> Result<()> {
    let mut results: Vec<(&Submodule, Result<()>)> = Vec::new();
    let mut ready: Vec<(&Submodule, String)> = Vec::new();
    for sm in targets {
        con.heading(format!("submodule {}", sm.path));
        match prepare(root, sm, con) {
            Ok(sha) => ready.push((sm, sha)),
            Err(e) => results.push((sm, Err(e))),
        }
    }

    if !ready.is_empty() {
        con.step(format!(
            "Fetching and checking out {} submodule(s) concurrently",
            ready.len()
        ));
    }
    // Per-step progress from concurrent workers would interleave into noise,
    // so the steps run silently and each reports one line when it's done.
    let quiet = Console::silent();
    let fetched = parallel::map(&ready, |(sm, sha)| {
        let result = sparse::ensure_commit(root, sm, sha, &quiet)
            .and_then(|()| sparse::checkout(root, sm, sha, &quiet));
        if result.is_ok() {
            con.item(format!(
                "{} checked out at {}",
                sm.path,
                &sha[..sha.len().min(8)]
            ));
        }
        result
    });

    for ((sm, _), result) in ready.iter().zip(fetched) {
        let result = result.and_then(|()| finish(root, sm, con));
        results.push((sm, result));
    }

    let failed = commands::summarize(results, con);
    if !failed.is_empty() {
        bail!(
            "{} of {} submodule(s) failed: {}",
            failed.len(),
            targets.len(),
            failed.join(", ")
        );
    }
    Ok(())
}

/// Resolve the pin and build/configure the git dir; returns the pinned SHA.
/// Registration and relative-`url` resolution happen in `sparse::prepare`.
fn prepare(root: &Path, sm: &Submodule, con: &Console) -> Result<String> {
    let sha = sparse::pinned_sha(root, &sm.path)?;
    con.detail(format!("pinned at {sha}"));
    let gitdir = sparse::prepare(root, sm, con)?;
    con.detail(format!("git dir {}", gitdir.display()));
    Ok(sha)
}

/// Run the post-update hook and report the result.
fn finish(root: &Path, sm: &Submodule, con: &Console) -> Result<()> {
    hook::run_post_update(root, sm, con)?;
    match sparse::worktree_size(root, sm) {
        Some(size) => con.success(format!("{} ready ({})", sm.path, sparse::human_size(size))),
        None => con.success(format!("{} ready", sm.path)),
    }
    Ok(())
}
