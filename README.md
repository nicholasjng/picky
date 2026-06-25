# picky

A lightweight tool for **sparse, shallow, blobless git submodule checkouts**.

Projects often vendor a large dependency as a submodule but need only a fraction
of its tree, its history, or its file contents. `picky` fetches only the history
you ask for (`--depth`), only the objects you ask for (`--filter=blob:none`), and
writes only the paths you ask for (non-cone sparse-checkout), driven entirely by
declarative config committed to `.gitmodules`, so any checkout is reproducible
from a single command. For the cases where a pristine checkout isn't quite
enough, it also carries an optional working-tree patch stack and a post-update
hook.

`picky` shells out to the `git` CLI; it is a single binary with no libgit2 dependency.

## Motivating examples

picky generalizes the hand-written `sh` setup scripts that several of my projects had each reinvented:

- **MLIR out of LLVM.** Building MLIR needs LLVM's core, `cmake/`, `third-party/`
  and all of `mlir/`, but not clang, lldb, flang, the runtimes, or LLVM's
  ~1.2 GB `test/` tree. A sparse, blobless, depth-1 checkout of
  `llvm/llvm-project` lands the working tree at **~310 MB instead of ~8 GB**.
- **DuckDB with local patches.** A sparse checkout of `duckdb/duckdb` trims the
  working tree from **~280 MB to ~50 MB**, and a committed `patches/` stack is
  reapplied on top of each pinned upstream commit with `git apply --3way`.

Both started as a bespoke script per repo; picky replaces them with one binary driven by a `.gitmodules` entry.

## Install

```sh
# straight from git, no checkout needed
cargo install --git https://github.com/nicholasjng/picky

# …or from a local clone of this repo
cargo install --path .
```

Make sure `~/.cargo/bin` is on your `PATH`. Requires a recent `git` (≥ 2.41 for `GIT_NO_LAZY_FETCH`) on `PATH` at runtime, plus a Unix-like shell environment (`sh` for the post-update hook, `du` for working-tree sizing in `status`/`add`): Linux, macOS, WSL, or Git Bash. Native Windows (cmd/PowerShell without WSL) is not supported.

## Quick start

In an existing superproject whose `.gitmodules` declares a submodule:

```sh
picky init                      # reconstruct every declared submodule
picky init ext/duckdb           # …or just one
picky status                    # show pins, sparse state, sizes, patch counts
picky update ext/duckdb v1.6.3  # bump the pin, re-checkout, re-apply patches
picky update --all              # refresh every submodule at its current pin (no bump)
picky doctor                    # sanity-check .gitmodules vs. what's actually on disk
```

Or add a new sparse submodule from scratch:

```sh
picky add https://github.com/duckdb/duckdb.git ext/duckdb \
    --branch main \
    --sparse /src/ --sparse /third_party/ --sparse /extension/parquet/ \
    --patches patches
```

`add` builds the checkout first; only once it succeeds does it write `.gitmodules`
and stage both it and the new gitlink, commit them to record the submodule. A
failed `add` (bad URL, bad `--ref`, network) leaves no trace in `.gitmodules`.

Removing a submodule is the inverse:

```sh
picky remove ext/duckdb
```

This deletes the working tree and the submodule's git dir, drops the gitlink
from the index, and strips its `submodule.<name>`/`picky.<name>` sections from
`.gitmodules`, staging all of it for you to commit. There's no bare `picky
remove` with no args; paths are always explicit. It asks for confirmation
before deleting anything (`y`/`N`); pass `--yes`/`-y` to skip the prompt for
scripted use, required when not running attached to a terminal.

## How it works

For each submodule, `picky`:

1. builds the submodule git dir under `.git/modules/<path>` (where `git
   submodule` expects it);
2. configures a partial clone (`extensions.partialClone`,
   `remote.origin.promisor`, `partialclonefilter=blob:none`) and non-cone
   sparse-checkout (`core.sparseCheckout=true`, `core.sparseCheckoutCone=false`)
   **before** the first checkout, so unused paths are never written and their
   blobs are never downloaded;
3. fetches the pinned commit shallow + blobless if it isn't present;
4. checks it out and runs `sparse-checkout reapply`.

Blobs for in-sparse files are lazy-fetched from the promisor on checkout;
out-of-sparse blobs are never fetched. Every command is idempotent: a fresh
clone, a half-finished run, or an existing full checkout all converge to the
same state.

## Config schema (`.gitmodules`)

`picky` reads and writes `.gitmodules` via `git config -f`, splitting keys
across two sections joined by the shared subsection name. Git's **standard**
keys stay in the `submodule.<name>` section it understands; picky's **own**
options live in a parallel `picky.<name>` section that stock git ignores entirely.

| Key | Section | Meaning |
|---|---|---|
| `submodule.<name>.path`    | git   | checkout path within the superproject |
| `submodule.<name>.url`     | git   | remote URL |
| `submodule.<name>.branch`  | git   | branch to track (used by `update <branch>`) |
| `submodule.<name>.shallow` | git   | `true` ⇒ shallow fetch (depth 1 unless `depth` set) |
| `picky.<name>.sparse`      | picky | **multivalued** non-cone pattern; absence ⇒ full checkout |
| `picky.<name>.depth`       | picky | explicit shallow `--depth N` |
| `picky.<name>.filter`      | picky | partial-clone filter; default `blob:none`, `none` disables |
| `picky.<name>.patches`     | picky | directory of the `*.patch` overlay stack |
| `picky.<name>.postUpdate`  | picky | shell command run after each checkout (post-update hook) |

The `picky.<name>` section holds **only** the options git doesn't understand;
nothing is duplicated from the `submodule.<name>` section. The pin (gitlink SHA)
lives where git already keeps it, the superproject tree (`git ls-files -s
<path>`); no extra config file is introduced.

Example:

```ini
[submodule "ext/duckdb"]
	path = ext/duckdb
	url = https://github.com/duckdb/duckdb.git
	branch = main
	shallow = true
[picky "ext/duckdb"]
	sparse = /src/
	sparse = /third_party/
	sparse = /extension/parquet/
	patches = patches
	postUpdate = cmake -P cmake/OverrideGitDescribe.cmake
```

### Editing sparse patterns after init

Use `picky sparse` to edit the pattern list and reconcile the checkout in one
step (widening materializes new paths, narrowing trims removed ones). The
operation is a subcommand; the submodule is named with `-p/--path` (optional
when there's only one submodule):

```sh
picky sparse list -p ext/duckdb                          # show current patterns
picky sparse add /extension/json/ -p ext/duckdb          # add + reconcile
picky sparse remove /extension/icu/ -p ext/duckdb        # remove (exact match) + reconcile
picky sparse add /a/ /b/ -p ext/duckdb                   # add several at once
picky sparse set /src/ /third_party/ -p ext/duckdb       # replace the whole list
picky sparse clear -p ext/duckdb                         # drop all → full checkout
picky sparse add /x/ -p ext/duckdb --no-reinit           # edit .gitmodules only
```

It edits `.gitmodules`, stages it, then re-runs `init` for that submodule
(`--no-reinit` skips the reconcile). Removal is by **exact value**, so patterns
with metacharacters like `/extension/*.cmake` work without the `git config
--unset` regex footgun.

#### Bulk input from a file or stdin

`set`, `add`, and `remove` can read newline-delimited patterns from a file
(`--from <file>`) or stdin (`--stdin`) instead of (or in addition to) positional
arguments, handy for a long list. Blank lines and `#` comments are skipped, so
a pattern file doubles as documentation:

```sh
picky sparse set -p ext/duckdb --stdin <<'EOF'
# CMake glue
/CMakeLists.txt
/extension/*.cmake
# sources
/src/
/third_party/
EOF

# or keep the canonical list in a committed file
picky sparse set -p ext/duckdb --from sparse-paths.txt
```

`set` requires at least one source (use `clear` to empty the list) and drops
duplicate patterns.

Equivalent by hand, if you prefer raw git:

```sh
git config -f .gitmodules --add picky.ext/duckdb.sparse /extension/json/
picky init ext/duckdb
git add .gitmodules
```

## Patch stack

If a submodule sets `patches`, `picky update` applies every `<patches>/*.patch`
in lexical order with `git apply --3way` (a working-tree overlay over a pristine
upstream commit). A failing patch is fatal and leaves conflict markers in the
tree for you to resolve. Skip the stack with `picky update --no-patches`.
`picky init` checks out pristine upstream **without** patches; run `picky update`
(no ref) afterwards to apply them at the current pin.

## Post-update hook

A submodule may declare `picky.<name>.postUpdate`, a shell command run after
its working tree is (re)materialized by `add`, `init`, or `update` (in the latter
case, after the patch stack). It runs through `sh -c` **in the submodule's
working tree**, and a non-zero exit is fatal.

These environment variables are exported to the hook:

| Variable | Value |
|---|---|
| `PICKY_ROOT`           | absolute path of the superproject root |
| `PICKY_SUBMODULE_NAME` | the `submodule.<name>` section name |
| `PICKY_SUBMODULE_PATH` | the submodule's path within the superproject |
| `PICKY_SUBMODULE_SHA`  | the checked-out commit SHA |

Set it with `picky add … --post-update '<cmd>'`, or by hand:

```sh
git config -f .gitmodules picky.ext/duckdb.postUpdate \
    'cmake -P cmake/OverrideGitDescribe.cmake'
```

### Trust

`postUpdate` comes from `.gitmodules`, which is committed and travels with the
repo, so in a clone of someone else's repo it's attacker-controlled text. It's
never run unconditionally: the first time picky sees a given hook command for
a submodule, it prints the command and asks for interactive approval before
running it. Approval is recorded **locally** (`picky.<name>.trustedPostUpdate`
and `picky.<name>.trustedPostUpdateSha` in `.git/config`, never `.gitmodules`),
pinned to both the command text and the submodule's checked-out SHA. Either
changing invalidates the approval and triggers a re-prompt. The SHA is pinned
too because a hook that runs a script inside the submodule (e.g.
`sh scripts/hook.sh`) can keep an identical `postUpdate` string across a
commit bump while the script's contents change underneath it.

Running non-interactively (CI, scripts) with an unapproved hook fails with
guidance rather than silently skipping or silently running it. Either approve
it once interactively beforehand, trust it directly:

```sh
git config picky.ext/duckdb.trustedPostUpdate 'cmake -P cmake/OverrideGitDescribe.cmake'
git config picky.ext/duckdb.trustedPostUpdateSha '<submodule sha>'
```

or set `PICKY_TRUST_HOOKS=1` in the environment to auto-approve (and persist)
any hook it encounters, useful for CI that already trusts the checked-out
content.

Not covered: commands that fetch or compute at run time (`curl ... | sh`,
`$(...)`), and scripts outside `.gitmodules`'s reach (e.g. in the
superproject itself, no different from any other tracked file).

## Commands

| Command | Purpose |
|---|---|
| `picky add <url> <path> [opts]` | declare + check out a new sparse submodule |
| `picky remove <path>… [--yes]`  | undeclare a submodule and delete its checkout (the inverse of `add`) |
| `picky init [<path>…]`          | reconstruct checkout(s) from `.gitmodules` (no args ⇒ all) |
| `picky update [<path>] [<ref>]` | bump pin / re-checkout / re-apply patches |
| `picky update --all`            | refresh every declared submodule at its current pin (no bump) |
| `picky sparse <list/add/remove/set/clear>` | edit sparse patterns + reconcile |
| `picky status [<path>…] [--json]` | table (or JSON) of pin, branch, upstream staleness, sparse, filter, size, patches |
| `picky refresh [<path>…]`       | refresh the cached remote ref list (for `<ref>` completion and `status`'s upstream column) |
| `picky doctor [--strict]`       | sanity-check submodule state against `.gitmodules` (diagnostic by default) |
| `picky completions <shell>`     | print the eval-able dynamic-completion registration script |

Global flags: `-q/--quiet`, `-v/--verbose`. Color honors `NO_COLOR` and TTY
detection. Run `picky <command> --help` for the full option list.

`update` accepts its two positionals smartly: with one argument, a value that
matches a submodule path is treated as the path (refresh at current pin);
otherwise it's treated as a ref against the lone submodule. `--all` refreshes
every declared submodule at its current pin instead, and can't be combined
with a path or ref.

A bump fetches **only the target ref**, shallow + blobless (like `add`), so the
object store stays small and no history is downloaded. Pass `--unshallow` to
instead fetch the full history and all tags (needed for `git describe`); note
that on a large repo this fattens the (still blobless) object store with every
tree and commit, which is rarely what you want. If a default bump's fetch
fails, that's often a server refusing to hand over an unadvertised commit SHA
directly; the error suggests retrying with `--unshallow`.

`status`'s `UPSTREAM` column compares the pin against the tracked branch's
cached remote tip (`current`/`stale→<sha>`), or `-` when no branch is tracked
(a bare SHA/tag pin has no "latest" to compare against). It's cache-only,
`status` never hits the network, so run `picky refresh` first to populate or
update it; a `?` means there's no cache yet. `picky status --json` emits the
same fields as a JSON array (`[]` when there are no submodules) instead of the
table, for scripting (hand-rolled, no `serde_json` dependency).

`picky sparse list` (no `-p`) reports every declared submodule, like
`status`/`init`/`refresh`'s "no path ⇒ all" convention. The mutating actions
(`add`/`remove`/`set`/`clear`) still require `-p`/an unambiguous single
submodule, since applying them to every submodule at once usually isn't what
you want.

`picky doctor` looks for state that usually comes from hand-editing
`.gitmodules` instead of using `add`/`remove`/`sparse`: a dangling gitlink
(worktree `.git` pointing at a missing git dir, self-heals on the next
`init`), an orphaned `.git/modules/<path>` with no matching declaration, or a
`picky.<name>` section with no matching `submodule.<name>` section. It only
warns and exits 0 by default; pass `--strict` to exit 1 when issues are found,
for a pre-commit hook or CI check.

Every command checks the installed `git` up front and fails with a clear
message if it's missing or older than 2.41 (needed for `GIT_NO_LAZY_FETCH`),
rather than failing confusingly partway through.

## Shell completions

Add the line for your shell to its startup file:

```sh
# bash   (~/.bashrc)
source <(picky completions bash)
# zsh    (~/.zshrc)
eval "$(picky completions zsh)"
# fish   (~/.config/fish/config.fish)
picky completions fish | source
# elvish (~/.config/elvish/rc.elv)
eval (picky completions elvish | slurp)
```

Supported shells: `bash`, `zsh`, `fish`, `elvish`, `powershell`. `picky` must be
installed and on your `PATH` for completion to work.

You then get `<TAB>` completion on submodule paths (`init`, `update`, `remove`,
`sparse`, `status`, `refresh`) and on remote tags/branches for
`picky update <path> <TAB>`.

## Use as a library

picky is published as both a binary and a library, so you can drive the
sparse-checkout engine from Rust instead of shelling out to the CLI:

```toml
[dependencies]
picky = { git = "https://github.com/nicholasjng/picky" }
# add the `serde` feature for Serialize/Deserialize on `Submodule` and `Level`:
picky = { git = "https://github.com/nicholasjng/picky", features = ["serde"] }
```

```rust
use picky::Console;

let root = picky::repo_root()?;
for sm in picky::submodules(&root)? {            // -> Vec<picky::Submodule>
    println!("{} @ {}", sm.path, sm.url);
}
picky::init(&root, &[], &Console::silent())?;     // reconstruct every submodule
picky::update(                                    // bump one
    &root, Some("ext/duckdb".into()), Some("v1.6.3".into()),
    /*no_patches*/ false, /*unshallow*/ false, /*depth*/ None,
    /*all*/ false, &Console::silent(),
)?;
```

The high-level helpers (`init`, `update`, `set_sparse`, …) and the full module
surface (`picky::commands`, `picky::config`, `picky::sparse`, …) take a
`Console` for progress output. Pick one of:

- `Console::new(quiet, verbose)`: colored output to stdout/stderr (the CLI
  default).
- `Console::silent()`: discards everything (the `/dev/null` sink).
- `Console::with_sink(|level, msg| …)`: forward each message to your own sink
  (a channel, a Tauri event, a log) as a `(Level, &str)` pair:

```rust
use std::sync::mpsc;
let (tx, rx) = mpsc::channel::<(picky::Level, String)>();
let con = picky::Console::with_sink(move |level, msg: &str| {
    let _ = tx.send((level, msg.to_owned()));
});
// drive picky with `&con`, drain `rx` to render progress in your UI
```

`Console` is `Send + Sync`, so it can live in shared application state.
The `git` CLI must be on `PATH` at runtime, as for the binary.

## Local development

```sh
cargo build                 # debug binary -> target/debug/picky
cargo run -- status         # run without installing
cargo test                  # integration tests (tests/integration.rs)
cargo clippy --all-targets
cargo fmt
```

The integration tests spin up a local `file://` blobless remote and a
superproject fixture, asserting sparse-only materialization, shallow + blobless
clones, idempotent re-init, pin bumps, and patch apply / fatal broken patch.

## Roadmap

- Cone-mode sparse-checkout (picky defaults to non-cone, matching the reference
  scripts).
