//! `picky gc [<path>…]`: explicitly prune each submodule's object store
//! (`git gc --prune=now`). Deterministic reclaiming for repos updated often
//! enough that shallow-graft churn and partial-clone objects pile up between
//! runs (e.g. an LLVM-sized submodule bumped daily). See also `PICKY_AUTO_GC`
//! (`src/sparse.rs`) for an opt-in, threshold-gated `gc --auto` after every
//! fetch instead of waiting for an explicit `picky gc`.

use anyhow::Result;
use std::path::Path;

use crate::config::{self, Submodule};
use crate::console::Console;
use crate::git;

pub fn run(root: &Path, paths: &[String], con: &Console) -> Result<()> {
    let subs: Vec<Submodule> = if paths.is_empty() {
        config::load_all(root)?
    } else {
        paths
            .iter()
            .map(|p| config::find(root, p))
            .collect::<Result<_>>()?
    };

    for sm in &subs {
        let wt = root.join(&sm.path);
        con.step(format!("Pruning {}", sm.path));
        git::run(&wt, &["gc", "--prune=now"])?;
        con.success(format!("{}: gc'd", sm.path));
    }
    Ok(())
}
