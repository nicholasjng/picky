//! The single place that spawns `git`. Every partial-clone / sparse-checkout
//! incantation goes through one of these helpers, so the surface git CLI we
//! depend on stays auditable in one module.

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::{Command, Stdio};

fn base(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args);
    cmd
}

/// Run `git <args>` in `dir` with stdio inherited so progress is visible.
/// Errors on a non-zero exit.
pub fn run(dir: &Path, args: &[&str]) -> Result<()> {
    let status = base(dir, args)
        .status()
        .with_context(|| format!("failed to spawn `git {}`", args.join(" ")))?;
    if !status.success() {
        bail!("`git {}` exited with {status}", args.join(" "));
    }
    Ok(())
}

/// Run `git <args>` in `dir`, returning trimmed stdout. Errors on a non-zero
/// exit, surfacing stderr.
pub fn capture(dir: &Path, args: &[&str]) -> Result<String> {
    let out = base(dir, args)
        .output()
        .with_context(|| format!("failed to spawn `git {}`", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "`git {}` failed:\n{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim_end()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// Run `git <args>` in `dir`, returning `Some(stdout)` on success and `None`
/// on a clean failure (e.g. `config --get` of a missing key). Spawn failures
/// still bubble up.
pub fn capture_opt(dir: &Path, args: &[&str]) -> Result<Option<String>> {
    let out = base(dir, args)
        .output()
        .with_context(|| format!("failed to spawn `git {}`", args.join(" ")))?;
    if out.status.success() {
        Ok(Some(
            String::from_utf8_lossy(&out.stdout).trim_end().to_string(),
        ))
    } else {
        Ok(None)
    }
}

/// Run `git <args>` silently, returning whether it succeeded. For predicate
/// queries like `cat-file -e`, `remote get-url`, `rev-parse --verify`.
pub fn ok(dir: &Path, args: &[&str]) -> Result<bool> {
    ok_env(dir, args, &[])
}

/// Like [`ok`] but with extra environment variables set. Used to run the
/// object-presence check with `GIT_NO_LAZY_FETCH=1`, so a configured promisor
/// remote does not silently fetch (unfiltered, full-depth) the object we are
/// only probing for — that would defeat our explicit shallow+blobless fetch.
pub fn ok_env(dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> Result<bool> {
    let mut cmd = base(dir, args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let status = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("failed to spawn `git {}`", args.join(" ")))?;
    Ok(status.success())
}

/// `git rev-parse --show-toplevel` from the current directory.
pub fn repo_root() -> Result<std::path::PathBuf> {
    let root = capture(Path::new("."), &["rev-parse", "--show-toplevel"])
        .context("not inside a git repository")?;
    Ok(std::path::PathBuf::from(root))
}
