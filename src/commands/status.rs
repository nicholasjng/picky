//! `picky status [<path>…]`: a table of each submodule's pin, branch, upstream
//! staleness, sparse state, filter, working-tree size, object-store size and
//! patch count. `--json` emits the same fields as a JSON array, with the sizes
//! as byte counts.

use anyhow::Result;
use std::path::Path;

use crate::config::{self, Submodule};
use crate::console::Console;
use crate::{parallel, refcache, sparse};

const HEADERS: [&str; 9] = [
    "SUBMODULE",
    "PIN",
    "BRANCH",
    "UPSTREAM",
    "SPARSE",
    "FILTER",
    "SIZE",
    "STORE",
    "PATCHES",
];

/// One submodule's report. Sizes and counts stay numeric so `--json` can emit
/// machine-readable values while the table formats its own units.
struct Row {
    submodule: String,
    pin: String,
    branch: String,
    upstream: String,
    sparse: String,
    filter: String,
    /// Working-tree disk usage in bytes; `None` when not checked out or `du`
    /// is unavailable.
    size: Option<u64>,
    /// Object-store (submodule git dir) disk usage in bytes.
    store: Option<u64>,
    /// Patches in the declared stack; `None` when no `patches` dir is declared.
    patches: Option<usize>,
}

impl Row {
    /// The nine table cells, sizes rendered `du -h`-style.
    fn cells(&self) -> [String; 9] {
        let size = |b: Option<u64>| b.map_or_else(|| "-".into(), sparse::human_size);
        [
            self.submodule.clone(),
            self.pin.clone(),
            self.branch.clone(),
            self.upstream.clone(),
            self.sparse.clone(),
            self.filter.clone(),
            size(self.size),
            size(self.store),
            self.patches.map_or_else(|| "-".into(), |n| n.to_string()),
        ]
    }
}

pub fn run(root: &Path, paths: &[String], json: bool, con: &Console) -> Result<()> {
    let cfg = config::Config::load(root)?;
    let subs: Vec<Submodule> = if paths.is_empty() {
        cfg.all()?
    } else {
        paths.iter().map(|p| cfg.find(p)).collect::<Result<_>>()?
    };

    if subs.is_empty() {
        if json {
            con.plain("[]");
        } else {
            con.warn("no submodules declared in .gitmodules");
        }
        return Ok(());
    }

    // Rows are gathered concurrently: each costs a `git ls-files` plus two
    // directory walks, and on an LLVM-scale checkout one walk runs for
    // seconds. The modules dir is resolved once for all of them.
    let modules = sparse::modules_dir(root).ok();
    let rows = parallel::map(&subs, |sm| row(root, modules.as_deref(), sm));

    if json {
        con.plain(render_json(&rows));
        return Ok(());
    }

    let cells: Vec<[String; 9]> = rows.iter().map(Row::cells).collect();

    // Char counts, not `len()`: `{:<width$}` pads by chars, so a byte width
    // over-pads any cell holding a multi-byte character (the `stale→` arrow).
    let chars = |s: &str| s.chars().count();
    let mut widths = HEADERS.map(chars);
    for row in &cells {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(chars(cell));
        }
    }

    let render = |cells: &[String; 9]| {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c:<width$}", width = widths[i]))
            .collect::<Vec<_>>()
            .join("  ")
    };

    con.heading(render(&HEADERS.map(str::to_string)).trim_end());
    for row in &cells {
        con.plain(render(row).trim_end());
    }
    if rows.iter().any(|r| r.upstream == "?") {
        con.plain("  '?' = no cached ref data yet; run `picky refresh` to check for updates");
    }
    Ok(())
}

fn row(root: &Path, modules: Option<&Path>, sm: &Submodule) -> Row {
    let pin_full = sparse::pinned_sha(root, &sm.path).ok();
    let pin = pin_full
        .as_deref()
        .map(|s| s.get(..8).unwrap_or(s).to_string())
        .unwrap_or_else(|| "-".into());

    let checked_out = root.join(&sm.path).join(".git").exists();

    Row {
        submodule: sm.path.clone(),
        pin,
        branch: sm.branch.clone().unwrap_or_else(|| "-".into()),
        upstream: upstream_status(root, sm, pin_full.as_deref()),
        sparse: if sm.sparse.is_empty() {
            "off".into()
        } else {
            format!("on({})", sm.sparse.len())
        },
        filter: sm.effective_filter().unwrap_or("none").to_string(),
        size: checked_out
            .then(|| sparse::worktree_size(root, sm))
            .flatten(),
        // Reported even with no working tree, to surface an object store left
        // behind by a deleted checkout.
        store: modules.and_then(|m| sparse::store_size(m, sm)),
        patches: patch_count(root, sm),
    }
}

/// Compare the pin against the cached remote tip of the tracked branch
/// (populated by `picky refresh`; `status` itself is offline-only and never
/// touches the network). `-` when no branch is tracked (a bare SHA/tag pin
/// has no "latest" to compare against); `?` when there's no cache yet.
fn upstream_status(root: &Path, sm: &Submodule, pin: Option<&str>) -> String {
    let (Some(branch), Some(pin)) = (&sm.branch, pin) else {
        return "-".into();
    };
    // The resolved URL: the key `refresh` wrote under.
    let url = sparse::origin_url(root, sm).unwrap_or_else(|_| sm.url.clone());
    let Some(cached) = refcache::read(&url) else {
        return "?".into();
    };
    match cached.sha(branch) {
        Some(sha) if sha == pin => "current".into(),
        Some(sha) => format!("stale→{}", sha.get(..8).unwrap_or(sha)),
        None => "?".into(),
    }
}

fn patch_count(root: &Path, sm: &Submodule) -> Option<usize> {
    let dir = sm.patches.as_ref()?;
    let entries = std::fs::read_dir(root.join(dir)).ok()?;
    Some(
        entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "patch"))
            .count(),
    )
}

/// Hand-rolled JSON (no `serde_json` dependency for one output format): a
/// pretty-printed array of `{"submodule": …, "pin": …, …}` objects.
///
/// `size`, `store` and `patches` are numbers (bytes, or `null` when
/// unavailable) rather than the table's `"77M"`/`"-"` strings, which a script
/// cannot use.
fn render_json(rows: &[Row]) -> String {
    let mut out = String::from("[\n");
    for (i, row) in rows.iter().enumerate() {
        out.push_str("  {");
        for (j, (key, value)) in [
            ("submodule", Value::Str(&row.submodule)),
            ("pin", Value::Str(&row.pin)),
            ("branch", Value::Str(&row.branch)),
            ("upstream", Value::Str(&row.upstream)),
            ("sparse", Value::Str(&row.sparse)),
            ("filter", Value::Str(&row.filter)),
            ("size", Value::Num(row.size)),
            ("store", Value::Num(row.store)),
            ("patches", Value::Num(row.patches.map(|n| n as u64))),
        ]
        .into_iter()
        .enumerate()
        {
            if j > 0 {
                out.push_str(", ");
            }
            out.push('"');
            out.push_str(key);
            out.push_str("\": ");
            match value {
                Value::Str(s) => {
                    out.push('"');
                    json_escape_into(s, &mut out);
                    out.push('"');
                }
                Value::Num(Some(n)) => out.push_str(&n.to_string()),
                Value::Num(None) => out.push_str("null"),
            }
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

/// The two JSON value shapes a row cell takes.
enum Value<'a> {
    Str(&'a str),
    Num(Option<u64>),
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
