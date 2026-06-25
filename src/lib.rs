//! picky as a library: the sparse-checkout engine behind the `picky` CLI,
//! exposed for embedding (e.g. a Tauri backend). Everything shells out to the
//! `git` CLI, which must be on `PATH` at runtime.
//!
//! # Embedding
//!
//! The high-level operations live in [`commands`]; the convenience wrappers
//! below ([`init`], [`update`], [`set_sparse`], …) call into them. Each takes a
//! [`Console`] for progress output; pass [`Console::silent`] when you don't
//! want any.
//!
//! Two things for a GUI integration:
//!
//! - To render progress in-app instead of on stdout/stderr, build the console
//!   with [`Console::with_sink`] and pass a `Fn(Level, &str)` (or a [`Sink`])
//!   that forwards each message to a channel or a Tauri event.
//! - Enable the `serde` feature for `Serialize`/`Deserialize` on [`Submodule`]
//!   and [`Level`], so they cross a Tauri command boundary directly.
//!
//! ```no_run
//! use picky::Console;
//!
//! let root = picky::repo_root()?;
//! for sm in picky::submodules(&root)? {
//!     println!("{} @ {}", sm.path, sm.url);
//! }
//! picky::init(&root, &[], &Console::silent())?;              // reconstruct all
//! picky::update(                                             // bump one
//!     &root, Some("ext/duckdb".into()), Some("v1.6.3".into()),
//!     false, false, None, false, &Console::silent(),
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
pub use console::{Console, Level, Sink};

use anyhow::Result;
use std::path::{Path, PathBuf};

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

/// Undeclare submodule(s) and delete their checkouts: the inverse of
/// [`commands::add::run`]. `paths` must be non-empty; there is no "remove all".
/// `yes` skips the interactive confirmation prompt (required when not
/// running attached to a terminal).
pub fn remove(root: &Path, paths: &[String], yes: bool, con: &Console) -> Result<()> {
    commands::remove::run(root, paths, yes, con)
}

/// Bump the pin / re-checkout / re-apply the patch stack. See
/// [`commands::update::run`] for the positional and flag semantics. `all`
/// refreshes every declared submodule at its current pin instead (mutually
/// exclusive with `target`/`reference`).
#[allow(clippy::too_many_arguments)]
pub fn update(
    root: &Path,
    target: Option<String>,
    reference: Option<String>,
    no_patches: bool,
    unshallow: bool,
    depth: Option<u32>,
    all: bool,
    con: &Console,
) -> Result<()> {
    commands::update::run(
        root, target, reference, no_patches, unshallow, depth, all, con,
    )
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

#[cfg(all(test, feature = "serde"))]
mod serde_assertions {
    fn is_serde<T: serde::Serialize + serde::de::DeserializeOwned>() {}

    #[test]
    fn public_types_are_serde() {
        is_serde::<crate::Submodule>();
        is_serde::<crate::Level>();
    }
}
