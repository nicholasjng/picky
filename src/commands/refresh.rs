//! `picky refresh [<path>…]`: refresh the cached ref list (used by `<ref>`
//! completion) from each submodule's remote. Also invoked detached as a
//! background warm-up when completion serves a stale cache.

use anyhow::Result;
use std::path::Path;

use crate::config::{self, Submodule};
use crate::console::Console;
use crate::refcache;

pub fn run(root: &Path, paths: &[String], con: &Console) -> Result<()> {
    let cfg = config::Config::load(root)?;
    let subs: Vec<Submodule> = if paths.is_empty() {
        cfg.all()?
    } else {
        paths.iter().map(|p| cfg.find(p)).collect::<Result<_>>()?
    };

    for sm in &subs {
        con.step(format!("Fetching refs for {} from {}", sm.path, sm.url));
        match refcache::ls_remote(&sm.url, None) {
            Some(refs) => {
                refcache::write(&sm.url, &refs)?;
                con.success(format!("{}: cached {} ref(s)", sm.path, refs.len()));
            }
            None => con.warn(format!("{}: could not reach {}", sm.path, sm.url)),
        }
    }
    Ok(())
}
