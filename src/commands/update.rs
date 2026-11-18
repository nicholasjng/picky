//! `picky update [<path>] [<ref>] …`: bump the pin / re-checkout / re-apply
//! the patch stack. The `bump-duckdb.sh` equivalent.

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::Path;

use crate::config::{self, Submodule};
use crate::console::Console;
use crate::{git, hook, patch, sparse};

/// What one `picky update` invocation should do. [`Default`] is a plain
/// refresh: the lone submodule, onto its pin, patches applied.
///
/// A struct rather than a positional argument list because the flags are five
/// consecutive `bool`/`Option` parameters, where transposing two type-checks
/// silently.
#[derive(Debug, Clone, Default)]
pub struct UpdateOptions {
    /// First positional: a submodule path, or a ref when only one submodule
    /// is declared.
    pub target: Option<String>,
    /// Second positional: the ref to bump to.
    pub reference: Option<String>,
    /// Skip the patch stack.
    pub no_patches: bool,
    /// Fetch full history + all tags rather than only the target ref.
    pub unshallow: bool,
    /// Override the shallow fetch depth.
    pub depth: Option<u32>,
    /// Refresh every declared submodule; can't be combined with
    /// `target`/`reference`.
    pub all: bool,
    /// Wipe the git dir and rebuild from scratch at the target.
    pub fresh: bool,
}

pub fn run(root: &Path, opts: UpdateOptions, con: &Console) -> Result<()> {
    if opts.all {
        // Not redundant with clap's `conflicts_with_all`: `picky::update` is a
        // public library entry point that never goes through the parser.
        if opts.target.is_some() || opts.reference.is_some() {
            bail!(
                "--all refreshes every submodule onto its pinned commit and can't be combined with a path or ref"
            );
        }
        let targets = config::Config::load(root)?.all()?;
        if targets.is_empty() {
            con.warn("no submodules declared in .gitmodules");
            return Ok(());
        }
        for sm in targets {
            con.heading(format!("submodule {}", sm.path));
            update_one(root, sm, None, &opts, con)?;
        }
        return Ok(());
    }

    // Resolve the two optional positionals into (submodule, ref). With two
    // args it is unambiguous; with one, a value matching a submodule is a path
    // (refresh), otherwise it is a ref against the lone submodule.
    let cfg = config::Config::load(root)?;
    let (sm, refarg): (Submodule, Option<String>) =
        match (opts.target.clone(), opts.reference.clone()) {
            (Some(a), Some(b)) => (cfg.find(&a)?, Some(b)),
            (Some(a), None) => match cfg.find(&a) {
                Ok(sm) => (sm, None),
                // Name the argument: with several submodules, a mistyped
                // path otherwise surfaces as a bare "specify a path".
                Err(_) => (
                    cfg.only()
                        .with_context(|| format!("'{a}' is not a declared submodule path"))?,
                    Some(a),
                ),
            },
            (None, _) => (cfg.only()?, None),
        };
    update_one(root, sm, refarg, &opts, con)
}

/// Bump (or, with `refarg: None`, just refresh) a single submodule: fetch if
/// needed, re-checkout, re-apply the patch stack, run the post-update hook.
fn update_one(
    root: &Path,
    mut sm: Submodule,
    refarg: Option<String>,
    opts: &UpdateOptions,
    con: &Console,
) -> Result<()> {
    config::check_depth(opts.depth)?;
    if let Some(d) = opts.depth {
        sm.depth = Some(d);
    }
    let fresh = opts.fresh;

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
    //
    // "Dirty" alone can't mean that: applying the patch stack *is* what makes
    // the tree dirty, so a plain `status --porcelain` check fired on every run
    // after the first and trained the user to ignore it. Compare against the
    // state picky itself left behind last time instead.
    let state = worktree_state(&wt)?;
    if let Some(state) = &state
        && recorded_state(root, &sm)?.as_deref() != Some(state.as_str())
    {
        con.warn(format!(
            "{} has changes beyond its patch stack; resetting to target + patches",
            sm.path
        ));
    }

    // A refresh (no ref given) targets the superproject's gitlink pin, not the
    // submodule's current `HEAD`: the two differ after a pulled commit moves
    // the pin, which is what a refresh has to catch up with.
    let mut refish = match refarg {
        Some(r) => r,
        None => sparse::pinned_sha(root, &sm.path)
            .with_context(|| format!("resolving the commit pinned for {}", sm.path))?,
    };

    if fresh {
        // `git gc` can never reclaim a partial clone's old packs,
        // so the only way to bound the object store across repeated bumps
        // is to throw the git dir away and refetch just the target commit.
        con.step("Rebuilding from scratch (--fresh)");
        // Same guard as `remove`: this wipes the path recursively.
        config::check_path_on_disk(root, &sm.path)?;
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
        if opts.unshallow {
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
    } else {
        // A moved pin may name a commit this shallow checkout has never
        // fetched, so make it present before `checkout` asks for it.
        sparse::ensure_commit(root, &sm, &refish, con)?;
    }

    sparse::checkout(root, &sm, &refish, con)?;
    let new_sha = git::capture(&wt, &["rev-parse", "HEAD"])?;

    let applied = if opts.no_patches {
        0
    } else {
        patch::apply_stack(root, &sm, con)?
    };

    hook::run_post_update(root, &sm, con)?;

    // Remember what picky left behind, so the next run can tell its own patch
    // stack apart from a genuine unexported edit.
    record_state(root, &sm, worktree_state(&wt)?.as_deref())?;

    if bump {
        // Record the moved pin in the superproject index.
        git::run(root, &["add", "--", &sm.path])?;
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
    } else if new_sha == old_sha {
        con.success(format!(
            "refreshed {} at {} (already on its pin)",
            sm.path,
            short(&new_sha)
        ));
    } else {
        // The pin moved under this checkout and the refresh caught it up.
        con.success(format!(
            "refreshed {}: {} -> {} (moved onto its pin)",
            sm.path,
            short(&old_sha),
            short(&new_sha)
        ));
    }
    con.plain(format!(
        "  patches applied: {applied}{}",
        if opts.no_patches {
            " (--no-patches)"
        } else {
            ""
        }
    ));
    if bump {
        con.plain(format!(
            "  staged gitlink, commit {} to record the pin",
            sm.path
        ));
    }
    Ok(())
}

/// A fingerprint of the submodule's working-tree deviation from `HEAD`, or
/// `None` when it is clean. Covers tracked modifications (`diff HEAD`) *and*
/// untracked/staged entries (`status --porcelain`): a patch that only edits a
/// file already listed as modified would not change the status line on its
/// own, so the diff text has to be part of it.
fn worktree_state(wt: &Path) -> Result<Option<String>> {
    let status = git::capture(wt, &["status", "--porcelain"])?;
    if status.is_empty() {
        return Ok(None);
    }
    let diff = git::capture(wt, &["diff", "HEAD"])?;
    Ok(Some(crate::hash::fnv1a(&format!("{status}\0{diff}"))))
}

/// The key holding the last state picky left in this submodule. Local config
/// (`.git/config`), never `.gitmodules`: it describes one checkout on one
/// machine, not the declaration that travels with the repo.
fn state_key(sm: &Submodule) -> String {
    format!("picky.{}.lastWorktreeState", sm.name)
}

fn recorded_state(root: &Path, sm: &Submodule) -> Result<Option<String>> {
    // A git failure must surface rather than read as "nothing recorded", which
    // would compare unequal and fire the warning below spuriously.
    git::capture_unless_unset(root, &["config", "--get", &state_key(sm)])
}

fn record_state(root: &Path, sm: &Submodule, state: Option<&str>) -> Result<()> {
    let key = state_key(sm);
    match state {
        Some(s) => git::run(root, &["config", &key, s]),
        // A clean tree records nothing, so a later edit to a patch-free
        // submodule still warns.
        None => {
            let _ = git::ok(root, &["config", "--unset", &key]);
            Ok(())
        }
    }
}
