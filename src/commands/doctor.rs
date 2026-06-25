//! `picky doctor`: sanity-check the repo's submodule state against
//! `.gitmodules`. Dangling gitlinks, orphaned submodule git dirs left behind
//! by hand-editing `.gitmodules` instead of `add`/`remove`, and half-edited
//! `.gitmodules` sections. Diagnostic by default; reports issues as warnings
//! and exits 0 regardless. `--strict` exits 1 when issues are found, for a
//! pre-commit hook or CI check.

use anyhow::{Result, bail};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::config;
use crate::console::Console;
use crate::git;

pub fn run(root: &Path, strict: bool, con: &Console) -> Result<()> {
    let declared = config::load_all(root)?;
    let declared_paths: HashSet<&str> = declared.iter().map(|sm| sm.path.as_str()).collect();
    let mut issues = 0usize;

    for sm in &declared {
        let dotgit = root.join(&sm.path).join(".git");
        if dotgit.exists() {
            let valid = dotgit
                .to_str()
                .map(|s| git::ok(root, &["rev-parse", "--resolve-git-dir", s]).unwrap_or(false))
                .unwrap_or(false);
            if !valid {
                con.warn(format!(
                    "{}: dangling gitlink, `picky init {}` will rebuild it",
                    sm.path, sm.path
                ));
                issues += 1;
            }
        }
    }

    let mut gitdirs = Vec::new();
    find_gitdirs(&root.join(".git/modules"), Path::new(""), &mut gitdirs);
    for rel in &gitdirs {
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if !declared_paths.contains(rel_str.as_str()) {
            con.warn(format!(
                "orphaned git dir .git/modules/{rel_str}: no submodule declares this path; \
                 remove it with `rm -rf .git/modules/{rel_str}` if it's no longer needed"
            ));
            issues += 1;
        }
    }

    let sub_names: HashSet<String> = config::section_names(root, "submodule")?
        .into_iter()
        .collect();
    let picky_names: HashSet<String> = config::section_names(root, "picky")?.into_iter().collect();
    for name in picky_names.difference(&sub_names) {
        con.warn(format!(
            "picky.{name} section in .gitmodules has no matching submodule.{name} section, \
             probably a half-finished manual edit"
        ));
        issues += 1;
    }

    if issues == 0 {
        con.success("no issues found");
    } else {
        con.plain(format!("  {issues} issue(s) found"));
        if strict {
            bail!("{issues} issue(s) found (--strict)");
        }
    }
    Ok(())
}

/// Recursively find leaf directories under `base` that look like a git dir
/// (have both `HEAD` and `config`), returning each one's path relative to
/// `base`: the shape `.git/modules/<submodule path>` takes for a submodule
/// whose own path contains slashes (e.g. `ext/dep`).
fn find_gitdirs(base: &Path, rel: &Path, out: &mut Vec<PathBuf>) {
    let dir = base.join(rel);
    if dir.join("HEAD").is_file() && dir.join("config").is_file() {
        out.push(rel.to_path_buf());
        return;
    }
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.path().is_dir() {
            find_gitdirs(base, &rel.join(entry.file_name()), out);
        }
    }
}
