//! `picky add <url> <path> …`: build the sparse checkout, then write and
//! stage the `.gitmodules` entry and the new gitlink.

use anyhow::{Result, bail};
use std::path::Path;

use crate::config::{self, Submodule};
use crate::console::Console;
use crate::{git, hook, sparse};

#[allow(clippy::too_many_arguments)]
pub fn run(
    root: &Path,
    url: String,
    path: String,
    name: Option<String>,
    sparse_pats: Vec<String>,
    depth: Option<u32>,
    filter: Option<String>,
    branch: Option<String>,
    reference: Option<String>,
    patches: Option<String>,
    post_update: Option<String>,
    con: &Console,
) -> Result<()> {
    // Before anything is created, fetched, or run: a path that escapes the
    // superproject would otherwise materialize a full clone outside the repo
    // and run the post-update hook there, only failing at the final `git add`.
    config::check_path(&path)?;
    config::check_depth(depth)?;

    // This path is about to be `git init`ed and checked out into. Re-running
    // `add` over picky's own checkout is supported (it converges the
    // declaration), so a directory holding a `.git` is fine; unrelated content
    // is not, since checking out over it cannot be undone.
    let dest = root.join(&path);
    if dest.exists() && !dest.is_dir() {
        bail!("'{path}' exists and is not a directory");
    }
    if dest.is_dir() && !dest.join(".git").exists() && dest.read_dir()?.next().is_some() {
        bail!(
            "'{path}' already exists and is not a git checkout; picky would check out \
             on top of its contents. Move it aside, or choose another path."
        );
    }

    let sm = Submodule {
        name: name.unwrap_or_else(|| path.clone()),
        path,
        url,
        branch: branch.clone(),
        // Default to a shallow, lightweight checkout.
        shallow: true,
        sparse: sparse_pats,
        depth,
        filter,
        patches,
        post_update,
    };

    con.heading(format!("adding submodule {}", sm.path));

    // Build the checkout first, entirely from the in-memory `sm`: nothing
    // touches `.gitmodules` on disk until it succeeds, so a failed add (bad
    // URL, bad --ref, network) leaves no half-declared submodule behind.
    sparse::prepare(root, &sm, con)?;

    // No gitlink exists yet, so fetch the requested ref (or the remote HEAD)
    // and detach onto it.
    let target = reference.as_deref().or(branch.as_deref()).unwrap_or("HEAD");
    sparse::fetch_ref(root, &sm, target, con)?;
    sparse::checkout(root, &sm, "FETCH_HEAD", con)?;
    hook::run_post_update(root, &sm, con)?;

    // Only now record + stage the declaration. `--no-warn-embedded-repo`
    // rather than `-c advice.addEmbeddedRepo=false`: the advice setting drops
    // the follow-up hint but still prints the "adding embedded git repository"
    // warning itself, and a gitlink is exactly what we want here.
    config::write(root, &sm)?;
    git::run(root, &["add", ".gitmodules"])?;
    git::run(root, &["add", "--no-warn-embedded-repo", "--", &sm.path])?;

    let sha = git::capture(&root.join(&sm.path), &["rev-parse", "HEAD"])?;
    let short = sha.get(..8).unwrap_or(&sha);
    match sparse::worktree_size(root, &sm) {
        Some(size) => con.success(format!("{} added at {short} ({size})", sm.path)),
        None => con.success(format!("{} added at {short}", sm.path)),
    }
    con.plain("  staged .gitmodules + gitlink, commit them to record the submodule");
    Ok(())
}
