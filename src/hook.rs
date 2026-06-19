//! The optional post-update hook: a shell command a submodule may declare via
//! `picky.<name>.postUpdate`, run after its working tree is (re)materialized.
//! This is the seam for project-specific glue like ducky's `OVERRIDE_GIT_DESCRIBE`
//! CMake rewrite, which v1 deliberately leaves out of the core.

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;

use crate::config::Submodule;
use crate::console::Console;
use crate::git;

/// Run the submodule's `postUpdate` hook, if configured; a no-op when unset.
/// The command runs through `sh -c` in the submodule's working tree, with
/// `PICKY_*` env vars describing the checkout. A non-zero exit is fatal.
pub fn run_post_update(root: &Path, sm: &Submodule, con: &Console) -> Result<()> {
    let Some(cmd) = &sm.post_update else {
        return Ok(());
    };
    let wt = root.join(&sm.path);
    let sha = git::capture(&wt, &["rev-parse", "HEAD"]).unwrap_or_default();

    con.step("Running post-update hook");
    con.detail(cmd);
    let status = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(&wt)
        .env("PICKY_ROOT", root)
        .env("PICKY_SUBMODULE_NAME", &sm.name)
        .env("PICKY_SUBMODULE_PATH", &sm.path)
        .env("PICKY_SUBMODULE_SHA", &sha)
        .status()
        .with_context(|| format!("failed to launch post-update hook for {}", sm.path))?;
    if !status.success() {
        bail!("post-update hook for {} failed ({status})", sm.path);
    }
    Ok(())
}
