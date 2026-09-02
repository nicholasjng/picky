# picky

A lightweight tool for **sparse, shallow, blobless git submodule checkouts**: fetch only the history you ask for, only the objects you ask for, and write only the paths you ask for, all declared in `.gitmodules` so any checkout is reproducible from a single command.

`picky` shells out to the `git` CLI; it is a single binary with no libgit2 dependency.

## Install

```sh
# straight from git, no checkout needed
cargo install --git https://github.com/nicholasjng/picky

# …or from a local clone of this repo
cargo install --path .
```

Make sure `~/.cargo/bin` is on your `PATH`. Requires a recent `git` (≥ 2.41 for
`GIT_NO_LAZY_FETCH`) on `PATH` at runtime, plus `sh` if a submodule declares a
post-update hook. Tested on Linux, macOS, WSL and Git Bash. Native Windows
(cmd/PowerShell without WSL) is untested: hooks need `sh`, and the ref cache
needs `XDG_CACHE_HOME` or `HOME`.

## Commands

| Command | Purpose |
|---|---|
| `picky add <url> <path> [opts]` | declare + check out a new sparse submodule |
| `picky remove <path>… [--yes]`  | undeclare a submodule and delete its checkout (the inverse of `add`) |
| `picky init [<path>…]`          | reconstruct checkout(s) from `.gitmodules` (no args ⇒ all) |
| `picky update [<path>] [<ref>]` | bump pin / re-checkout / re-apply patches |
| `picky update --all`            | move every declared submodule onto its pinned commit + re-apply patches (no bump) |
| `picky sparse <list/add/remove/set/clear>` | edit sparse patterns + reconcile |
| `picky status [<path>…] [--json]` | table (or JSON) of pin, branch, upstream staleness, sparse, filter, worktree + object-store size, patches |
| `picky refresh [<path>…]`       | refresh the cached remote ref list (for `<ref>` completion and `status`'s upstream column) |
| `picky doctor [--strict]`       | sanity-check submodule state against `.gitmodules` (diagnostic by default) |
| `picky completions <shell>`     | print the eval-able dynamic-completion registration script |

Global flags: `-q/--quiet`, `-v/--verbose`. Color honors `NO_COLOR` and TTY
detection. Run `picky <command> --help` for the full option list.

## Documentation

- **[QRH.md](QRH.md)** — quick reference: "the problem I have" → the commands
  that solve it. Start here when you haven't touched submodules in a while.
- **[DOC.md](DOC.md)** — full reference: config schema, sparse pattern editing,
  the patch stack, post-update hooks and their trust model, shell completions,
  and use as a library.

## Roadmap

- Cone-mode sparse-checkout (picky defaults to non-cone, matching the reference
  scripts).

## License

Apache-2.0. See [LICENSE](LICENSE).
