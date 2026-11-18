//! The declarative submodule model, read/written via `git config -f .gitmodules`.
//! Git's standard keys live in `submodule.<name>` (`path`, `url`, `branch`,
//! `shallow`); picky's own options live in a parallel `picky.<name>` section
//! that stock git ignores, joined by the shared subsection name.

use anyhow::{Context, Result, bail};
use std::path::Path;

use crate::git;

const FILE: &str = ".gitmodules";

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Submodule {
    /// `submodule.<name>` section name (conventionally equal to `path`).
    pub name: String,
    pub path: String,
    pub url: String,
    pub branch: Option<String>,
    pub shallow: bool,
    /// Non-cone sparse-checkout patterns; empty ⇒ full checkout.
    pub sparse: Vec<String>,
    pub depth: Option<u32>,
    /// Partial-clone filter; `None` ⇒ the `blob:none` default, literal `none`
    /// disables filtering. See [`Submodule::effective_filter`].
    pub filter: Option<String>,
    /// Directory holding the `*.patch` overlay stack, if any.
    pub patches: Option<String>,
    /// Shell command run after the working tree is (re)materialized, if any.
    /// The seam for project-specific glue like ducky's `OVERRIDE_GIT_DESCRIBE`.
    pub post_update: Option<String>,
}

impl Submodule {
    /// The filter to fetch with: an explicit value, or the `blob:none`
    /// default. A configured literal `none` disables filtering entirely.
    pub fn effective_filter(&self) -> Option<&str> {
        match self.filter.as_deref() {
            None => Some("blob:none"),
            Some("none") => None,
            Some(other) => Some(other),
        }
    }

    /// Shallow fetch depth: explicit `depth`, else 1 when `shallow = true`.
    pub fn effective_depth(&self) -> Option<u32> {
        self.depth.or(if self.shallow { Some(1) } else { None })
    }
}

/// Reject a submodule path that would escape the superproject.
///
/// `.gitmodules` is committed and travels with the repo, so in a hostile clone
/// this string is attacker-controlled, and picky both materializes a checkout
/// at it and deletes it recursively (`remove`, `update --fresh`). A path of
/// `..`, `/abs`, or `.` would put those operations outside the repo root, `.`
/// aiming a `remove_dir_all` straight at the superproject itself. Git itself
/// refuses such names ("ignoring suspicious submodule name"); this is the same
/// guard, applied before picky acts rather than after.
pub fn check_path(path: &str) -> Result<()> {
    use std::path::Component;
    if path.is_empty() {
        bail!("submodule path is empty");
    }
    let p = Path::new(path);
    if !p
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
    {
        bail!(
            "submodule path '{path}' must stay inside the superproject \
             (no leading '/', no '..')"
        );
    }
    if !p.components().any(|c| matches!(c, Component::Normal(_))) {
        bail!("submodule path '{path}' does not name a directory inside the superproject");
    }
    Ok(())
}

/// [`check_path`], plus a check of what is on disk: no existing component of
/// `<root>/<path>` may be a symlink.
///
/// `check_path` is lexical. With `ext` committed as a symlink to `$HOME`,
/// `ext/dep` passes it while `root.join("ext/dep")` resolves to `$HOME/dep`,
/// where the recursive deletes in `remove` and `update --fresh`, and the
/// checkout `add` builds, would follow it.
pub fn check_path_on_disk(root: &Path, path: &str) -> Result<()> {
    use std::path::Component;
    check_path(path)?;
    let mut cur = root.to_path_buf();
    for c in Path::new(path).components() {
        let Component::Normal(part) = c else { continue };
        cur.push(part);
        match std::fs::symlink_metadata(&cur) {
            Ok(meta) if meta.file_type().is_symlink() => bail!(
                "submodule path '{path}' runs through a symlink ({}); picky refuses \
                 to build or delete a checkout outside the superproject",
                cur.strip_prefix(root).unwrap_or(&cur).display()
            ),
            Ok(_) => {}
            // Nothing exists from here down, so nothing further can be a link.
            Err(_) => break,
        }
    }
    Ok(())
}

/// Parse a `picky.<name>.depth` value.
///
/// Zero is rejected here rather than at the fetch, where `git fetch --depth=0`
/// dies with "depth 0 is not a positive number" only after the git dir is
/// built and the remote configured, leaving a half-built checkout behind.
fn parse_depth(name: &str, s: &str) -> Result<u32> {
    let depth: u32 = s
        .parse()
        .with_context(|| format!("picky.{name}.depth is not a positive integer: '{s}'"))?;
    check_depth(Some(depth))?;
    Ok(depth)
}

/// Reject a zero fetch depth (see [`parse_depth`]). Clap bounds the `--depth`
/// flags; this guards the library entry points, which never go through the
/// parser.
pub fn check_depth(depth: Option<u32>) -> Result<()> {
    if depth == Some(0) {
        bail!("depth must be at least 1 (`--depth 0` is not a valid shallow fetch)");
    }
    Ok(())
}

/// `.gitmodules` parsed once, in full.
///
/// Every lookup used to be its own `git config --get`, so building the
/// submodule list cost roughly nine `git` spawns per submodule plus one to
/// enumerate them — paid on every command, and on every TAB press through the
/// completion candidates. One `--list` covers the whole file instead.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// `(key, value)` in file order, duplicates kept: `picky.<name>.sparse` is
    /// multivalued, and git resolves a single-valued key to the *last* entry.
    entries: Vec<(String, String)>,
}

impl Config {
    /// Read and parse `<root>/.gitmodules`.
    ///
    /// An absent file is an empty config. A file that is present but
    /// unparseable is an error, not an empty config: read as "nothing
    /// declared", a syntax error would be reported as `no submodules declared
    /// in .gitmodules`, and `doctor --strict` would pass on a broken file.
    pub fn load(root: &Path) -> Result<Self> {
        if !root.join(FILE).exists() {
            return Ok(Self::default());
        }
        let out = git::capture_unless_unset(root, &["config", "-f", FILE, "--list", "-z"])
            .with_context(|| format!("reading {FILE}"))?;
        Ok(out.as_deref().map(Self::parse).unwrap_or_default())
    }

    /// Parse `git config --list -z`: NUL-separated entries, each `key\nvalue`,
    /// or a bare `key` when it was set with no value.
    ///
    /// The `-z` form (rather than line-oriented `--list`) is what makes a value
    /// containing a newline survive: a multi-line `sparse` pattern would
    /// otherwise be read back as several separate patterns.
    fn parse(out: &str) -> Self {
        let entries = out
            .split('\0')
            .filter(|e| !e.is_empty())
            .map(|entry| match entry.split_once('\n') {
                Some((k, v)) => (k.to_string(), v.to_string()),
                None => (entry.to_string(), String::new()),
            })
            .collect();
        Self { entries }
    }

    /// The value of a single-valued key: the last one set, as git resolves it.
    fn get(&self, key: &str) -> Option<&str> {
        let key = normalize_key(key);
        self.entries
            .iter()
            .rev()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Every value of a multivalued key, in file order.
    fn get_all(&self, key: &str) -> Vec<String> {
        let key = normalize_key(key);
        self.entries
            .iter()
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v.clone())
            .collect()
    }

    /// All `submodule.<name>` section names declared, in file order. A section
    /// counts as a submodule only once it sets `path`.
    pub fn names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for (key, _) in &self.entries {
            if let Some(rest) = key.strip_prefix("submodule.")
                && let Some(name) = rest.strip_suffix(".path")
                && !names.iter().any(|n| n == name)
            {
                names.push(name.to_string());
            }
        }
        names
    }

    /// All `<section>.<name>` subsection names, whichever keys they set
    /// (unlike [`Config::names`], which recognizes a submodule by its `.path`
    /// key). Used by `picky doctor` to spot a `picky.<name>` section with no
    /// matching `submodule.<name>`.
    pub fn section_names(&self, section: &str) -> Vec<String> {
        let prefix = format!("{section}.");
        let mut names = Vec::new();
        for (key, _) in &self.entries {
            if let Some(rest) = key.strip_prefix(&prefix)
                && let Some((name, _key)) = rest.rsplit_once('.')
                && !names.iter().any(|n| n == name)
            {
                names.push(name.to_string());
            }
        }
        names
    }

    /// One submodule's full configuration: git's keys from
    /// `submodule.<name>.*`, picky's from `picky.<name>.*`.
    pub fn submodule(&self, name: &str) -> Result<Submodule> {
        let sub = |k: &str| format!("submodule.{name}.{k}");
        let own = |k: &str| format!("picky.{name}.{k}");
        Ok(Submodule {
            name: name.to_string(),
            path: self
                .get(&sub("path"))
                .map_or_else(|| name.to_string(), str::to_string),
            url: self.get(&sub("url")).unwrap_or_default().to_string(),
            branch: self.get(&sub("branch")).map(str::to_string),
            shallow: self.get(&sub("shallow")) == Some("true"),
            sparse: self.get_all(&own("sparse")),
            depth: self
                .get(&own("depth"))
                .map(|s| parse_depth(name, s))
                .transpose()?,
            filter: self.get(&own("filter")).map(str::to_string),
            patches: self.get(&own("patches")).map(str::to_string),
            post_update: self.get(&own("postUpdate")).map(str::to_string),
        })
    }

    /// Every declared submodule, in file order.
    pub fn all(&self) -> Result<Vec<Submodule>> {
        self.names().iter().map(|n| self.submodule(n)).collect()
    }

    /// The submodule whose section name or `path` matches `needle`.
    pub fn find(&self, needle: &str) -> Result<Submodule> {
        for name in self.names() {
            let sm = self.submodule(&name)?;
            if sm.name == needle || sm.path == needle {
                return Ok(sm);
            }
        }
        bail!("no submodule matching '{needle}' in .gitmodules");
    }

    /// The lone submodule, erroring if there is not exactly one (used when a
    /// command's path argument is omitted).
    pub fn only(&self) -> Result<Submodule> {
        let mut all = self.all()?;
        match all.len() {
            1 => Ok(all.remove(0)),
            0 => bail!("no submodules declared in .gitmodules"),
            _ => bail!("multiple submodules declared, specify a path"),
        }
    }
}

/// Parse a git config boolean value (see `git help config`, "boolean"):
/// `true`/`yes`/`on`/non-zero and `false`/`no`/`off`/`0`/empty, any case.
/// `None` for anything else.
pub fn parse_bool(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" => Some(true),
        "false" | "no" | "off" | "" => Some(false),
        n => n.parse::<i64>().ok().map(|n| n != 0),
    }
}

/// Render a lookup key the way `git config --list` renders one: section and
/// variable lowercased, the subsection between them left verbatim.
///
/// The subsection is the only case-sensitive part of a git config key, so
/// `picky.<name>.postUpdate` is listed back as `picky.<name>.postupdate` while
/// `<name>` keeps its case. Comparing against the un-normalized key silently
/// found nothing for every mixed-case variable picky defines.
fn normalize_key(key: &str) -> String {
    let (Some(first), Some(last)) = (key.find('.'), key.rfind('.')) else {
        return key.to_ascii_lowercase();
    };
    if first == last {
        // `section.variable`, no subsection: both halves are case-insensitive.
        return key.to_ascii_lowercase();
    }
    format!(
        "{}{}{}",
        key[..first].to_ascii_lowercase(),
        &key[first..=last],
        key[last + 1..].to_ascii_lowercase()
    )
}

/// All `submodule.<name>` section names declared in `.gitmodules`.
pub fn names(root: &Path) -> Result<Vec<String>> {
    Ok(Config::load(root)?.names())
}

/// All `<section>.<name>` subsection names declared in `.gitmodules`.
pub fn section_names(root: &Path, section: &str) -> Result<Vec<String>> {
    Ok(Config::load(root)?.section_names(section))
}

/// Read a single submodule's full configuration.
pub fn load(root: &Path, name: &str) -> Result<Submodule> {
    Config::load(root)?.submodule(name)
}

/// Every submodule declared in `.gitmodules`.
pub fn load_all(root: &Path) -> Result<Vec<Submodule>> {
    Config::load(root)?.all()
}

/// Find the submodule whose section name or `path` matches `needle`.
pub fn find(root: &Path, needle: &str) -> Result<Submodule> {
    Config::load(root)?.find(needle)
}

/// The lone submodule, erroring if there is not exactly one.
pub fn only(root: &Path) -> Result<Submodule> {
    Config::load(root)?.only()
}

/// Replace the multivalued `picky.<name>.sparse` list for a submodule. Clears
/// the existing values and re-adds `patterns` verbatim, so removal is by exact
/// value (no `git config --unset` regex interpretation of patterns like
/// `/extension/*.cmake`).
pub fn set_sparse(root: &Path, name: &str, patterns: &[String]) -> Result<()> {
    let key = format!("picky.{name}.sparse");
    let _ = git::ok(root, &["config", "-f", FILE, "--unset-all", &key]);
    for pat in patterns {
        git::run(root, &["config", "-f", FILE, "--add", &key, pat])?;
    }
    Ok(())
}

/// Write a submodule entry into `.gitmodules` (used by `picky add`). Git's keys
/// go in `submodule.<name>.*`, picky's in `picky.<name>.*`. Fully declarative:
/// an absent optional field *clears* any previously-written value, so
/// re-running `write` for an existing name converges to exactly `sm` instead
/// of unioning old and new (e.g. re-adding without `--branch` drops a
/// previously-set branch rather than leaving it stale).
pub fn write(root: &Path, sm: &Submodule) -> Result<()> {
    let sub = |k: &str| format!("submodule.{}.{}", sm.name, k);
    let own = |k: &str| format!("picky.{}.{}", sm.name, k);

    git::run(root, &["config", "-f", FILE, &sub("path"), &sm.path])?;
    git::run(root, &["config", "-f", FILE, &sub("url"), &sm.url])?;
    set_or_unset(root, &sub("branch"), sm.branch.as_deref())?;
    set_or_unset(root, &sub("shallow"), sm.shallow.then_some("true"))?;
    let depth = sm.depth.map(|d| d.to_string());
    set_or_unset(root, &own("depth"), depth.as_deref())?;
    set_or_unset(root, &own("filter"), sm.filter.as_deref())?;
    set_or_unset(root, &own("patches"), sm.patches.as_deref())?;
    set_or_unset(root, &own("postUpdate"), sm.post_update.as_deref())?;
    // Multivalued: clear any prior list, then append each pattern.
    set_sparse(root, &sm.name, &sm.sparse)?;
    Ok(())
}

/// Set `key` to `value`, or unset it when `value` is `None` (best-effort: a
/// key that was never set is not an error). The primitive that makes
/// [`write`] declarative rather than additive.
fn set_or_unset(root: &Path, key: &str, value: Option<&str>) -> Result<()> {
    match value {
        Some(v) => git::run(root, &["config", "-f", FILE, key, v]),
        None => {
            let _ = git::ok(root, &["config", "-f", FILE, "--unset", key]);
            Ok(())
        }
    }
}

/// Remove a submodule's declaration from `.gitmodules`: git's
/// `submodule.<name>` section and picky's own `picky.<name>` section (which
/// may not exist if no picky-specific option was ever set).
pub fn remove(root: &Path, name: &str) -> Result<()> {
    git::run(
        root,
        &[
            "config",
            "-f",
            FILE,
            "--remove-section",
            &format!("submodule.{name}"),
        ],
    )?;
    let _ = git::ok(
        root,
        &[
            "config",
            "-f",
            FILE,
            "--remove-section",
            &format!("picky.{name}"),
        ],
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Config, check_path, normalize_key};

    /// `git config --list -z`: NUL-separated entries of `key\nvalue`.
    fn cfg(entries: &[&str]) -> Config {
        Config::parse(&entries.join("\0"))
    }

    #[test]
    fn parse_reads_keys_values_and_multivalues() {
        let c = cfg(&[
            "submodule.ext/dep.path\next/dep",
            "submodule.ext/dep.url\nhttps://example.com/x.git",
            "picky.ext/dep.sparse\n/src/*",
            "picky.ext/dep.sparse\n/docs/*",
        ]);
        assert_eq!(c.names(), vec!["ext/dep"]);
        let sm = c.submodule("ext/dep").unwrap();
        assert_eq!(sm.path, "ext/dep");
        assert_eq!(sm.url, "https://example.com/x.git");
        assert_eq!(sm.sparse, vec!["/src/*", "/docs/*"]);
    }

    #[test]
    fn single_valued_key_resolves_to_the_last_entry() {
        // git's own `--get` semantics for a key set twice.
        let c = cfg(&["submodule.a.path\nfirst", "submodule.a.path\nsecond"]);
        let sm = c.submodule("a").unwrap();
        assert_eq!(sm.path, "second");
        assert_eq!(
            c.names(),
            vec!["a"],
            "a repeated key is still one submodule"
        );
    }

    #[test]
    fn a_value_containing_a_newline_survives() {
        // The reason for `-z`: line-oriented `--list` would read this back as
        // two separate patterns.
        let c = cfg(&["picky.a.sparse\nmulti\nline"]);
        let sm = c.submodule("a").unwrap();
        assert_eq!(sm.sparse, vec!["multi\nline"]);
    }

    #[test]
    fn valueless_key_parses_as_empty() {
        let c = cfg(&["picky.a.bare", "submodule.a.path\na"]);
        assert_eq!(c.submodule("a").unwrap().path, "a");
    }

    #[test]
    fn lookups_match_gits_key_case_folding() {
        // `--list` lowercases section and variable but preserves subsection.
        let c = cfg(&["picky.Ext/Dep.postupdate\nmake"]);
        assert_eq!(
            c.submodule("Ext/Dep").unwrap().post_update.as_deref(),
            Some("make")
        );
        assert_eq!(
            normalize_key("picky.Ext/Dep.postUpdate"),
            "picky.Ext/Dep.postupdate"
        );
        assert_eq!(normalize_key("Core.SparseCheckout"), "core.sparsecheckout");
    }

    #[test]
    fn an_empty_config_declares_nothing() {
        let c = Config::parse("");
        assert!(c.names().is_empty());
        assert!(c.all().unwrap().is_empty());
        assert!(c.only().is_err());
        assert!(c.find("nope").is_err());
    }

    #[test]
    fn a_malformed_depth_is_reported_not_ignored() {
        let c = cfg(&["submodule.a.path\na", "picky.a.depth\nabc"]);
        let err = c.submodule("a").unwrap_err().to_string();
        assert!(err.contains("depth"), "got: {err}");
    }

    #[test]
    fn check_path_accepts_paths_inside_the_superproject() {
        for ok in ["ext/dep", "dep", "a/b/c", "./ext/dep"] {
            assert!(check_path(ok).is_ok(), "{ok} should be accepted");
        }
    }

    #[test]
    fn check_path_rejects_escapes() {
        // `..` escapes; `.` and "" would aim a recursive delete at the root.
        for bad in ["..", "../outside", "ext/../..", "/abs/path", ".", ""] {
            assert!(check_path(bad).is_err(), "{bad:?} should be rejected");
        }
    }
}
