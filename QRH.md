# picky QRH

Quick reference handbook. Find the condition, run the procedure, top to bottom.

`<path>` is the submodule path as declared in `.gitmodules` (e.g. `ext/duckdb`).
It can be omitted from any command when the superproject declares exactly one
submodule. Reference material lives in [DOC.md](DOC.md); the command list is in
[README.md](README.md#commands).

## Index

| Condition | § |
|---|---|
| Cloned a repo, submodule directories are empty | [1.1](#11-fresh-clone--submodules-empty) |
| Want to vendor a new dependency, but only part of it | [1.2](#12-add-a-new-dependency-sparsely) |
| Have a normal submodule, want it sparse | [1.3](#13-convert-an-existing-submodule-to-sparse) |
| New machine, want TAB completion | [1.4](#14-enable-shell-completion) |
| Need to move a dependency to a new version | [2.1](#21-bump-a-dependency) |
| Pulled, and someone else moved a pin | [2.2](#22-sync-after-someone-else-moved-a-pin) |
| Want to know if a dependency has updates | [2.3](#23-check-for-upstream-updates) |
| Bumped by mistake, want the old pin back | [2.4](#24-undo-a-bump) |
| Done with a dependency | [2.5](#25-drop-a-dependency) |
| Need a file that isn't in the checkout | [3.1](#31-a-needed-file-is-missing-from-the-checkout) |
| Checkout is bigger than it needs to be | [3.2](#32-narrow-an-oversized-checkout) |
| Need the whole tree after all | [3.3](#33-go-back-to-a-full-checkout) |
| Pattern list has outgrown the command line | [3.4](#34-manage-a-long-pattern-list) |
| Need a local change to a dependency | [4.1](#41-add-a-local-modification) |
| Need a second local change on top | [4.2](#42-add-another-patch-to-an-existing-stack) |
| A patch stopped applying after a bump | [4.3](#43-a-patch-failed-to-apply) |
| Want to build without the patches | [4.4](#44-build-without-the-patch-stack) |
| A patch is no longer needed | [4.5](#45-retire-a-patch) |
| Fetch refused the commit | [5.1](#51-fetch-refused--not-our-ref) |
| Repo grew after many bumps | [5.2](#52-object-store-grew-after-repeated-bumps) |
| `git describe` fails inside the submodule | [5.3](#53-git-describe-fails-in-the-submodule) |
| Checkout looks broken | [5.4](#54-checkout-looks-broken) |
| Update refused: "changes beyond its patch stack" | [5.5](#55-changes-beyond-its-patch-stack) |
| Asked to approve a post-update hook | [5.6](#56-post-update-hook-approval-prompt) |
| Running picky in CI | [6.1](#61-run-picky-non-interactively) |
| Want CI to catch a broken `.gitmodules` | [6.2](#62-guard-gitmodules-in-ci) |

---

## 1. Setup

### 1.1 Fresh clone — submodules empty

**CONDITION** — Superproject cloned; submodule directories exist but are empty.

1. `picky init` — reconstruct every declared submodule at its pinned commit.
2. `picky update --all` — **only if** any submodule declares a patch stack.
3. `picky status` — verify pins, sizes and patch counts.

**NOTE** — Step 2 is not optional when patches are in play. `init` checks out
*pristine* upstream and does not apply patches; `update` is what applies them.
`picky status` shows a non-zero `PATCHES` count for the submodules that need it.

### 1.2 Add a new dependency sparsely

**CONDITION** — You want to vendor a large repo but need only part of its tree.

1. `picky add <url> <path> --branch <branch> --sparse /dir1/ --sparse /dir2/`
2. `ls <path>` — check you got what you need, and nothing else.
3. `picky sparse add /dir3/ -p <path>` — repeat until the build works.
4. `git commit` — `.gitmodules` and the gitlink are already staged.

**NOTE** — Defaults are shallow (depth 1) and blobless (`--filter=blob:none`).
Pass `--depth N` for more history, or `--filter none` to disable filtering.

**NOTE** — Patterns are non-cone, gitignore-style. Use a trailing `/` for a
directory (`/src/`), and a leading `/` to anchor at the repo root.

### 1.3 Convert an existing submodule to sparse

**CONDITION** — A plain `git submodule` checkout you want picky to manage.

1. `picky sparse set /src/ /include/ -p <path>` — declares the patterns, stages
   `.gitmodules`, and trims the checkout in one step.
2. `picky status` — confirm the new size.
3. `git commit`

**NOTE** — This trims the *working tree* but leaves the existing object store
as it is. To also shrink what's on disk under `.git/modules`, follow
[5.2](#52-object-store-grew-after-repeated-bumps).

### 1.4 Enable shell completion

**CONDITION** — New machine, or completion isn't firing.

1. Add the line for your shell to its startup file:
   - bash — `source <(picky completions bash)`
   - zsh — `eval "$(picky completions zsh)"`
   - fish — `picky completions fish | source`
2. Restart the shell.
3. `picky refresh` — warm the remote ref cache so `<ref>` completion is instant.

**NOTE** — `picky` must be on `PATH`; the shell calls back into the binary to
compute candidates.

---

## 2. Routine operations

### 2.1 Bump a dependency

**CONDITION** — You want a dependency at a newer tag, branch tip, or commit.

1. `picky update <path> <ref>` — fetches only that ref (shallow, blobless),
   re-checks out, re-applies the patch stack.
2. `picky status` — confirm the new pin.
3. `git commit` — the moved gitlink is already staged.

**IF** the fetch is refused → [5.1](#51-fetch-refused--not-our-ref).
**IF** a patch fails to apply → [4.3](#43-a-patch-failed-to-apply).

### 2.2 Sync after someone else moved a pin

**CONDITION** — You pulled the superproject; a submodule pin changed.

1. `picky update --all` — moves every checkout onto its newly pinned commit and
   re-applies each patch stack.
2. `picky status` — verify.

**NOTE** — Idempotent. Each submodule reports `moved onto its pin` or
`already on its pin`.

**NOTE** — `picky init` also moves checkouts onto their pins, but without
patches ([1.1](#11-fresh-clone--submodules-empty)). Use it when a checkout
needs rebuilding, not just moving.

### 2.3 Check for upstream updates

**CONDITION** — You want to know whether a dependency has moved on.

1. `picky refresh` — fetch remote ref lists into the local cache.
2. `picky status` — read the `UPSTREAM` column: `current`, or `stale→<sha>`.

**NOTE** — `status` never touches the network; it reads only the cache, so
step 1 is what makes the column meaningful. `?` means no cache yet.

**NOTE** — The column needs a tracked branch (`submodule.<name>.branch`). A pin
on a bare SHA or tag shows `-`, since there's no "latest" to compare against.

### 2.4 Undo a bump

**CONDITION** — You bumped a pin and want the previous one back.

1. `git checkout HEAD -- <path>` — restore the old gitlink in the index.
2. `picky init <path>` — move the working tree back to it.
3. `picky update <path>` — **only if** the submodule declares a patch stack.

**NOTE** — If you already committed the bump, revert that superproject commit
first, then run steps 2 and 3.

### 2.5 Drop a dependency

**CONDITION** — A submodule is no longer needed.

1. `picky remove <path>` — confirm with `y` at the prompt.
2. `git commit` — the `.gitmodules` edit and index removal are already staged.

**CAUTION** — This deletes the working tree and the submodule's git dir. Any
uncommitted work inside it is gone. picky warns first if it finds changes.

---

## 3. Checkout contents

### 3.1 A needed file is missing from the checkout

**CONDITION** — A build fails on a missing file that exists upstream.

1. `picky sparse list -p <path>` — see what's currently declared.
2. `picky sparse add /the/missing/dir/ -p <path>` — declares it and
   materializes it immediately.
3. `git commit .gitmodules`

**NOTE** — Widening never refetches the whole repo. Only the newly matched
blobs are lazily pulled from the promisor remote.

### 3.2 Narrow an oversized checkout

**CONDITION** — A submodule is taking more disk than it should.

1. `picky status` — read the `SIZE` column (the working tree).
2. `picky sparse list -p <path>` — find the pattern pulling in the bulk.
3. `picky sparse remove /that/dir/ -p <path>` — removes it and trims the tree.
4. `git commit .gitmodules`

**NOTE** — Removal is by *exact* pattern text, so metacharacters like
`/extension/*.cmake` are safe to remove verbatim.

**NOTE** — Sparse patterns don't shrink the `STORE` column; that's
[5.2](#52-object-store-grew-after-repeated-bumps).

### 3.3 Go back to a full checkout

**CONDITION** — Sparse filtering is now more trouble than it's worth.

1. `picky sparse clear -p <path>` — drops all patterns and restores every file.
2. `git commit .gitmodules`

**NOTE** — To re-narrow later, `picky sparse set /a/ /b/ -p <path>`.

### 3.4 Manage a long pattern list

**CONDITION** — The pattern list is too long to keep passing on the command line.

1. Keep the canonical list in a committed file, one pattern per line; blank
   lines and `#` comments are ignored.
2. `picky sparse set -p <path> --from sparse-paths.txt`
3. `git commit .gitmodules sparse-paths.txt`

**NOTE** — `--stdin` reads the same format, so a heredoc works for one-offs.

---

## 4. Local patches

A patch stack is a *working-tree overlay* re-applied on top of a pristine
upstream commit at every update. Never commit inside the submodule.

### 4.1 Add a local modification

**CONDITION** — You need a change to a dependency that isn't upstream yet.

1. Ensure the submodule declares a patch directory. If not:
   `git config -f .gitmodules picky.<name>.patches patches`
2. Edit the files inside `<path>`.
3. `git -C <path> diff HEAD > patches/0001-short-description.patch`
4. `git add patches .gitmodules && git commit`
5. `picky update <path>` — verify the patch reapplies cleanly from pristine.

**NOTE** — Patches apply in lexical filename order, hence the `NNNN-` prefix.

### 4.2 Add another patch to an existing stack

**CONDITION** — The stack already has patches and you want one more on top.
A plain `diff HEAD` here would capture the whole stack, not just your change.
Use the index as a baseline:

1. `picky update <path>` — clean state, existing stack applied.
2. `git -C <path> add -A` — index now holds the patched state.
3. Edit the files.
4. `git -C <path> diff > patches/0002-short-description.patch` — worktree
   against index, so only your new change.
5. `git -C <path> reset` — unstage; the working tree keeps its contents.
6. `git add patches && git commit`
7. `picky update <path>` — verify the whole stack reapplies.

### 4.3 A patch failed to apply

**CONDITION** — `picky update` failed with `patch(es) failed to apply`.

The error names each failing patch and quotes git's own reason. Read it first:

1. **IF** the reason is `does not exist in index` — the file the patch targets
   was renamed or deleted upstream. The patch must be rewritten against the new
   layout, or retired ([4.5](#45-retire-a-patch)).
2. **IF** the reason is `patch failed` / conflict — `--3way` left conflict
   markers in the working tree:
   - Open the affected files under `<path>` and resolve the markers.
   - `git -C <path> diff HEAD > patches/<the-failing-patch>.patch` — regenerate
     from the resolved tree. Collapse the stack into one patch, or regenerate
     the individual one with the index baseline from
     [4.2](#42-add-another-patch-to-an-existing-stack).
   - `picky update <path>` — verify.
3. To get back to a clean upstream tree at any point:
   `picky update <path> --no-patches`.

### 4.4 Build without the patch stack

**CONDITION** — You want to test against pristine upstream.

1. `picky update <path> --no-patches`
2. `picky update <path>` — restores the stack afterwards.

### 4.5 Retire a patch

**CONDITION** — A local change landed upstream, or is no longer wanted.

1. `rm patches/<the-patch>.patch`
2. `picky update <path>` — re-materializes without it.
3. `git add patches && git commit`

---

## 5. Abnormal situations

### 5.1 Fetch refused — "not our ref"

**CONDITION** — A bump fails while fetching, often mentioning `not our ref`.

**CAUSE** — Most servers only serve commits that are an advertised branch or
tag tip. A bare commit SHA in the middle of history is refused.

1. `picky update <path> <ref> --unshallow` — fetches full history (still
   blobless) so the commit resolves locally.

**NOTE** — Prefer naming a tag or branch over a raw SHA where you can; it keeps
the default shallow fetch working.

### 5.2 Object store grew after repeated bumps

**CONDITION** — `picky status` shows a `STORE` far above `SIZE`: the object
store in `.git/modules/<path>` outweighs the working tree.

**CAUSE** — `git gc` can never reclaim a partial clone's old packs, so every
bump adds to the store permanently.

1. `picky update <path> <ref> --fresh` — throws the git dir away and refetches
   only the target commit.

**NOTE** — Name a tag or branch as `<ref>`. Rebuilding at a bare pin SHA that
isn't an advertised tip hits [5.1](#51-fetch-refused--not-our-ref).

**CAUTION** — `--fresh` deletes the working tree and git dir for that submodule.
Export any local edits as patches first ([4.1](#41-add-a-local-modification)).

### 5.3 `git describe` fails in the submodule

**CONDITION** — A build step inside the submodule needs tags or history, and a
shallow checkout has neither.

1. `picky update <path> <ref> --unshallow` — fetches full history and all tags.

**NOTE** — On a large repo this fattens the (still blobless) object store with
every tree and commit. Only do it for submodules that genuinely need it.

### 5.4 Checkout looks broken

**CONDITION** — Files missing, git commands failing inside the submodule, or
state left over from hand-editing `.gitmodules`.

1. `picky doctor` — names the problem. Then, by what it reports:
   - *dangling gitlink* → `picky init <path>` rebuilds it.
   - *sparse patterns drifted* → `picky init <path>` puts the checkout back on
     the declared patterns, or `picky sparse set …` if the declaration is the
     stale half.
   - *orphaned git dir* → `rm -rf` the path it names.
   - *`picky.<name>` with no `submodule.<name>`* → finish or remove the section
     in `.gitmodules`.
   - *path escapes the superproject* → fix the `path` value; picky refuses to
     check out or delete it.
2. `picky init <path>` — reconcile.
3. **IF** still broken → `picky update <path> <ref> --fresh`
   ([5.2](#52-object-store-grew-after-repeated-bumps)).
4. **IF** still broken → `picky remove <path>` then `picky add …` from scratch.

### 5.5 "changes beyond its patch stack"

**CONDITION** — `picky update` (or a `picky sparse` edit) refuses to run because
the working tree changed since picky last left it.

**CAUSE** — An edit inside the submodule that isn't captured by a patch. The
check does *not* fire for the patch stack itself.

1. `git -C <path> status` and `git -C <path> diff` — see what it is.
2. **IF** you want to keep it → export it as a patch
   ([4.2](#42-add-another-patch-to-an-existing-stack)), then re-run with `--force`.
3. **IF** not → re-run with `--force`; the checkout is reset to target + patches.

### 5.6 Post-update hook approval prompt

**CONDITION** — picky prints a shell command and asks `run it? [y/N]`.

**CAUSE** — The submodule declares `picky.<name>.postUpdate`, which comes from
committed `.gitmodules` and so is attacker-controlled in someone else's repo.

1. Read the command. Accept only if you understand what it does.
2. `y` records the approval locally, pinned to that exact command text *and*
   the submodule's current SHA. Either changing re-prompts.

**NOTE** — For unattended runs → [6.1](#61-run-picky-non-interactively).

---

## 6. CI and automation

### 6.1 Run picky non-interactively

**CONDITION** — CI or a script, with no terminal to prompt at.

1. Export `PICKY_TRUST_HOOKS=1` if any submodule declares a post-update hook.
   Without it, an unapproved hook is a hard failure rather than a silent skip.
2. Pass `--yes` to `picky remove`; it refuses to delete unprompted otherwise.
3. Use `-q` to keep logs readable.

```sh
export PICKY_TRUST_HOOKS=1
picky -q init
picky -q update --all      # only needed if a patch stack is declared
```

**CAUTION** — `PICKY_TRUST_HOOKS=1` auto-approves *any* hook it encounters. Set
it only where the checked-out content is already trusted.

### 6.2 Guard `.gitmodules` in CI

**CONDITION** — You want a broken or hand-edited declaration to fail the build.

1. `picky doctor --strict` — exits 1 when any issue is found.

**NOTE** — Also useful as a pre-commit hook.
