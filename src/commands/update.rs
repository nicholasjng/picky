//! `picky update [<path>] [<ref>] …` — bump the pin / re-checkout / re-apply the
//! patch stack. The `bump-duckdb.sh` equivalent.

use anyhow::{Result, bail};
use std::path::Path;

use crate::config::{self, Submodule};
use crate::console::Console;
use crate::{git, patch, sparse};

pub fn run(
    root: &Path,
    arg1: Option<String>,
    arg2: Option<String>,
    no_patches: bool,
    depth: Option<u32>,
    con: &Console,
) -> Result<()> {
    // Resolve the two optional positionals into (submodule, ref). With two
    // args it is unambiguous; with one, a value matching a submodule is a path
    // (refresh), otherwise it is a ref against the lone submodule.
    let (mut sm, refarg): (Submodule, Option<String>) = match (arg1, arg2) {
        (Some(a), Some(b)) => (config::find(root, &a)?, Some(b)),
        (Some(a), None) => match config::find(root, &a) {
            Ok(sm) => (sm, None),
            Err(_) => (config::only(root)?, Some(a)),
        },
        (None, _) => (config::only(root)?, None),
    };
    if let Some(d) = depth {
        sm.depth = Some(d);
    }

    let wt = root.join(&sm.path);
    if !wt.join(".git").exists() {
        bail!(
            "{0} is not checked out — run `picky init {0}` first",
            sm.path
        );
    }

    let bump = refarg.is_some();
    let old_sha = git::capture(&wt, &["rev-parse", "HEAD"])?;

    // `checkout -f` below discards the working tree; warn if there are changes
    // not accounted for by patches/, so an unexported edit isn't lost silently.
    if !git::capture(&wt, &["status", "--porcelain"])?.is_empty() {
        con.warn(format!(
            "{} working tree is dirty; resetting to target + patches",
            sm.path
        ));
    }

    let mut refish = refarg.unwrap_or_else(|| old_sha.clone());

    if bump {
        // `describe`/history need the full graph; the checkout is shallow.
        let shallow = git::capture(&wt, &["rev-parse", "--is-shallow-repository"])? == "true";
        con.step("Fetching history + tags (blobless)");
        let filter = sm.effective_filter().map(|f| format!("--filter={f}"));
        let mut args = vec!["fetch", "--tags"];
        if let Some(f) = &filter {
            args.push(f);
        }
        if shallow {
            args.push("--unshallow");
        }
        args.push("origin");
        args.push("+refs/heads/*:refs/remotes/origin/*");
        git::run(&wt, &args)?;

        // A bare branch name resolves to the *local* branch ref, which the
        // fetch above never advances. Prefer the freshly-fetched remote ref so
        // we don't silently re-pin a stale local branch. (SHAs, tags, and an
        // explicit `origin/<x>` have no `refs/remotes/origin/<ref>` and fall
        // through unchanged.)
        if git::ok(
            &wt,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/remotes/origin/{refish}"),
            ],
        )? {
            con.step(format!("Resolving '{refish}' to 'origin/{refish}'"));
            refish = format!("origin/{refish}");
        }
    }

    sparse::checkout(root, &sm, &refish, con)?;
    let new_sha = git::capture(&wt, &["rev-parse", "HEAD"])?;

    let applied = if no_patches {
        0
    } else {
        patch::apply_stack(root, &sm, con)?
    };

    if bump {
        // Record the moved pin in the superproject index.
        git::run(root, &["add", &sm.path])?;
    }

    let short = |s: &str| s.get(..8).unwrap_or(s).to_string();
    con.plain("");
    if bump {
        con.success(format!(
            "bumped {}: {} -> {}",
            sm.path,
            short(&old_sha),
            short(&new_sha)
        ));
    } else {
        con.success(format!(
            "refreshed {} at {} (pin unchanged)",
            sm.path,
            short(&new_sha)
        ));
    }
    con.plain(format!(
        "  patches applied: {applied}{}",
        if no_patches { " (--no-patches)" } else { "" }
    ));
    if bump {
        con.plain(format!(
            "  staged gitlink — commit {} to record the pin",
            sm.path
        ));
    }
    Ok(())
}
