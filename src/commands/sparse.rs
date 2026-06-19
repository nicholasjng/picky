//! `picky sparse <path> [--add <pat>]… [--remove <pat>]… [--clear] [--list]` —
//! edit a submodule's sparse-checkout patterns and reconcile the checkout.

use anyhow::{Result, bail};
use std::path::Path;

use crate::commands;
use crate::config;
use crate::console::Console;
use crate::git;

#[allow(clippy::too_many_arguments)]
pub fn run(
    root: &Path,
    path: Option<String>,
    add: Vec<String>,
    remove: Vec<String>,
    clear: bool,
    list: bool,
    no_reinit: bool,
    con: &Console,
) -> Result<()> {
    let sm = match path {
        Some(p) => config::find(root, &p)?,
        None => config::only(root)?,
    };

    if list {
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

    if add.is_empty() && remove.is_empty() && !clear {
        bail!("nothing to do — pass --add, --remove, --clear, or --list");
    }

    let mut patterns = if clear { Vec::new() } else { sm.sparse.clone() };
    for r in &remove {
        match patterns.iter().position(|p| p == r) {
            Some(pos) => {
                patterns.remove(pos);
            }
            None => con.warn(format!("pattern not present, skipping: {r}")),
        }
    }
    for a in &add {
        if patterns.contains(a) {
            con.warn(format!("pattern already present, skipping: {a}"));
        } else {
            patterns.push(a.clone());
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
