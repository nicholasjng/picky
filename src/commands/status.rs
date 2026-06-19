//! `picky status [<path>…]` — a table of each submodule's pin, branch, sparse
//! state, filter, working-tree size and patch count.

use anyhow::Result;
use std::path::Path;

use crate::config::{self, Submodule};
use crate::console::Console;
use crate::sparse;

const HEADERS: [&str; 7] = [
    "SUBMODULE",
    "PIN",
    "BRANCH",
    "SPARSE",
    "FILTER",
    "SIZE",
    "PATCHES",
];

pub fn run(root: &Path, paths: &[String], con: &Console) -> Result<()> {
    let subs: Vec<Submodule> = if paths.is_empty() {
        config::load_all(root)?
    } else {
        paths
            .iter()
            .map(|p| config::find(root, p))
            .collect::<Result<_>>()?
    };

    if subs.is_empty() {
        con.warn("no submodules declared in .gitmodules");
        return Ok(());
    }

    let rows: Vec<[String; 7]> = subs.iter().map(|sm| row(root, sm)).collect();

    let mut widths = HEADERS.map(str::len);
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }

    let render = |cells: &[String; 7]| {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c:<width$}", width = widths[i]))
            .collect::<Vec<_>>()
            .join("  ")
    };

    con.heading(render(&HEADERS.map(str::to_string)).trim_end());
    for row in &rows {
        con.plain(render(row).trim_end());
    }
    Ok(())
}

fn row(root: &Path, sm: &Submodule) -> [String; 7] {
    let pin = sparse::pinned_sha(root, &sm.path)
        .ok()
        .map(|s| s.get(..8).unwrap_or(&s).to_string())
        .unwrap_or_else(|| "-".into());

    let branch = sm.branch.clone().unwrap_or_else(|| "-".into());

    let sparse = if sm.sparse.is_empty() {
        "off".into()
    } else {
        format!("on({})", sm.sparse.len())
    };

    let filter = sm.effective_filter().unwrap_or("none").to_string();

    let checked_out = root.join(&sm.path).join(".git").exists();
    let size = if checked_out {
        sparse::worktree_size(root, sm).unwrap_or_else(|| "?".into())
    } else {
        "-".into()
    };

    [
        sm.path.clone(),
        pin,
        branch,
        sparse,
        filter,
        size,
        patch_count(root, sm),
    ]
}

fn patch_count(root: &Path, sm: &Submodule) -> String {
    let Some(dir) = &sm.patches else {
        return "-".into();
    };
    match std::fs::read_dir(root.join(dir)) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "patch"))
            .count()
            .to_string(),
        Err(_) => "-".into(),
    }
}
