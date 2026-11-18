//! The optional post-update hook: a shell command a submodule may declare via
//! `picky.<name>.postUpdate`, run after its working tree is (re)materialized.
//! The seam for project-specific glue like ducky's `OVERRIDE_GIT_DESCRIBE` rewrite.
//!
//! The command text comes from `.gitmodules`, which is committed and travels
//! with the repo, so in a hostile clone it's attacker-controlled. It's never
//! run unconditionally: this mirrors git's own fix for CVE-2015-7545, which
//! let a malicious `.gitmodules` run arbitrary commands via
//! `submodule.<name>.update = !cmd`. Approval is recorded verbatim in
//! *local*, untracked config (`picky.<name>.trustedPostUpdate` /
//! `picky.<name>.trustedPostUpdateSha`, written to `.git/config`, never
//! `.gitmodules`) and re-asked whenever the command text or the submodule's
//! checked-out SHA changes. The SHA is pinned too: a hook that runs a script
//! inside the submodule can keep an identical `postUpdate` string across a
//! SHA bump while the script's contents change underneath it.

use anyhow::{Context, Result, bail};
use std::io::IsTerminal;
use std::path::Path;
use std::process::Command;

use crate::config::Submodule;
use crate::console::{self, Console};
use crate::git;

/// Run the submodule's `postUpdate` hook, if configured; a no-op when unset.
/// Refuses to run until `ensure_trusted` approves it. The command runs
/// through `sh -c` in the submodule's working tree, with `PICKY_*` env vars
/// describing the checkout. A non-zero exit is fatal.
pub fn run_post_update(root: &Path, sm: &Submodule, con: &Console) -> Result<()> {
    let Some(cmd) = &sm.post_update else {
        return Ok(());
    };

    let wt = root.join(&sm.path);
    // Not `unwrap_or_default()`: an empty SHA would silently degrade the trust
    // pin to the command text alone, which is exactly the weaker check the SHA
    // half exists to close.
    let sha = git::capture(&wt, &["rev-parse", "HEAD"])
        .with_context(|| format!("resolving HEAD in {} to pin hook trust", sm.path))?;

    ensure_trusted(root, sm, cmd, &sha, con)?;

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

/// The local-config keys a trust decision for `sm`'s hook is recorded under.
/// Deliberately not in `.gitmodules`: that file is exactly what a hostile
/// clone controls, so trust can't live there.
fn trust_key(sm: &Submodule) -> String {
    format!("picky.{}.trustedPostUpdate", sm.name)
}

/// The SHA half of the trust pin: a hook command that runs a script *inside*
/// the submodule (e.g. `sh scripts/hook.sh`) would keep an identical
/// `postUpdate` string across a SHA bump while the script's actual contents
/// changed underneath it, so the command text alone isn't a strong enough
/// pin.
fn trust_sha_key(sm: &Submodule) -> String {
    format!("picky.{}.trustedPostUpdateSha", sm.name)
}

/// Refuse to run `cmd` at `sha` unless it's approved for `sm`: already
/// trusted (local config holds this exact command text *and* SHA; either
/// changing invalidates it), approved interactively just now and then
/// recorded, or blanket-approved via `PICKY_TRUST_HOOKS=1` (for
/// CI/scripted use, where the caller already trusts the checked-out
/// content).
fn ensure_trusted(root: &Path, sm: &Submodule, cmd: &str, sha: &str, con: &Console) -> Result<()> {
    let key = trust_key(sm);
    let sha_key = trust_sha_key(sm);
    // A git failure must surface rather than read as unset, which would
    // re-prompt for approval on every run.
    let trusted_cmd = git::capture_unless_unset(root, &["config", "--get", &key])?;
    let trusted_sha = git::capture_unless_unset(root, &["config", "--get", &sha_key])?;
    if trusted_cmd.as_deref() == Some(cmd) && trusted_sha.as_deref() == Some(sha) {
        return Ok(());
    }

    if trust_env_set() {
        git::run(root, &["config", &key, cmd])?;
        git::run(root, &["config", &sha_key, sha])?;
        return Ok(());
    }

    con.warn(format!(
        "{} declares a post-update hook sourced from .gitmodules, a file \
         that ships with the repo and could carry a malicious command:",
        sm.path
    ));
    con.plain(format!("  {cmd}"));

    if !std::io::stdin().is_terminal() {
        bail!(
            "refusing to run an unapproved post-update hook non-interactively.\n  \
             Approve it once interactively, set PICKY_TRUST_HOOKS=1, or trust it \
             directly:\n  git config {key} {}\n  git config {sha_key} {sha}",
            sh_quote(cmd)
        );
    }

    if !console::confirm("  run it? [y/N] ")? {
        bail!(
            "post-update hook for {} was not approved; re-run and accept, \
             or remove `picky.{}.postUpdate` from .gitmodules",
            sm.path,
            sm.name
        );
    }
    git::run(root, &["config", &key, cmd])?;
    git::run(root, &["config", &sha_key, sha])?;
    Ok(())
}

/// Whether `PICKY_TRUST_HOOKS` blanket-approves hooks. Read as a git-style
/// boolean, so `PICKY_TRUST_HOOKS=0` (or `false`, or empty) does *not* trust:
/// a mere presence check would treat an explicit opt-out as an opt-in.
fn trust_env_set() -> bool {
    std::env::var("PICKY_TRUST_HOOKS")
        .ok()
        .and_then(|v| crate::config::parse_bool(v.trim()))
        .unwrap_or(false)
}

/// Single-quote `s` for a POSIX shell. The refusal message prints a
/// copy-pasteable `git config … <cmd>`, and `cmd` comes from `.gitmodules`:
/// interpolated raw, a hook like `x'; touch pwned; '` would turn the
/// suggested *approval* command into the attack itself.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::sh_quote;

    #[test]
    fn sh_quote_neutralizes_embedded_quotes() {
        assert_eq!(sh_quote("make all"), "'make all'");
        assert_eq!(
            sh_quote("x'; touch pwned; '"),
            r"'x'\''; touch pwned; '\'''"
        );
    }
}
