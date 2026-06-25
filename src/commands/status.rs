//! `picky status [<path>…]`: a table of each submodule's pin, branch,
//! upstream staleness, sparse state, filter, working-tree size and patch
//! count. `--json` emits the same fields as a JSON array instead.

use anyhow::Result;
use std::path::Path;

use crate::config::{self, Submodule};
use crate::console::Console;
use crate::{refcache, sparse};

const HEADERS: [&str; 8] = [
    "SUBMODULE",
    "PIN",
    "BRANCH",
    "UPSTREAM",
    "SPARSE",
    "FILTER",
    "SIZE",
    "PATCHES",
];

/// Lowercase field names for `--json`, positionally matching [`HEADERS`] /
/// the cells [`row`] produces.
const JSON_KEYS: [&str; 8] = [
    "submodule",
    "pin",
    "branch",
    "upstream",
    "sparse",
    "filter",
    "size",
    "patches",
];

pub fn run(root: &Path, paths: &[String], json: bool, con: &Console) -> Result<()> {
    let subs: Vec<Submodule> = if paths.is_empty() {
        config::load_all(root)?
    } else {
        paths
            .iter()
            .map(|p| config::find(root, p))
            .collect::<Result<_>>()?
    };

    if subs.is_empty() {
        if json {
            con.plain("[]");
        } else {
            con.warn("no submodules declared in .gitmodules");
        }
        return Ok(());
    }

    let rows: Vec<[String; 8]> = subs.iter().map(|sm| row(root, sm)).collect();

    if json {
        con.plain(render_json(&rows));
        return Ok(());
    }

    let mut widths = HEADERS.map(str::len);
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.len());
        }
    }

    let render = |cells: &[String; 8]| {
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
    if rows.iter().any(|r| r[3] == "?") {
        con.plain("  '?' = no cached ref data yet; run `picky refresh` to check for updates");
    }
    Ok(())
}

fn row(root: &Path, sm: &Submodule) -> [String; 8] {
    let pin_full = sparse::pinned_sha(root, &sm.path).ok();
    let pin = pin_full
        .as_deref()
        .map(|s| s.get(..8).unwrap_or(s).to_string())
        .unwrap_or_else(|| "-".into());

    let branch = sm.branch.clone().unwrap_or_else(|| "-".into());
    let upstream = upstream_status(sm, pin_full.as_deref());

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
        upstream,
        sparse,
        filter,
        size,
        patch_count(root, sm),
    ]
}

/// Compare the pin against the cached remote tip of the tracked branch
/// (populated by `picky refresh`; `status` itself is offline-only and never
/// touches the network). `-` when no branch is tracked (a bare SHA/tag pin
/// has no "latest" to compare against); `?` when there's no cache yet.
fn upstream_status(sm: &Submodule, pin: Option<&str>) -> String {
    let (Some(branch), Some(pin)) = (&sm.branch, pin) else {
        return "-".into();
    };
    let Some(cached) = refcache::read(&sm.url) else {
        return "?".into();
    };
    match cached.sha(branch) {
        Some(sha) if sha == pin => "current".into(),
        Some(sha) => format!("stale→{}", sha.get(..8).unwrap_or(sha)),
        None => "?".into(),
    }
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

/// Hand-rolled JSON (no `serde_json` dependency for one output format): a
/// pretty-printed array of `{"submodule": …, "pin": …, …}` objects, keys
/// positionally matching [`JSON_KEYS`]/[`HEADERS`].
fn render_json(rows: &[[String; 8]]) -> String {
    let mut out = String::from("[\n");
    for (i, row) in rows.iter().enumerate() {
        out.push_str("  {");
        for (j, (key, val)) in JSON_KEYS.iter().zip(row.iter()).enumerate() {
            if j > 0 {
                out.push_str(", ");
            }
            out.push('"');
            out.push_str(key);
            out.push_str("\": \"");
            json_escape_into(val, &mut out);
            out.push('"');
        }
        out.push('}');
        if i + 1 < rows.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push(']');
    out
}

fn json_escape_into(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
}
