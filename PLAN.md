# picky: a lightweight sparse-checkout client (Rust)

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
  (via `std::process::Command`), *not* libgit2: partial-clone filters,
  `git sparse-checkout`, promisor config and `--separate-git-dir` are exactly
  where libgit2 is weak/absent, and the proven scripts already use `git`.
- **Declarative config in `.gitmodules`** (committed): `picky` reconstructs any
  checkout reproducibly from it. Git's standard keys stay in `submodule.<name>.*`;
  picky's own options live in a parallel `picky.<name>.*` section stock git
  ignores, joined by the shared subsection name.
- **Patch stack in v1**: port `bump-duckdb.sh`'s `--3way` lexical-order apply.

## Reference behavior (what we are porting)

- `ducky/scripts/init-duckdb.sh`: build the submodule git dir under
  `.git/modules/<path>`, add remote, set `extensions.partialClone`,
  `remote.origin.promisor`, `partialclonefilter=blob:none`,
  `core.sparseCheckout=true`, `core.sparseCheckoutCone=false`, write patterns to
  `<gitdir>/info/sparse-checkout`, fetch the pinned SHA `--filter=blob:none --depth 1`
  if absent, `checkout -f --detach`, `sparse-checkout reapply`. Fully idempotent.
- `ducky/scripts/bump-duckdb.sh`: move the pin (fetch tags/history blobless,
  resolve bare branch → `origin/<branch>`), `checkout -f --detach`,
  `sparse-checkout reapply`, then `git apply --3way` each `patches/*.patch` in
  lexical order; any failure is fatal and leaves conflict markers. (The
  `OVERRIDE_GIT_DESCRIBE` CMake rewrite is ducky-specific glue, see Out of scope.)

## Config schema in `.gitmodules`

Read/written with `git config -f .gitmodules`. Git's standard keys stay in the
`submodule.<name>` section: `path`, `url`, `branch`, `shallow`. Picky's own
options live in a parallel `picky.<name>` section (ignored by stock git), holding
only what git doesn't understand:

- `picky.<name>.sparse`: **multivalued** (`--add` per pattern), the non-cone
  pattern list. Absence ⇒ full checkout.
- `picky.<name>.depth`: integer for `--depth` (shallow fetch).
- `picky.<name>.filter`: e.g. `blob:none` (blobless partial clone). Default on.
- `picky.<name>.patches`: dir of the patch stack (e.g. `patches`). Optional.
- `picky.<name>.postUpdate`: shell command run after each checkout (the
  post-update hook). Optional.

The pin (gitlink SHA) stays where git already keeps it, the superproject tree,
read via `git ls-files -s <path>`. No new config file is introduced.

## Command surface (clap derive + clap_complete)

- `picky add <url> <path> [opts]`: write the `.gitmodules` entry, stage it,
  then run the init path below.
- `picky remove <path>… [--yes]`: the inverse of `add`.
- `picky init [<path>…]`: reconstruct checkout(s) from committed config
  (init-duckdb.sh equivalent). No args ⇒ every submodule. Idempotent.
- `picky update [<path>] [<ref>] [opts] [--all]`: bump pin / re-checkout /
  re-apply patch stack (bump-duckdb.sh equivalent), or (`--all`) refresh
  every submodule at its current pin. Default fetches only the target ref,
  shallow + blobless (a bare branch lands on the fresh remote tip via
  FETCH_HEAD); `--unshallow` opts into full history + tags for `git describe`.
- `picky status [<path>…] [--json]`: table (or JSON) of pin, branch, upstream
  staleness, sparse state, filter, working-tree size, patch count.
- `picky sparse <list|add|remove|set|clear> [<pat>…] [-p <path>] [--no-reinit]`:
  edit the `picky.<name>.sparse` list and reconcile via init.
- `picky refresh [<path>…]`: refresh the per-remote ref cache used by `<ref>`
  completion and `status`'s upstream column (also spawned detached to warm a
  stale cache).
- `picky doctor [--strict]`: diagnose drift between `.gitmodules` and actual
  disk state.
- `picky completions <shell>`: print the eval-able registration script for
  clap_complete's dynamic completion engine (à la `zoxide init`). Path
  arguments complete on submodule paths read live from `.gitmodules`;
  `update <ref>` completes on remote tags/branches, hybrid-cached per remote
  URL under `${XDG_CACHE_HOME}/picky/refs` (1h TTL, 2s-bounded cold fetch,
  offline decay to local refs).

UX: `>> step…` progress lines mirroring the scripts; colored via `anstyle`/`anstream`
(honor `NO_COLOR`/`FORCE_COLOR`/TTY, as mew's `_console` does); `--quiet`/`--verbose`.

## Crate layout

```
Cargo.toml            # clap (derive), clap_complete, anstyle/anstream, anyhow
src/
  main.rs             # clap parser + dispatch, dynamic completion candidates
  lib.rs              # library surface: high-level wrappers over commands::*
  git.rs              # Command wrapper: run/capture/ok; central git invoker + version check
  config.rs           # Submodule model; read/write .gitmodules via `git config -f`
  sparse.rs           # gitdir construction, partial-clone/promisor + sparse config, fetch
  patch.rs            # discover patches/*.patch (lexical), `git apply --3way`, report
  hook.rs             # postUpdate hook + its trust gate
  console.rs          # styled progress/status output, color gate, confirm()
  refcache.rs         # per-remote ref name+SHA cache for completion and status
  commands/           # one module per subcommand: add, remove, init, update,
                       # sparse, status, refresh, doctor, completions
tests/
  integration.rs      # temp superproject + bare local file:// remote fixtures
```

Dependencies kept deliberately small: `clap` + `clap_complete` for the CLI and
completions, `anstyle`/`anstream` (already in clap's stack) for color, `anyhow`
for error context. **No libgit2, no serde_json** (the `serde` feature is
derive-only, for embedders; `status --json` is hand-rolled).

## Key implementation notes

- `git.rs` is the only place that spawns `git`; everything goes through
  `run`/`capture`/`ok` returning captured stdout + a checked status, so the
  partial-clone/sparse incantations live in one auditable module. A
  `check_version` preflight runs before any real command.
- `sparse.rs::prepare` mirrors init-duckdb.sh step-for-step and must stay
  idempotent: create gitdir only if `<path>/.git` absent; `remote set-url` vs
  `add`; fetch the pin only if `cat-file -e <sha>^{commit}` fails; always
  `checkout -f --detach` then `sparse-checkout reapply` to trim a
  pre-existing full checkout.
- `commands/update.rs`: by default a bump fetches only the target ref shallow
  + blobless (like `add`) and checks out `FETCH_HEAD`, so the object store
  never grows and a bare branch lands on the fresh remote tip (no
  stale-local-ref footgun) without pulling every ref. `--unshallow` restores
  the bump-duckdb.sh behavior: fetch `--tags --filter=blob:none --unshallow`
  + all heads, then map a bare branch to the freshly-fetched
  `origin/<branch>`, for when `git describe`/history is genuinely needed.
- `patch.rs`: apply in lexical order; a failing `--3way` is fatal and we
  surface the conflict-marker guidance, matching ducky's behavior.
- `hook.rs`: `postUpdate` is sourced from committed `.gitmodules`, so it is
  never run unconditionally (mirrors git's own fix for CVE-2015-7545, where a
  `.gitmodules`-sourced `submodule.<name>.update = !cmd` ran arbitrary
  commands). Approval is recorded verbatim in local, untracked config,
  pinned to both command text (`picky.<name>.trustedPostUpdate`) and
  submodule SHA (`picky.<name>.trustedPostUpdateSha`), so a commit bump
  re-arms the prompt even with unchanged command text. Non-interactive
  runs need a prior approval at the current SHA or `PICKY_TRUST_HOOKS=1`.

## Out of scope (v1)

- Cone-mode sparse-checkout (the scripts use non-cone; keep that default).

## Implemented after v1

- `picky sparse <list|add|remove|set|clear>`: edits the multivalued
  `picky.<name>.sparse` key (removal by exact value, avoiding git's
  `--unset` regex) and reconciles via init. `list` with no `-p` reports
  every submodule; the mutating actions still require an unambiguous target.
- Post-update hook (`picky.<name>.postUpdate`) with its trust gate (see
  `hook.rs` above): approve once interactively, blanket-approve via
  `PICKY_TRUST_HOOKS=1`, or trust it directly with `git config`.
- `add.rs` ordering fix: the checkout runs entirely off the in-memory
  `Submodule` before anything touches `.gitmodules` on disk, so a failed
  `add` leaves no half-declared submodule behind.
- `picky remove <path>…`: the inverse of `add`. Deletes the working tree and
  submodule git dir, drops the gitlink from the index, best-effort
  `git submodule deinit`s the local registration, and strips `.gitmodules`.
  No implicit "remove all"; asks for confirmation (`y`/`N`, or `--yes`/`-y`
  to skip, required non-interactively).
- `config::write` made declarative (`set_or_unset`): re-running `add` over an
  existing name converges to exactly the new args instead of unioning old
  and new.
- `update.rs` default-bump fetch failures now carry a `--unshallow` hint,
  since many git servers refuse to fetch an arbitrary commit SHA directly.
- `picky update --all`: refreshes every submodule at its current pin,
  closing the asymmetry with `init`/`status`/`refresh`'s "no args ⇒ all".
- `refcache` stores `(name, sha)` pairs instead of bare names, which
  `status`'s `UPSTREAM` column uses to compare a tracked branch's cached
  remote tip against the pin (cache-only; `picky refresh` populates it).
- `picky doctor [--strict]`: dangling gitlinks, orphaned
  `.git/modules/<path>` dirs, and `.gitmodules` section drift, backed by
  `config::section_names`. Diagnostic by default; `--strict` exits 1.
- `git::check_version`: a `git --version` preflight so a missing or too-old
  git fails with a clear message instead of a misattributed error deep
  inside some other command.
- `picky status --json`: hand-rolled JSON array of the same fields as the
  table (no `serde_json` dependency).

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
