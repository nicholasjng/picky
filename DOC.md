# picky: full documentation

Everything beyond the one-page overview in [README.md](README.md): what picky
does under the hood, how it is configured, and how to drive it from Rust.

Looking for "how do I do X?" rather than "how does X work?" — see
[QRH.md](QRH.md), the task-oriented quick reference.

- [Motivating examples](#motivating-examples)
- [Quick start](#quick-start)
- [How it works](#how-it-works)
- [Config schema (`.gitmodules`)](#config-schema-gitmodules)
- [Editing sparse patterns after init](#editing-sparse-patterns-after-init)
- [Patch stack](#patch-stack)
- [Post-update hook](#post-update-hook)
- [Command reference](#command-reference)
- [Shell completions](#shell-completions)
- [Use as a library](#use-as-a-library)
- [Local development](#local-development)

## Motivating examples

picky generalizes the hand-written `sh` setup scripts that several of my
projects had each reinvented:

- **MLIR out of LLVM.** Building MLIR needs LLVM's core, `cmake/`, `third-party/`
  and all of `mlir/`, but not clang, lldb, flang, the runtimes, or LLVM's
  ~1.2 GB `test/` tree. A sparse, blobless, depth-1 checkout of
  `llvm/llvm-project` lands the working tree at **~310 MB instead of ~8 GB**.
- **DuckDB with local patches.** A sparse checkout of `duckdb/duckdb` trims the
  working tree from **~280 MB to ~50 MB**, and a committed `patches/` stack is
  reapplied on top of each pinned upstream commit with `git apply --3way`.

Both started as a bespoke script per repo; picky replaces them with one binary
driven by a `.gitmodules` entry.

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
A path that would escape the superproject (absolute, or containing `..`) is
refused up front, before anything is fetched or created.

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
   sparse-checkout **before** the first checkout, so unused paths are never
   written and their blobs are never downloaded;
3. fetches the pinned commit shallow + blobless if it isn't present;
4. checks it out, then reconciles the working tree against the declared
   patterns.

Sparse state is driven through the `git sparse-checkout` porcelain
(`set --no-cone`, or `disable` when no patterns are declared) rather than by
writing `core.sparseCheckout` and `info/sparse-checkout` directly. That keeps
picky consistent with a submodule you have also poked at with stock git, and it
is what lets a cleared pattern list actually restore the full checkout: the
index's `skip-worktree` bits have to be cleared, which flipping the config key
alone does not do.

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
| `submodule.<name>.url`     | git   | remote URL, absolute or `../`-relative |
| `submodule.<name>.branch`  | git   | branch to track (used by `update <branch>`) |
| `submodule.<name>.shallow` | git   | `true` ⇒ shallow fetch (depth 1 unless `depth` set) |
| `picky.<name>.sparse`      | picky | **multivalued** non-cone pattern; absence ⇒ full checkout |
| `picky.<name>.depth`       | picky | explicit shallow `--depth N` (at least 1) |
| `picky.<name>.filter`      | picky | partial-clone filter; default `blob:none`, `none` disables |
| `picky.<name>.patches`     | picky | directory of the `*.patch` overlay stack |
| `picky.<name>.postUpdate`  | picky | shell command run after each checkout (post-update hook) |

A relative `url` (`../sibling.git`) is resolved against the *superproject's*
remote, the way git resolves one — not against the submodule directory. picky
takes that resolution from git rather than reimplementing its rules, so it
needs a remote on the superproject, exactly as `git submodule init` does.

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

## Editing sparse patterns after init

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

### Bulk input from a file or stdin

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
upstream commit). Skip the stack with `picky update --no-patches`.
`picky init` checks out pristine upstream **without** patches; run `picky update`
(no ref) afterwards to apply them at the current pin.

A failing patch is fatal, and the error carries git's own reason for each
failure, which is what distinguishes the two cases: a patch whose context has
drifted from upstream, versus one whose target file no longer exists at all.
Where `--3way` hit drifted context it leaves conflict markers in the tree;
resolve them and regenerate the patch (`git -C <path> diff HEAD > patches/…`),
or retire it.

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

## Command reference

The table in [README.md](README.md#commands) lists every command; this is the
behaviour worth knowing beyond `--help`.

### `update`

`update` accepts its two positionals smartly: with one argument, a value that
matches a submodule path is treated as the path (refresh onto the pin);
otherwise it's treated as a ref against the lone submodule. `--all` refreshes
every declared submodule instead, and can't be combined with a path or ref.

A refresh — `update <path>` with no ref, or `update --all` — targets the
superproject's **gitlink pin**, not whatever the submodule currently has
checked out; the two differ after you pull a commit that moved the pin. It
fetches the pinned commit if this shallow checkout has never seen it, moves
the working tree onto it, and re-applies the patch stack. The result line is
`moved onto its pin` or `already on its pin`. Nothing is staged: a refresh
brings the checkout *to* the index, not the other way round.

A bump fetches **only the target ref**, shallow + blobless (like `add`), so the
object store stays small and no history is downloaded. Pass `--unshallow` to
instead fetch the full history and all tags (needed for `git describe`); note
that on a large repo this fattens the (still blobless) object store with every
tree and commit, which is rarely what you want. If a default bump's fetch
fails, that's often a server refusing to hand over an unadvertised commit SHA
directly; the error suggests retrying with `--unshallow`.

`--fresh` throws the submodule's git dir away and rebuilds it at the target
instead of fetching on top of what's already there. `git gc` can never reclaim
a partial clone's old packs, so this is the only way to bound the object store
to one fetch's worth however many prior bumps happened.

Because `checkout -f` discards the working tree, `update` warns when it finds
changes it can't account for. Applying the patch stack is itself what makes the
tree dirty, so the warning is not "dirty" but "changed since picky last left
it": a submodule whose patches reapply identically stays quiet, and a hand-edit
you never exported to a patch is reported, including one made inside a file the
patch stack already touches.

### `status`

`status`'s `UPSTREAM` column compares the pin against the tracked branch's
cached remote tip (`current`/`stale→<sha>`), or `-` when no branch is tracked
(a bare SHA/tag pin has no "latest" to compare against). It's cache-only,
`status` never hits the network, so run `picky refresh` first to populate or
update it; a `?` means there's no cache yet. `picky status --json` emits the
same fields as a JSON array (`[]` when there are no submodules) instead of the
table, for scripting (hand-rolled, no `serde_json` dependency).

### `sparse`

`picky sparse list` (no `-p`) reports every declared submodule, like
`status`/`init`/`refresh`'s "no path ⇒ all" convention. The mutating actions
(`add`/`remove`/`set`/`clear`) still require `-p`/an unambiguous single
submodule, since applying them to every submodule at once usually isn't what
you want.

### `doctor`

`picky doctor` looks for state that usually comes from hand-editing
`.gitmodules` instead of using `add`/`remove`/`sparse`: a dangling gitlink
(worktree `.git` pointing at a missing git dir, self-heals on the next
`init`), an orphaned `.git/modules/<path>` with no matching declaration, a
`picky.<name>` section with no matching `submodule.<name>` section, or a
declared path that escapes the superproject. It only warns and exits 0 by
default; pass `--strict` to exit 1 when issues are found, for a pre-commit
hook or CI check.

### Preflight

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
    /*all*/ false, /*fresh*/ false, &Console::silent(),
)?;
```

To read `.gitmodules` once and answer many queries from it (rather than paying
a `git config` spawn per lookup), use `picky::config::Config`:

```rust
let cfg = picky::config::Config::load(&root)?;
let all = cfg.all()?;                             // every declared submodule
let one = cfg.find("ext/duckdb")?;                // by section name or path
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
cargo test                  # unit + integration tests
cargo clippy --all-targets
cargo fmt
```

The integration tests (`tests/integration.rs`) spin up a local `file://`
blobless remote and a superproject fixture, asserting sparse-only
materialization, shallow + blobless clones, idempotent re-init, pin bumps,
sparse widening/narrowing/clearing, patch apply and fatal broken patches, hook
trust, and `doctor`'s diagnostics. Unit tests cover the `.gitmodules` parser,
path validation, and git version parsing.
