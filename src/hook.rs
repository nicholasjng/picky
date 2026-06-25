//! The optional post-update hook: a shell command a submodule may declare via
//! `picky.<name>.postUpdate`, run after its working tree is (re)materialized —
//! the seam for project-specific glue like ducky's `OVERRIDE_GIT_DESCRIBE` rewrite.
//!
//! The command text comes from `.gitmodules`, which is committed and travels
//! with the repo — in a hostile clone it is attacker-controlled. So it is
//! never run unconditionally: this mirrors git's own protection against
//! executable directives read from a versioned file (the fix for
//! CVE-2015-7545, which let a malicious `.gitmodules` run arbitrary commands
//! via `submodule.<name>.update = !cmd`). Approval is recorded verbatim in
//! *local*, untracked config (`picky.<name>.trustedPostUpdate`, written to
//! `.git/config`, never `.gitmodules`) and is re-asked whenever the command
//! text changes.

use anyhow::{Context, Result, bail};
use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::Command;

use crate::config::Submodule;
use crate::console::Console;
use crate::git;

/// Run the submodule's `postUpdate` hook, if configured; a no-op when unset.
/// Refuses to run until [`ensure_trusted`] approves it. The command runs
/// through `sh -c` in the submodule's working tree, with `PICKY_*` env vars
/// describing the checkout. A non-zero exit is fatal.
pub fn run_post_update(root: &Path, sm: &Submodule, con: &Console) -> Result<()> {
    let Some(cmd) = &sm.post_update else {
        return Ok(());
    };
    ensure_trusted(root, sm, cmd, con)?;

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

/// The local-config key a trust decision for `sm`'s hook is recorded under.
/// Deliberately not in `.gitmodules` — that file is exactly what a hostile
/// clone controls, so trust can't be allowed to live there.
fn trust_key(sm: &Submodule) -> String {
    format!("picky.{}.trustedPostUpdate", sm.name)
}

/// Refuse to run `cmd` unless it is approved for `sm`: already trusted (local
/// config holds this exact command text — any edit invalidates it), approved
/// interactively just now (and then recorded), or blanket-approved via
/// `PICKY_TRUST_HOOKS=1` (for CI / scripted use, where the caller already
/// trusts the checked-out content).
fn ensure_trusted(root: &Path, sm: &Submodule, cmd: &str, con: &Console) -> Result<()> {
    let key = trust_key(sm);
    if git::capture_opt(root, &["config", "--get", &key])?.as_deref() == Some(cmd) {
        return Ok(());
    }

    if std::env::var_os("PICKY_TRUST_HOOKS").is_some() {
        git::run(root, &["config", &key, cmd])?;
        return Ok(());
    }

    con.warn(format!(
        "{} declares a post-update hook sourced from .gitmodules — a file \
         that ships with the repo and could carry a malicious command:",
        sm.path
    ));
    con.plain(format!("  {cmd}"));

    if !std::io::stdin().is_terminal() {
        bail!(
            "refusing to run an unapproved post-update hook non-interactively.\n  \
             Approve it once interactively, set PICKY_TRUST_HOOKS=1, or trust it \
             directly:\n  git config {key} '{cmd}'"
        );
    }

    eprint!("  run it? [y/N] ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("reading hook approval from stdin")?;
    if !matches!(line.trim().to_lowercase().as_str(), "y" | "yes") {
        bail!(
            "post-update hook for {} was not approved — re-run and accept, \
             or remove `picky.{}.postUpdate` from .gitmodules",
            sm.path,
            sm.name
        );
    }
    git::run(root, &["config", &key, cmd])?;
    Ok(())
}
