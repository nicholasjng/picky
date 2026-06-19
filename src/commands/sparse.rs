//! `picky sparse <list|add|remove|clear>` — inspect and edit a submodule's
//! sparse-checkout patterns, reconciling the checkout afterwards.

use anyhow::Result;
use std::path::Path;

use crate::commands;
use crate::config;
use crate::console::Console;
use crate::git;

/// What to do to the pattern list. `List` only reads; the rest mutate and
/// (unless `no_reinit`) reconcile the checkout via `init`.
pub enum Action {
    List,
    Add(Vec<String>),
    Remove(Vec<String>),
    Clear,
}

pub fn run(
    root: &Path,
    path: Option<String>,
    action: Action,
    no_reinit: bool,
    con: &Console,
) -> Result<()> {
    let sm = match path {
        Some(p) => config::find(root, &p)?,
        None => config::only(root)?,
    };

    // Resolve the action into the next pattern list (or print + return for List).
    let mut patterns = match &action {
        Action::List => {
            if sm.sparse.is_empty() {
                con.plain(format!("{}: no sparse patterns (full checkout)", sm.path));
            } else {
                con.heading(format!("{} sparse patterns:", sm.path));
                for p in &sm.sparse {
                    con.plain(format!("  {p}"));
                }
            }
            return Ok(());
        }
        Action::Clear => Vec::new(),
        Action::Add(_) | Action::Remove(_) => sm.sparse.clone(),
    };

    if let Action::Remove(remove) = &action {
        for r in remove {
            match patterns.iter().position(|p| p == r) {
                Some(pos) => {
                    patterns.remove(pos);
                }
                None => con.warn(format!("pattern not present, skipping: {r}")),
            }
        }
    }
    if let Action::Add(add) = &action {
        for a in add {
            if patterns.contains(a) {
                con.warn(format!("pattern already present, skipping: {a}"));
            } else {
                patterns.push(a.clone());
            }
        }
    }

    if patterns == sm.sparse {
        con.plain(format!("{}: sparse patterns unchanged", sm.path));
        return Ok(());
    }

    config::set_sparse(root, &sm.name, &patterns)?;
    git::run(root, &["add", ".gitmodules"])?;
    con.success(format!(
        "{}: now {} sparse pattern(s) configured (.gitmodules staged)",
        sm.path,
        patterns.len()
    ));

    if no_reinit {
        con.plain("  run `picky init` to reconcile the checkout");
    } else {
        // Reconcile: rewrites the sparse file and reapplies (widening
        // materializes new paths, narrowing trims removed ones).
        commands::init::run(root, std::slice::from_ref(&sm.path), con)?;
    }
    Ok(())
}
