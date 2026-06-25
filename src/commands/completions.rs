//! `picky completions <shell>` — print the eval-able registration script for
//! clap_complete's completion *engine* (à la `zoxide init`). The script makes
//! the shell call back into the binary at completion time, so candidates reflect
//! the live repo. Wire up with `eval "$(picky completions zsh)"`.

use anyhow::{Result, bail};
use clap_complete::Shell;
use clap_complete::env::Shells;
use std::io;

pub fn run(shell: Shell) -> Result<()> {
    let name = shell.to_string();
    let shells = Shells::builtins();
    let Some(completer) = shells.completer(&name) else {
        bail!("unsupported shell: {name}");
    };
    // env var "COMPLETE"; name/bin/callback all "picky" (must be on PATH).
    completer.write_registration("COMPLETE", "picky", "picky", "picky", &mut io::stdout())?;
    Ok(())
}
