# picky

A lightweight client for **sparse, shallow, blobless git submodule checkouts**.

Projects often vendor a large dependency as a submodule but need only a fraction
of its tree, its history, or its file contents. `picky` fetches only the history
you ask for (`--depth`), only the objects you ask for (`--filter=blob:none`), and
writes only the paths you ask for (non-cone sparse-checkout) — driven entirely by
declarative config committed to `.gitmodules`, so any checkout is reproducible
from a single command. For the cases where a pristine checkout isn't quite
enough, it also carries an optional working-tree patch stack and a post-update
hook.

`picky` shells out to the `git` CLI; it is a single binary with no libgit2
dependency.

## Motivating examples

picky generalizes the hand-written `sh` setup scripts that several projects had
each reinvented:

- **MLIR out of LLVM.** Building MLIR needs LLVM's core, `cmake/`, `third-party/`
  and all of `mlir/` — but not clang, lldb, flang, the runtimes, or LLVM's
  ~1.2 GB `test/` tree. A sparse, blobless, depth-1 checkout of
  `llvm/llvm-project` lands the working tree at **~310 MB instead of ~8 GB**.
- **DuckDB with local patches.** A sparse checkout of `duckdb/duckdb` trims the
  working tree from **~280 MB to ~50 MB**, and a committed `patches/` stack is
  reapplied on top of each pinned upstream commit with `git apply --3way`.

Both started as a bespoke script per repo; picky replaces them with one binary
driven by a `.gitmodules` entry.

## Install

```sh
# from a clone of this repo
cargo install --path .          # installs `picky` into ~/.cargo/bin
```

Make sure `~/.cargo/bin` is on your `PATH`. Requires a recent `git` (≥ 2.41 for
`GIT_NO_LAZY_FETCH`) on `PATH` at runtime.

## Quick start

In an existing superproject whose `.gitmodules` declares a submodule:

```sh
picky init                      # reconstruct every declared submodule
picky init ext/duckdb           # …or just one
picky status                    # show pins, sparse state, sizes, patch counts
picky update ext/duckdb v1.6.3  # bump the pin, re-checkout, re-apply patches
```

Or add a new sparse submodule from scratch:

```sh
picky add https://github.com/duckdb/duckdb.git ext/duckdb \
    --branch main \
    --sparse /src/ --sparse /third_party/ --sparse /extension/parquet/ \
    --patches patches
```

`add` writes the `.gitmodules` entry, builds the checkout, and stages both
`.gitmodules` and the new gitlink — commit them to record the submodule.

## How it works

For each submodule `picky`:

1. builds the submodule git dir under `.git/modules/<path>` (where `git
   submodule` expects it);
2. configures a partial clone (`extensions.partialClone`,
   `remote.origin.promisor`, `partialclonefilter=blob:none`) and non-cone
   sparse-checkout (`core.sparseCheckout=true`, `core.sparseCheckoutCone=false`)
   **before** the first checkout, so unused paths are never written and their
   blobs never downloaded;
3. fetches the pinned commit shallow + blobless if it isn't present;
4. checks it out and runs `sparse-checkout reapply`.

Blobs for in-sparse files are lazy-fetched from the promisor on checkout;
out-of-sparse blobs are never fetched. Every command is idempotent — a fresh
clone, a half-finished run, or an existing full checkout all converge to the
same state.

## Config schema (`.gitmodules`)

`picky` reads and writes `.gitmodules` via `git config -f`, splitting keys
across two sections joined by the shared subsection name. Git's **standard**
keys stay in the `submodule.<name>` section it understands; picky's **own**
options live in a parallel `picky.<name>` section that stock git ignores
entirely.

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

The `picky.<name>` section holds **only** the options git doesn't understand —
nothing is duplicated from the `submodule.<name>` section. The pin (gitlink SHA)
lives where git already keeps it — the superproject tree (`git ls-files -s
<path>`); no extra config file is introduced.

Example:

```ini
[submodule "ext/duckdb"]      # git's section — git's keys only
	path = ext/duckdb
	url = https://github.com/duckdb/duckdb.git
	branch = main
	shallow = true
[picky "ext/duckdb"]          # picky's section — only what git doesn't know
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
picky sparse clear -p ext/duckdb                         # drop all → full checkout
picky sparse add /x/ -p ext/duckdb --no-reinit           # edit .gitmodules only
```

It edits `.gitmodules`, stages it, then re-runs `init` for that submodule
(`--no-reinit` skips the reconcile). Removal is by **exact value**, so patterns
with metacharacters like `/extension/*.cmake` work without the `git config
--unset` regex footgun.

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

A submodule may declare `picky.<name>.postUpdate` — a shell command run after
its working tree is (re)materialized by `add`, `init`, or `update` (in the latter
case, after the patch stack). It runs through `sh -c` **in the submodule's
working tree**, and a non-zero exit is fatal. This is the seam for
project-specific glue such as ducky's `OVERRIDE_GIT_DESCRIBE` CMake rewrite.

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

## Commands

| Command | Purpose |
|---|---|
| `picky add <url> <path> [opts]` | declare + check out a new sparse submodule |
| `picky init [<path>…]`          | reconstruct checkout(s) from `.gitmodules` (no args ⇒ all) |
| `picky update [<path>] [<ref>]` | bump pin / re-checkout / re-apply patches |
| `picky sparse <list/add/remove/clear>` | edit sparse patterns + reconcile |
| `picky status [<path>…]`        | table of pin, branch, sparse, filter, size, patches |
| `picky refresh [<path>…]`       | refresh the cached remote ref list (for `<ref>` completion) |
| `picky completions <shell>`     | print the eval-able dynamic-completion registration script |

Global flags: `-q/--quiet`, `-v/--verbose`. Color honors `NO_COLOR` and TTY
detection. Run `picky <command> --help` for the full option list.

`update` accepts its two positionals smartly: with one argument, a value that
matches a submodule path is treated as the path (refresh at current pin);
otherwise it's treated as a ref against the lone submodule.

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

You then get `<TAB>` completion on submodule paths (`init`, `update`, `sparse`,
`status`) and on remote tags/branches for `picky update <path> <TAB>`.

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
