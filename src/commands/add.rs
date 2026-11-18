//! `picky add <url> <path> …`: build the sparse checkout, then write and
//! stage the `.gitmodules` entry and the new gitlink.

use anyhow::{Result, bail};
use std::path::Path;

use crate::config::{self, Submodule};
use crate::console::Console;
use crate::{git, hook, sparse};

/// Everything `picky add` needs to declare and build a submodule.
///
/// A struct rather than a positional argument list because six of these fields
/// are `Option<String>` and two more are `String`, where transposing any two
/// type-checks silently.
#[derive(Debug, Clone)]
pub struct AddOptions {
    /// Remote URL, absolute or `../`-relative.
    pub url: String,
    /// Checkout path within the superproject.
    pub path: String,
    /// `.gitmodules` section name; defaults to `path`.
    pub name: Option<String>,
    /// Non-cone sparse-checkout patterns; empty ⇒ full checkout.
    pub sparse: Vec<String>,
    /// Shallow fetch depth; `add` defaults to `shallow = true` (depth 1).
    pub depth: Option<u32>,
    /// Partial-clone filter; `None` ⇒ the `blob:none` default.
    pub filter: Option<String>,
    /// Branch to track, recorded as `submodule.<name>.branch`.
    pub branch: Option<String>,
    /// Ref to pin to instead of the remote `HEAD`.
    pub reference: Option<String>,
    /// Directory holding the `*.patch` overlay stack.
    pub patches: Option<String>,
    /// Shell command to run after each checkout (post-update hook).
    pub post_update: Option<String>,
}

impl AddOptions {
    /// The two required fields, every optional one unset:
    ///
    /// ```
    /// # use picky::AddOptions;
    /// let opts = AddOptions {
    ///     sparse: vec!["/src/".into()],
    ///     ..AddOptions::new("https://example.com/x.git", "ext/dep")
    /// };
    /// assert_eq!(opts.path, "ext/dep");
    /// ```
    pub fn new(url: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            path: path.into(),
            name: None,
            sparse: Vec::new(),
            depth: None,
            filter: None,
            branch: None,
            reference: None,
            patches: None,
            post_update: None,
        }
    }
}

pub fn run(root: &Path, opts: AddOptions, con: &Console) -> Result<()> {
    let AddOptions {
        url,
        path,
        name,
        sparse: sparse_pats,
        depth,
        filter,
        branch,
        reference,
        patches,
        post_update,
    } = opts;

    // Before anything is created, fetched, or run: a path that escapes the
    // superproject would otherwise materialize a full clone outside the repo
    // and run the post-update hook there, only failing at the final `git add`.
    config::check_path_on_disk(root, &path)?;
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
        Some(size) => con.success(format!(
            "{} added at {short} ({})",
            sm.path,
            sparse::human_size(size)
        )),
        None => con.success(format!("{} added at {short}", sm.path)),
    }
    con.plain("  staged .gitmodules + gitlink, commit them to record the submodule");
    Ok(())
}
