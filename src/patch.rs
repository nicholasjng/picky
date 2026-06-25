//! The working-tree patch overlay: discover `<patches>/*.patch` in lexical
//! order and `git apply --3way` each onto the submodule. Ports the patch loop
//! from `bump-duckdb.sh`; a failing apply is fatal and leaves conflict markers.

use anyhow::{Result, bail};
use std::path::Path;

use crate::config::Submodule;
use crate::console::Console;
use crate::git;

/// Apply the submodule's patch stack, returning the number applied. A no-op
/// (returning 0) when the submodule has no `patches` dir or it is empty.
pub fn apply_stack(root: &Path, sm: &Submodule, con: &Console) -> Result<usize> {
    let Some(dir) = &sm.patches else {
        return Ok(0);
    };
    let patch_dir = root.join(dir);
    if !patch_dir.is_dir() {
        return Ok(0);
    }

    let mut patches: Vec<_> = std::fs::read_dir(&patch_dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "patch"))
        .collect();
    patches.sort();
    if patches.is_empty() {
        return Ok(0);
    }

    con.step(format!("Applying patch stack from {dir}/"));
    let wt = root.join(&sm.path);
    let mut applied = 0usize;
    let mut failed = Vec::new();
    for p in &patches {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let abs = p.to_string_lossy();
        // Run the real apply (not `--check`): `--3way` returns non-zero AND
        // leaves conflict markers when upstream context drifted, which a
        // separate check does not predict.
        if git::ok(&wt, &["apply", "--3way", &abs])? {
            con.item(format!("applied  {name}"));
            applied += 1;
        } else {
            con.error(format!("FAILED   {name}"));
            failed.push(name);
        }
    }

    if !failed.is_empty() {
        bail!(
            "patch(es) need rebasing: {}\n  \
             Conflict markers are left in {}: resolve them, regenerate the patch \
             (git -C {} diff HEAD > {}/...), or retire it.",
            failed.join(", "),
            sm.path,
            sm.path,
            dir
        );
    }
    Ok(applied)
}
