//! picky as a library — the sparse-checkout engine behind the `picky` CLI,
//! exposed for embedding (e.g. a Tauri backend). Everything shells out to the
//! `git` CLI, which must be on `PATH` at runtime.
//!
//! # Embedding
//!
//! The high-level operations live in [`commands`]; the convenience wrappers
//! below ([`init`], [`update`], [`set_sparse`], …) call into them. Each takes a
//! [`Console`] for progress output — pass [`quiet`] when you don't want any.
//!
//! Two things to know for a GUI integration:
//!
//! - Output currently goes to the process's stdout/stderr (via `anstream`), not
//!   a return value. To render progress in-app, make [`console`] write to a
//!   pluggable sink (a callback/channel) instead of `anstream::println!`.
//! - [`Submodule`] is plain data; derive `serde::Serialize` for it (behind a
//!   feature) to hand it straight to a Tauri command's frontend.
//!
//! ```no_run
//! use std::path::Path;
//!
//! let root = picky::repo_root()?;
//! for sm in picky::submodules(&root)? {
//!     println!("{} @ {}", sm.path, sm.url);
//! }
//! picky::init(&root, &[], &picky::quiet())?;                 // reconstruct all
//! picky::update(                                             // bump one
//!     &root, Some("ext/duckdb".into()), Some("v1.6.3".into()),
//!     false, false, None, &picky::quiet(),
//! )?;
//! # Ok::<(), anyhow::Error>(())
//! ```

pub mod commands;
pub mod config;
pub mod console;
pub mod git;
pub mod hook;
pub mod patch;
pub mod refcache;
pub mod sparse;

pub use config::Submodule;
pub use console::Console;

use anyhow::Result;
use std::path::{Path, PathBuf};

/// A [`Console`] that suppresses progress output — the usual choice for an
/// embedder that doesn't want CLI chatter on stdout.
pub fn quiet() -> Console {
    Console::new(true, false)
}

/// The superproject root (`git rev-parse --show-toplevel` from the cwd).
pub fn repo_root() -> Result<PathBuf> {
    git::repo_root()
}

/// Every submodule declared in `<root>/.gitmodules`.
pub fn submodules(root: &Path) -> Result<Vec<Submodule>> {
    config::load_all(root)
}

/// Reconstruct sparse checkouts from committed config; empty `paths` ⇒ all.
pub fn init(root: &Path, paths: &[String], con: &Console) -> Result<()> {
    commands::init::run(root, paths, con)
}

/// Bump the pin / re-checkout / re-apply the patch stack. See
/// [`commands::update::run`] for the positional and flag semantics.
pub fn update(
    root: &Path,
    target: Option<String>,
    reference: Option<String>,
    no_patches: bool,
    unshallow: bool,
    depth: Option<u32>,
    con: &Console,
) -> Result<()> {
    commands::update::run(root, target, reference, no_patches, unshallow, depth, con)
}

/// Replace a submodule's sparse pattern list and reconcile the checkout. `path`
/// may be `None` when exactly one submodule is declared.
pub fn set_sparse(
    root: &Path,
    path: Option<String>,
    patterns: Vec<String>,
    con: &Console,
) -> Result<()> {
    commands::sparse::run(
        root,
        path,
        commands::sparse::Action::Set(patterns),
        false,
        con,
    )
}
