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
/// remote doesn't silently full-fetch the object we're only probing for and
/// defeat our explicit shallow+blobless fetch.
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

/// Minimum git version picky requires (`GIT_NO_LAZY_FETCH` was added in 2.41).
const MIN_VERSION: (u32, u32, u32) = (2, 41, 0);

/// Verify a `git` on `PATH` exists and is new enough, before any real git
/// invocation runs. Without this, a missing git fails inside [`repo_root`]
/// with a misattributed "not inside a git repository", and a too-old git
/// fails confusingly deep inside whichever command first needs
/// `GIT_NO_LAZY_FETCH`.
pub fn check_version() -> Result<()> {
    let out = Command::new("git").arg("--version").output().context(
        "`git` not found on PATH: picky shells out to git and needs a recent version installed",
    )?;
    if !out.status.success() {
        bail!("`git --version` failed");
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let version = parse_version(&text)
        .with_context(|| format!("couldn't parse git version from: {}", text.trim()))?;
    if version < MIN_VERSION {
        bail!(
            "picky requires git >= {}.{}.{} (for GIT_NO_LAZY_FETCH), found {}.{}.{}; please upgrade",
            MIN_VERSION.0,
            MIN_VERSION.1,
            MIN_VERSION.2,
            version.0,
            version.1,
            version.2,
        );
    }
    Ok(())
}

/// Parse `"git version 2.55.0"` (possibly with a platform/distro suffix like
/// `"2.55.0.windows.1"` or `"2.39.2 (Apple Git-143)"`) into `(major, minor,
/// patch)`. A missing or unparseable patch component defaults to `0`.
fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let ver = text.trim().strip_prefix("git version ")?;
    let leading_digits = |s: &str| -> Option<u32> {
        s.chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .ok()
    };
    let mut parts = ver.split('.');
    let major = leading_digits(parts.next()?)?;
    let minor = leading_digits(parts.next()?)?;
    let patch = parts.next().and_then(leading_digits).unwrap_or(0);
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_handles_plain_and_suffixed_output() {
        assert_eq!(parse_version("git version 2.55.0\n"), Some((2, 55, 0)));
        assert_eq!(
            parse_version("git version 2.39.2 (Apple Git-143)\n"),
            Some((2, 39, 2))
        );
        assert_eq!(
            parse_version("git version 2.41.0.windows.1\n"),
            Some((2, 41, 0))
        );
        assert_eq!(parse_version("not git at all"), None);
    }

    #[test]
    fn min_version_ordering() {
        assert!((2, 40, 9) < MIN_VERSION);
        assert!((2, 41, 0) >= MIN_VERSION);
        assert!((3, 0, 0) >= MIN_VERSION);
    }
}
