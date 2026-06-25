# picky — a lightweight sparse-checkout client (Rust)

## Context

`mew` and `ducky` each carry hand-written `sh` scripts that sparsely check out a
git submodule: fetch it shallow + blobless, configure a non-cone sparse-checkout
*before* the first checkout so unused paths are never written, and (in ducky)
apply a working-tree patch stack on top of a pinned upstream commit. `ducky`'s
`scripts/init-duckdb.sh` and `scripts/bump-duckdb.sh` are the battle-tested
reference implementations; `mlir-metal` (`ext/llvm` = the full LLVM monorepo) is
the multi-GB stress case where a full checkout is wasteful.

`picky` consolidates that knowledge into one reusable binary so each repo no
longer needs a bespoke script. Decisions locked with the user:

- **Rust**, single binary, runnable standalone. **Shell out to the `git` CLI**
  (via `std::process::Command`), *not* libgit2 — partial-clone filters,
  `git sparse-checkout`, promisor config and `--separate-git-dir` are exactly
  where libgit2 is weak/absent, and the proven scripts already use `git`.
- **Declarative config in `.gitmodules`** (committed) — `picky` reconstructs any
  checkout reproducibly from it. Git's standard keys stay in `submodule.<name>.*`;
  picky's own options live in a parallel `picky.<name>.*` section stock git
  ignores, joined by the shared subsection name.
- **Patch stack in v1** — port `bump-duckdb.sh`'s `--3way` lexical-order apply.

## Reference behavior (what we are porting)

- `ducky/scripts/init-duckdb.sh` — build the submodule git dir under
  `.git/modules/<path>`, add remote, set `extensions.partialClone`,
  `remote.origin.promisor`, `partialclonefilter=blob:none`,
  `core.sparseCheckout=true`, `core.sparseCheckoutCone=false`, write patterns to
  `<gitdir>/info/sparse-checkout`, fetch the pinned SHA `--filter=blob:none --depth 1`
  if absent, `checkout -f --detach`, `sparse-checkout reapply`. Fully idempotent.
- `ducky/scripts/bump-duckdb.sh` — move the pin (fetch tags/history blobless,
  resolve bare branch → `origin/<branch>`), `checkout -f --detach`,
  `sparse-checkout reapply`, then `git apply --3way` each `patches/*.patch` in
  lexical order; any failure is fatal and leaves conflict markers. (The
  `OVERRIDE_GIT_DESCRIBE` CMake rewrite is ducky-specific glue — see Out of scope.)

## Config schema in `.gitmodules`

Read/written with `git config -f .gitmodules`. Git's standard keys stay in the
`submodule.<name>` section: `path`, `url`, `branch`, `shallow`. Picky's own
options live in a parallel `picky.<name>` section (ignored by stock git), holding
only what git doesn't understand:

- `picky.<name>.sparse` — **multivalued** (`--add` per pattern), the non-cone
  pattern list. Absence ⇒ full checkout.
- `picky.<name>.depth` — integer for `--depth` (shallow fetch).
- `picky.<name>.filter` — e.g. `blob:none` (blobless partial clone). Default on.
- `picky.<name>.patches` — dir of the patch stack (e.g. `patches`). Optional.
- `picky.<name>.postUpdate` — shell command run after each checkout (the
  post-update hook). Optional.

The pin (gitlink SHA) stays where git already keeps it — the superproject tree —
read via `git ls-files -s <path>`. No new config file is introduced.

## Command surface (clap derive + clap_complete)

- `picky add <url> <path> [--sparse <pat>]… [--depth N] [--filter blob:none|none] [--branch B] [--ref R] [--patches DIR]`
  — write the `.gitmodules` entry, stage it, then run the init path below.
- `picky init [<path>…]` — reconstruct checkout(s) from committed config
  (init-duckdb.sh equivalent). No args ⇒ every submodule. Idempotent.
- `picky update [<path>] [<ref>] [--no-patches] [--unshallow] [--depth N]` — bump
  pin / re-checkout / re-apply patch stack (bump-duckdb.sh equivalent). Default
  fetches only the target ref, shallow + blobless (a bare branch lands on the
  fresh remote tip via FETCH_HEAD); `--unshallow` opts into full history + tags
  for `git describe` at the cost of a fat (blobless) object store.
- `picky status [<path>…]` — table: name, pinned SHA (short), branch, sparse on/off,
  filter, working-tree size (`du`-style), patches-applied count.
- `picky sparse <list|add|remove|clear> [<pat>…] [-p <path>] [--no-reinit]` —
  edit the `picky.<name>.sparse` list and reconcile via init. The operation is a
  subcommand; the submodule is named with `-p/--path` (optional when there's one).
- `picky refresh [<path>…]` — refresh the per-remote ref cache used by `<ref>`
  completion (also spawned detached to warm a stale cache).
- `picky completions <shell>` — print the eval-able registration script for
  clap_complete's dynamic completion engine (à la `zoxide init`, via
  `EnvCompleter::write_registration`; use `eval "$(picky completions zsh)"`).
  Path arguments
  complete on submodule paths read live from `.gitmodules`; `update <ref>`
  completes on remote tags/branches, hybrid-cached per remote URL under
  `${XDG_CACHE_HOME}/picky/refs` (1h TTL, 2s-bounded cold fetch, offline decay
  to local refs).

UX: `>> step…` progress lines mirroring the scripts; colored via `anstyle`/`anstream`
(honor `NO_COLOR`/`FORCE_COLOR`/TTY, as mew's `_console` does); `--quiet`/`--verbose`.

## Crate layout

```
Cargo.toml            # clap (derive), clap_complete, anstyle/anstream, anyhow
src/
  main.rs             # clap parser + dispatch
  git.rs              # Command wrapper: run/run_in(dir)/capture/check; central git invoker
  config.rs           # Submodule model; read/write .gitmodules via `git config -f`
  sparse.rs           # gitdir construction, partial-clone/promisor + sparse config, fetch
  submodule.rs        # high-level add / init / update orchestration
  patch.rs            # discover patches/*.patch (lexical), `git apply --3way`, report
  console.rs          # styled progress/status output, color gate
  commands/{add,init,update,status,completions}.rs
tests/
  integration.rs      # temp superproject + bare local file:// remote fixtures
```

Dependencies kept deliberately small: `clap` + `clap_complete` for the CLI and
completions, `anstyle`/`anstream` (already in clap's stack) for color, `anyhow`
for error context. **No libgit2.**

## Key implementation notes

- `git.rs` is the only place that spawns `git`; everything goes through
  `run_in(dir, args)` returning captured stdout + a checked status, so the
  partial-clone/sparse incantations live in one auditable module.
- `sparse.rs::init` mirrors init-duckdb.sh step-for-step and must stay idempotent:
  create gitdir only if `<path>/.git` absent; `remote set-url` vs `add`; fetch the
  pin only if `cat-file -e <sha>^{commit}` fails; always `checkout -f --detach`
  then `sparse-checkout reapply` to trim a pre-existing full checkout.
- `update.rs`: by default a bump fetches only the target ref shallow + blobless
  (like `add`) and checks out `FETCH_HEAD`, so the object store never grows and a
  bare branch lands on the fresh remote tip (no stale-local-ref footgun) without
  pulling every ref. `--unshallow` restores the bump-duckdb.sh behavior — fetch
  `--tags --filter=blob:none --unshallow` + all heads, then map a bare branch to
  the freshly-fetched `origin/<branch>` — for when `git describe`/history is
  genuinely needed.
- `patch.rs`: apply in lexical order; a failing `--3way` is fatal and we surface
  the conflict-marker guidance, matching ducky's behavior.

## Out of scope (v1)

- Cone-mode sparse-checkout (the scripts use non-cone; keep that default).

(Implemented after v1:
- `picky sparse <list|add|remove|clear>` edits the multivalued
  `picky.<name>.sparse` key — removal by exact value, avoiding git's `--unset`
  regex — and reconciles via init.
- The `picky.<name>.postUpdate` hook — a `sh -c` command run after each checkout
  (in `add`/`init`/`update`), in the submodule worktree, with `PICKY_ROOT`,
  `PICKY_SUBMODULE_{NAME,PATH,SHA}` exported; a non-zero exit is fatal. This is
  the seam for ducky's project-specific `OVERRIDE_GIT_DESCRIBE` CMake rewrite.
- `hook.rs` trust gate — `postUpdate` is sourced from committed `.gitmodules`,
  so it is never run unconditionally (mirrors git's own fix for
  CVE-2015-7545, where a `.gitmodules`-sourced `submodule.<name>.update = !cmd`
  ran arbitrary commands). Approval is recorded verbatim in local, untracked
  config (`picky.<name>.trustedPostUpdate`); non-interactive runs need a prior
  approval or `PICKY_TRUST_HOOKS=1`.
- `add.rs` ordering fix — the checkout (`sparse::prepare`/`fetch_ref`/`checkout`/
  the post-update hook) now runs entirely off the in-memory `Submodule` before
  anything touches `.gitmodules` on disk; a failed `add` used to stage a
  `.gitmodules` entry with no working gitlink behind it.
- `picky remove <path>…` — the inverse of `add`: deletes the working tree and
  submodule git dir, drops the gitlink from the index (`git rm --cached`),
  best-effort `git submodule deinit`s the local registration, and strips the
  `submodule.<name>`/`picky.<name>` sections from `.gitmodules` via the new
  `config::remove`, staging the result. No implicit "remove all" — paths are
  required.
- `config::write` made declarative — `set_or_unset` clears an optional key
  (`branch`, `shallow`, `depth`, `filter`, `patches`, `postUpdate`) when the
  new value is absent, instead of only ever setting it. Re-running `add` over
  an existing name now converges to exactly the new args rather than unioning
  old and new (a stale `--branch`/`--depth` used to survive a re-add that
  omitted them).
- `update.rs` default-bump fetch failures now carry a `--unshallow` hint —
  many git servers refuse to fetch an arbitrary commit SHA directly (only
  advertised branch/tag tips), which the default (shallow, single-ref) bump
  path used to surface as a bare git exit-status error with no guidance.)

## Verification

1. **Unit/integration tests** (`tests/integration.rs`): create a bare local repo
   as a `file://` remote with `uploadpack.allowFilter=true`, a superproject that
   pins it, then assert: `init` writes only sparse paths (unlisted paths absent),
   the clone is shallow (`rev-parse --is-shallow-repository` = true) and blobless
   (`rev-list --filter=blob:none --missing=print` shows missing blobs), `update`
   moves the pin, and a fixture patch applies + a deliberately-broken patch fails
   fatally. Re-running `init` is a no-op (idempotency).
2. **Stress / acceptance against the real repos** (manual): in `ducky`, populate
   `.gitmodules` with the `ext/duckdb` sparse patterns and confirm `picky init`
   reproduces the ~50 MB working tree (vs ~280 MB full); `picky update <ref>`
   bumps + applies `patches/`. In `mlir-metal`, sparse-init `ext/llvm` to just the
   MLIR paths and confirm we avoid the multi-GB full checkout.
3. `picky status` renders correctly; `picky completions {bash,zsh,fish}` emit
   loadable scripts.
