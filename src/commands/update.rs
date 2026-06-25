//! `picky update [<path>] [<ref>] …`: bump the pin / re-checkout / re-apply
//! the patch stack. The `bump-duckdb.sh` equivalent.

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::Path;

use crate::config::{self, Submodule};
use crate::console::Console;
use crate::{git, hook, patch, sparse};

#[allow(clippy::too_many_arguments)]
pub fn run(
    root: &Path,
    arg1: Option<String>,
    arg2: Option<String>,
    no_patches: bool,
    unshallow: bool,
    depth: Option<u32>,
    all: bool,
    fresh: bool,
    con: &Console,
) -> Result<()> {
    if all {
        if arg1.is_some() || arg2.is_some() {
            bail!(
                "--all refreshes every submodule at its current pin and can't be combined with a path or ref"
            );
        }
        let targets = config::load_all(root)?;
        if targets.is_empty() {
            con.warn("no submodules declared in .gitmodules");
            return Ok(());
        }
        for sm in targets {
            con.heading(format!("submodule {}", sm.path));
            update_one(root, sm, None, no_patches, unshallow, depth, fresh, con)?;
        }
        return Ok(());
    }

    // Resolve the two optional positionals into (submodule, ref). With two
    // args it is unambiguous; with one, a value matching a submodule is a path
    // (refresh), otherwise it is a ref against the lone submodule.
    let (sm, refarg): (Submodule, Option<String>) = match (arg1, arg2) {
        (Some(a), Some(b)) => (config::find(root, &a)?, Some(b)),
        (Some(a), None) => match config::find(root, &a) {
            Ok(sm) => (sm, None),
            Err(_) => (config::only(root)?, Some(a)),
        },
        (None, _) => (config::only(root)?, None),
    };
    update_one(root, sm, refarg, no_patches, unshallow, depth, fresh, con)
}

/// Bump (or, with `refarg: None`, just refresh) a single submodule: fetch if
/// needed, re-checkout, re-apply the patch stack, run the post-update hook.
#[allow(clippy::too_many_arguments)]
fn update_one(
    root: &Path,
    mut sm: Submodule,
    refarg: Option<String>,
    no_patches: bool,
    unshallow: bool,
    depth: Option<u32>,
    fresh: bool,
    con: &Console,
) -> Result<()> {
    if let Some(d) = depth {
        sm.depth = Some(d);
    }

    let wt = root.join(&sm.path);
    if !wt.join(".git").exists() {
        bail!(
            "{0} is not checked out, run `picky init {0}` first",
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

    if fresh {
        // `git gc` can never reclaim a partial clone's old packs,
        // so the only way to bound the object store across repeated bumps
        // is to throw the git dir away and refetch just the target commit.
        con.step("Rebuilding from scratch (--fresh)");
        if wt.exists() {
            fs::remove_dir_all(&wt)?;
        }
        if let Ok(gitdir) = sparse::gitdir(root, &sm)
            && gitdir.exists()
        {
            fs::remove_dir_all(&gitdir)?;
        }
        sparse::prepare(root, &sm, con)?;
    }

    if bump || fresh {
        if unshallow {
            // Opt-in: full history + all tags for `git describe`; fattens the
            // (blobless) object store with every tree/commit on a big repo.
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

            // A bare branch resolves to the stale *local* ref the fetch never
            // advances; prefer the fresh remote-tracking ref. (SHAs, tags, and
            // explicit `origin/<x>` have no such ref and fall through.)
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
        } else {
            // Default: fetch only the target ref, shallow + blobless (like
            // `add`), no history download; a bare branch lands on the fresh
            // remote tip via FETCH_HEAD (no stale-local-ref footgun).
            //
            // Caveat: many git servers refuse to fetch an arbitrary commit SHA
            // directly unless it's an advertised branch/tag tip
            // (`uploadpack.allowReachableSHA1InWant`/`allowAnySHA1InWant`
            // unset). `--unshallow` sidesteps this by fetching full history
            // instead, so the SHA is already present locally to check out.
            sparse::fetch_ref(root, &sm, &refish, con).with_context(|| {
                format!(
                    "fetching '{refish}' failed; if it's a commit SHA, the \
                     remote may be refusing to fetch it directly (only \
                     advertised branch/tag tips), try \
                     `picky update {} {refish} --unshallow` to fetch full \
                     history and resolve it from there",
                    sm.path
                )
            })?;
            refish = "FETCH_HEAD".to_string();
        }
    }

    sparse::checkout(root, &sm, &refish, con)?;
    let new_sha = git::capture(&wt, &["rev-parse", "HEAD"])?;

    let applied = if no_patches {
        0
    } else {
        patch::apply_stack(root, &sm, con)?
    };

    hook::run_post_update(root, &sm, con)?;

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
            "  staged gitlink, commit {} to record the pin",
            sm.path
        ));
    }
    Ok(())
}
