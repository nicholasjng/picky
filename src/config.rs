//! The declarative submodule model, read/written via `git config -f .gitmodules`.
//! Git's standard keys live in `submodule.<name>` (`path`, `url`, `branch`,
//! `shallow`); picky's own options live in a parallel `picky.<name>` section
//! that stock git ignores, joined by the shared subsection name.

use anyhow::{Result, bail};
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

fn get(root: &Path, key: &str) -> Result<Option<String>> {
    git::capture_opt(root, &["config", "-f", FILE, "--get", key])
}

fn get_all(root: &Path, key: &str) -> Result<Vec<String>> {
    match git::capture_opt(root, &["config", "-f", FILE, "--get-all", key])? {
        Some(s) if !s.is_empty() => Ok(s.lines().map(str::to_string).collect()),
        _ => Ok(Vec::new()),
    }
}

/// All `submodule.<name>` section names declared in `.gitmodules`.
pub fn names(root: &Path) -> Result<Vec<String>> {
    let out = git::capture_opt(
        root,
        &[
            "config",
            "-f",
            FILE,
            "--name-only",
            "--get-regexp",
            r"^submodule\..*\.path$",
        ],
    )?;
    let Some(out) = out else {
        return Ok(Vec::new());
    };
    let mut names = Vec::new();
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("submodule.")
            && let Some(name) = rest.strip_suffix(".path")
        {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

/// Read a single submodule's full configuration: git's keys from
/// `submodule.<name>.*`, picky's keys from `picky.<name>.*`.
pub fn load(root: &Path, name: &str) -> Result<Submodule> {
    let sub = |k: &str| format!("submodule.{name}.{k}");
    let own = |k: &str| format!("picky.{name}.{k}");
    Ok(Submodule {
        name: name.to_string(),
        path: get(root, &sub("path"))?.unwrap_or_else(|| name.to_string()),
        url: get(root, &sub("url"))?.unwrap_or_default(),
        branch: get(root, &sub("branch"))?,
        shallow: get(root, &sub("shallow"))?.as_deref() == Some("true"),
        sparse: get_all(root, &own("sparse"))?,
        depth: get(root, &own("depth"))?.and_then(|s| s.parse().ok()),
        filter: get(root, &own("filter"))?,
        patches: get(root, &own("patches"))?,
        post_update: get(root, &own("postUpdate"))?,
    })
}

/// Every submodule declared in `.gitmodules`.
pub fn load_all(root: &Path) -> Result<Vec<Submodule>> {
    names(root)?.iter().map(|n| load(root, n)).collect()
}

/// Find the submodule whose section name or `path` matches `needle`.
pub fn find(root: &Path, needle: &str) -> Result<Submodule> {
    for name in names(root)? {
        let sm = load(root, &name)?;
        if sm.name == needle || sm.path == needle {
            return Ok(sm);
        }
    }
    bail!("no submodule matching '{needle}' in .gitmodules");
}

/// The lone submodule, erroring if there is not exactly one (used when a
/// command's path argument is omitted).
pub fn only(root: &Path) -> Result<Submodule> {
    let mut all = load_all(root)?;
    match all.len() {
        1 => Ok(all.remove(0)),
        0 => bail!("no submodules declared in .gitmodules"),
        _ => bail!("multiple submodules declared — specify a path"),
    }
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
/// go in `submodule.<name>.*`, picky's in `picky.<name>.*`.
pub fn write(root: &Path, sm: &Submodule) -> Result<()> {
    let sub = |k: &str| format!("submodule.{}.{}", sm.name, k);
    let own = |k: &str| format!("picky.{}.{}", sm.name, k);
    let set = |k: &str, v: &str| git::run(root, &["config", "-f", FILE, k, v]);

    set(&sub("path"), &sm.path)?;
    set(&sub("url"), &sm.url)?;
    if let Some(branch) = &sm.branch {
        set(&sub("branch"), branch)?;
    }
    if sm.shallow {
        set(&sub("shallow"), "true")?;
    }
    if let Some(depth) = sm.depth {
        set(&own("depth"), &depth.to_string())?;
    }
    if let Some(filter) = &sm.filter {
        set(&own("filter"), filter)?;
    }
    if let Some(patches) = &sm.patches {
        set(&own("patches"), patches)?;
    }
    if let Some(post_update) = &sm.post_update {
        set(&own("postUpdate"), post_update)?;
    }
    // Multivalued: clear any prior list, then append each pattern.
    set_sparse(root, &sm.name, &sm.sparse)?;
    Ok(())
}
