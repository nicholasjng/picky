# Project picky: A lightweight sparse-checkout client for development

This repo is supposed to consolidate the knowledge obtained from my work on the mew and ducky repositories.
Tentative name is "picky", but maybe we can find a better one.

## Situation

The mentioned repos (checked out under as `mew` and `ducky` in ~/Workspaces/python, respectively) each contain shell scripts for sparsely checking out submodules, since often, not the whole project needs to be present.
In extreme cases (e.g. LLVM), the full checkout can be several gigabytes, while only a fraction of that is necessary.

## Goal

Implement a client (Python, Go, rust, ...) with the following capabilities for sparse checkouts in git repositories:

- Adding a submodule, with the potential for sparse-checkout paths to avoid a full fetch.
- Support for --depth to avoid fetching the whole history.
- Updating a submodule to a SHA, ref, etc., with the option to apply patches later (this is the case in ducky with the patches/ directory).

## Requirements

An executable that can be run in git repos, either standalone or as a Python script (e.g. with `uvx` or `uv tool`).
Bonus for good user experience (CLI polish, completions, ANSI support, etc.).

The tech stack is up for debate, but the CLI should be lightweight. When in doubt, check the design decisions from mew (Python stdlib + hand-rolled ANSI markup module)

## Stress tests / supported scenarios

The client should work as-is in the mentioned mew and ducky repos. For a stress test against LLVM, you can check the ~/Workspaces/mlir-metal repo.
