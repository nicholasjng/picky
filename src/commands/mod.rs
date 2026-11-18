pub mod add;
pub mod completions;
pub mod doctor;
pub mod init;
pub mod refresh;
pub mod remove;
pub mod sparse;
pub mod status;
pub mod update;

use anyhow::{Result, bail};

use crate::config::Submodule;
use crate::console::Console;

/// Run `f` for every target, carrying on past a failure and failing at the
/// end with a summary, so one broken submodule (an unreachable remote, a
/// patch that no longer applies) doesn't leave the rest of an `--all` run
/// undone. A single target fails with its own error, unwrapped.
pub(crate) fn for_each(
    targets: &[Submodule],
    con: &Console,
    mut f: impl FnMut(&Submodule) -> Result<()>,
) -> Result<()> {
    if let [only] = targets {
        return f(only);
    }
    let failed = summarize(targets.iter().map(|sm| (sm, f(sm))), con);
    if failed.is_empty() {
        Ok(())
    } else {
        bail!(
            "{} of {} submodule(s) failed: {}",
            failed.len(),
            targets.len(),
            failed.join(", ")
        )
    }
}

/// Report each failed `(submodule, result)` as an error line, returning the
/// failed paths in order.
pub(crate) fn summarize<'a>(
    results: impl IntoIterator<Item = (&'a Submodule, Result<()>)>,
    con: &Console,
) -> Vec<String> {
    let mut failed = Vec::new();
    for (sm, result) in results {
        if let Err(e) = result {
            con.error(format!("{}: {e:#}", sm.path));
            failed.push(sm.path.clone());
        }
    }
    failed
}
