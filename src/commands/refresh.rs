//! `picky refresh [<path>…]`: refresh the cached ref list (used by `<ref>`
//! completion and `status`'s upstream column) from each submodule's remote.
//! Also invoked detached as a background warm-up when completion serves a
//! stale cache.

use anyhow::Result;
use std::path::Path;

use crate::config::{self, Submodule};
use crate::console::Console;
use crate::{parallel, refcache, sparse};

pub fn run(root: &Path, paths: &[String], con: &Console) -> Result<()> {
    let cfg = config::Config::load(root)?;
    let subs: Vec<Submodule> = if paths.is_empty() {
        cfg.all()?
    } else {
        paths.iter().map(|p| cfg.find(p)).collect::<Result<_>>()?
    };

    if subs.is_empty() {
        con.warn("no submodules declared in .gitmodules");
        return Ok(());
    }

    con.step(format!("Fetching refs for {} submodule(s)", subs.len()));

    // One `ls-remote` per remote, run concurrently: these are pure network
    // waits. Results come back in declaration order, so the report below is
    // deterministic.
    let results = parallel::map(&subs, |sm| {
        let url = sparse::origin_url(root, sm).unwrap_or_else(|_| sm.url.clone());
        let refs = refcache::ls_remote(&url, Some(refcache::REFRESH_TIMEOUT));
        (url, refs)
    });

    for (sm, (url, refs)) in subs.iter().zip(results) {
        match refs {
            Some(refs) => {
                // Report a failed cache write without abandoning the rest.
                match refcache::write(&url, &refs) {
                    Ok(()) => con.success(format!("{}: cached {} ref(s)", sm.path, refs.len())),
                    Err(e) => con.warn(format!("{}: caching refs failed: {e}", sm.path)),
                }
            }
            None => con.warn(format!("{}: could not reach {url}", sm.path)),
        }
    }
    Ok(())
}
