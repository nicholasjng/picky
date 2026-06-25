//! `picky add <url> <path> …`: build the sparse checkout, then write and
//! stage the `.gitmodules` entry and the new gitlink.

use anyhow::Result;
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

    // Only now record + stage the declaration. Suppress git's "embedded git
    // repository" hint: a gitlink is exactly what we want.
    config::write(root, &sm)?;
    git::run(root, &["add", ".gitmodules"])?;
    git::run(
        root,
        &["-c", "advice.addEmbeddedRepo=false", "add", &sm.path],
    )?;

    let sha = git::capture(&root.join(&sm.path), &["rev-parse", "HEAD"])?;
    let short = sha.get(..8).unwrap_or(&sha);
    match sparse::worktree_size(root, &sm) {
        Some(size) => con.success(format!("{} added at {short} ({size})", sm.path)),
        None => con.success(format!("{} added at {short}", sm.path)),
    }
    con.plain("  staged .gitmodules + gitlink, commit them to record the submodule");
    Ok(())
}
