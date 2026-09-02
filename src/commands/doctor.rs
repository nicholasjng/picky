//! `picky doctor`: sanity-check the repo's submodule state against
//! `.gitmodules`. Dangling gitlinks, sparse patterns that drifted from the
//! declaration, orphaned submodule git dirs left behind by hand-editing
//! `.gitmodules` instead of `add`/`remove`, and half-edited `.gitmodules`
//! sections. Diagnostic by default; reports issues as warnings and exits 0
//! regardless. `--strict` exits 1 when issues are found, for a pre-commit
//! hook or CI check.

use anyhow::{Result, bail};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::config;
use crate::console::Console;
use crate::{git, sparse};

pub fn run(root: &Path, strict: bool, con: &Console) -> Result<()> {
    let cfg = config::Config::load(root)?;
    let declared = cfg.all()?;
    let declared_paths: HashSet<&str> = declared.iter().map(|sm| sm.path.as_str()).collect();
    let mut issues = 0usize;

    for sm in &declared {
        if let Err(e) = config::check_path(&sm.path) {
            con.warn(format!(
                "{e}; picky refuses to check out or delete this path"
            ));
            issues += 1;
            continue;
        }
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
                // The checks below read the submodule's own config, which a
                // broken gitlink makes meaningless.
                continue;
            }
            if let Some(drift) = sparse::sparse_drift(root, sm) {
                con.warn(format!(
                    "{}: {drift}; `picky init {}` will reconcile it",
                    sm.path, sm.path
                ));
                issues += 1;
            }
        }
    }

    // Ask git rather than assuming `.git/modules`: the two disagree under a
    // linked worktree or an explicit `GIT_DIR`.
    let modules = sparse::modules_dir(root).unwrap_or_else(|_| root.join(".git/modules"));
    let mut gitdirs = Vec::new();
    find_gitdirs(&modules, Path::new(""), &mut gitdirs);
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

    let sub_names: HashSet<String> = cfg.section_names("submodule").into_iter().collect();
    let picky_names: HashSet<String> = cfg.section_names("picky").into_iter().collect();
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
        // `file_type`, not `entry.path().is_dir()`: the latter follows
        // symlinks, so a link back up the tree sends this recursing through
        // itself, reporting phantom `a/loop/a/loop/…` orphans until the
        // platform's symlink limit stops it.
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            find_gitdirs(base, &rel.join(entry.file_name()), out);
        }
    }
}
