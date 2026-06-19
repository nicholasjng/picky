mod commands;
mod config;
mod console;
mod git;
mod hook;
mod patch;
mod refcache;
mod sparse;

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{ArgValueCandidates, CompleteEnv, CompletionCandidate, Shell};

use config::Submodule;
use console::Console;

/// Runtime completion candidates: the submodule paths declared in the current
/// repo's `.gitmodules`. Errors (not in a repo, no `.gitmodules`) yield no
/// candidates rather than failing the shell completion.
fn submodule_candidates() -> Vec<CompletionCandidate> {
    let Ok(root) = git::repo_root() else {
        return Vec::new();
    };
    config::load_all(&root)
        .unwrap_or_default()
        .into_iter()
        .map(|sm| CompletionCandidate::new(sm.path).help(Some(sm.url.into())))
        .collect()
}

/// Ref-name candidates for a submodule, hybrid-cached (see [`refcache`]):
/// fresh cache instantly; stale cache instantly + a detached background
/// refresh; no cache ⇒ one 2s-bounded `ls-remote`, decaying to local refs.
fn refs_for(root: &Path, sm: &Submodule) -> Vec<String> {
    let mut refs = match refcache::read(&sm.url) {
        Some(c) if c.fresh => c.refs,
        Some(c) => {
            spawn_bg_refresh(&sm.path);
            c.refs
        }
        None => match refcache::ls_remote(&sm.url, Some(Duration::from_secs(2))) {
            Some(refs) => {
                let _ = refcache::write(&sm.url, &refs);
                refs
            }
            None => refcache::local_refs(root, &sm.path),
        },
    };
    refs.sort();
    refs.dedup();
    refs
}

/// Spawn a detached `picky refresh <path>` to warm a stale cache. `COMPLETE` is
/// cleared so the child runs the command instead of re-entering completion mode.
fn spawn_bg_refresh(path: &str) {
    use std::process::{Command, Stdio};
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = Command::new(exe)
        .args(["refresh", path])
        .env_remove("COMPLETE")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// The submodule whose refs to complete for `picky update`: the first declared
/// submodule named on the command line, else the lone submodule.
fn update_submodule(root: &Path) -> Option<Submodule> {
    let args: Vec<String> = std::env::args().collect();
    let after = args.iter().position(|a| a == "--").map(|i| &args[i + 1..]);
    let from_cmdline = after.and_then(|words| {
        let pos = words.iter().position(|w| w == "update")?;
        words[pos + 1..]
            .iter()
            .filter(|w| !w.starts_with('-'))
            .find_map(|w| config::find(root, w).ok())
    });
    from_cmdline.or_else(|| config::only(root).ok())
}

/// Completion for `update`'s ref positional: the resolved submodule's refs.
fn update_ref_candidates() -> Vec<CompletionCandidate> {
    let Ok(root) = git::repo_root() else {
        return Vec::new();
    };
    let Some(sm) = update_submodule(&root) else {
        return Vec::new();
    };
    refs_for(&root, &sm)
        .into_iter()
        .map(CompletionCandidate::new)
        .collect()
}

/// Completion for `update`'s first positional: submodule paths, plus — when
/// there is exactly one submodule — its refs (the `update <ref>` shorthand).
fn update_target_candidates() -> Vec<CompletionCandidate> {
    let Ok(root) = git::repo_root() else {
        return Vec::new();
    };
    let mut out: Vec<CompletionCandidate> = config::load_all(&root)
        .unwrap_or_default()
        .into_iter()
        .map(|sm| CompletionCandidate::new(sm.path).help(Some(sm.url.into())))
        .collect();
    if let Ok(sm) = config::only(&root) {
        out.extend(
            refs_for(&root, &sm)
                .into_iter()
                .map(CompletionCandidate::new),
        );
    }
    out
}

#[derive(Parser)]
#[command(
    name = "picky",
    version,
    about = "Lightweight sparse-checkout client for git submodules"
)]
pub struct Cli {
    /// Suppress progress output
    #[arg(long, short, global = true)]
    quiet: bool,
    /// Print extra detail
    #[arg(long, short, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Add a submodule and check it out sparsely
    Add {
        /// Remote URL
        url: String,
        /// Path within the superproject
        path: String,
        /// Non-cone sparse-checkout pattern (repeatable); omit for a full checkout
        #[arg(long)]
        sparse: Vec<String>,
        /// Shallow fetch depth
        #[arg(long)]
        depth: Option<u32>,
        /// Partial-clone filter (e.g. blob:none, or none to disable)
        #[arg(long)]
        filter: Option<String>,
        /// Track this branch
        #[arg(long)]
        branch: Option<String>,
        /// Pin to this ref (SHA/tag/branch) instead of the remote HEAD
        #[arg(long = "ref")]
        reference: Option<String>,
        /// Directory holding the patch stack
        #[arg(long)]
        patches: Option<String>,
        /// Shell command to run after each checkout (post-update hook)
        #[arg(long = "post-update")]
        post_update: Option<String>,
        /// Section name in .gitmodules (defaults to <path>)
        #[arg(long)]
        name: Option<String>,
    },
    /// Reconstruct sparse checkouts from .gitmodules (no args ⇒ all submodules)
    Init {
        /// Submodule paths to initialize
        #[arg(add = ArgValueCandidates::new(submodule_candidates))]
        paths: Vec<String>,
    },
    /// Bump a submodule pin / re-checkout / re-apply the patch stack
    Update {
        /// Submodule path, or a ref when only one submodule exists
        #[arg(add = ArgValueCandidates::new(update_target_candidates))]
        target: Option<String>,
        /// Ref to bump to (SHA/tag/branch)
        #[arg(add = ArgValueCandidates::new(update_ref_candidates))]
        reference: Option<String>,
        /// Skip the patch stack
        #[arg(long)]
        no_patches: bool,
        /// Override the shallow fetch depth
        #[arg(long)]
        depth: Option<u32>,
    },
    /// Edit a submodule's sparse-checkout patterns and reconcile the checkout
    Sparse {
        #[command(subcommand)]
        action: SparseAction,
    },
    /// Show a submodule status table
    Status {
        /// Submodule paths to report (no args ⇒ all)
        #[arg(add = ArgValueCandidates::new(submodule_candidates))]
        paths: Vec<String>,
    },
    /// Refresh the cached remote ref list used by `<ref>` completion
    Refresh {
        /// Submodule paths to refresh (no args ⇒ all)
        #[arg(add = ArgValueCandidates::new(submodule_candidates))]
        paths: Vec<String>,
    },
    /// Generate shell completions
    Completions {
        /// Target shell
        shell: Shell,
    },
}

#[derive(Subcommand)]
enum SparseAction {
    /// List the current sparse patterns
    List {
        /// Submodule path (optional when only one submodule exists)
        #[arg(short, long, add = ArgValueCandidates::new(submodule_candidates))]
        path: Option<String>,
    },
    /// Add one or more patterns and reconcile the checkout
    Add {
        /// Patterns to add
        #[arg(required = true)]
        patterns: Vec<String>,
        /// Submodule path (optional when only one submodule exists)
        #[arg(short, long, add = ArgValueCandidates::new(submodule_candidates))]
        path: Option<String>,
        /// Edit config only; don't re-run init
        #[arg(long)]
        no_reinit: bool,
    },
    /// Remove one or more patterns (exact match) and reconcile the checkout
    Remove {
        /// Patterns to remove
        #[arg(required = true)]
        patterns: Vec<String>,
        /// Submodule path (optional when only one submodule exists)
        #[arg(short, long, add = ArgValueCandidates::new(submodule_candidates))]
        path: Option<String>,
        /// Edit config only; don't re-run init
        #[arg(long)]
        no_reinit: bool,
    },
    /// Remove all patterns (⇒ full checkout) and reconcile the checkout
    Clear {
        /// Submodule path (optional when only one submodule exists)
        #[arg(short, long, add = ArgValueCandidates::new(submodule_candidates))]
        path: Option<String>,
        /// Edit config only; don't re-run init
        #[arg(long)]
        no_reinit: bool,
    },
}

fn main() {
    // Handle dynamic shell-completion requests (when COMPLETE=<shell> is set)
    // before any normal parsing or output; exits the process if it ran.
    CompleteEnv::with_factory(Cli::command).complete();

    let cli = Cli::parse();
    let con = Console::new(cli.quiet, cli.verbose);
    if let Err(err) = run(cli.command, &con) {
        con.error(format!("{err:#}"));
        std::process::exit(1);
    }
}

fn run(command: Commands, con: &Console) -> Result<()> {
    // Completions need neither a repo nor git.
    if let Commands::Completions { shell } = command {
        return commands::completions::run(shell);
    }

    let root = git::repo_root()?;

    match command {
        Commands::Add {
            url,
            path,
            sparse,
            depth,
            filter,
            branch,
            reference,
            patches,
            post_update,
            name,
        } => commands::add::run(
            &root,
            url,
            path,
            name,
            sparse,
            depth,
            filter,
            branch,
            reference,
            patches,
            post_update,
            con,
        ),
        Commands::Init { paths } => commands::init::run(&root, &paths, con),
        Commands::Update {
            target,
            reference,
            no_patches,
            depth,
        } => commands::update::run(&root, target, reference, no_patches, depth, con),
        Commands::Sparse { action } => {
            use commands::sparse::Action;
            let (path, op, no_reinit) = match action {
                SparseAction::List { path } => (path, Action::List, false),
                SparseAction::Add {
                    patterns,
                    path,
                    no_reinit,
                } => (path, Action::Add(patterns), no_reinit),
                SparseAction::Remove {
                    patterns,
                    path,
                    no_reinit,
                } => (path, Action::Remove(patterns), no_reinit),
                SparseAction::Clear { path, no_reinit } => (path, Action::Clear, no_reinit),
            };
            commands::sparse::run(&root, path, op, no_reinit, con)
        }
        Commands::Status { paths } => commands::status::run(&root, &paths, con),
        Commands::Refresh { paths } => commands::refresh::run(&root, &paths, con),
        Commands::Completions { .. } => unreachable!("handled above"),
    }
}
