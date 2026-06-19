//! `picky completions <shell>` — print the eval-able dynamic-completion
//! registration script (à la `zoxide init`), so it can be wired up with
//! `eval "$(picky completions zsh)"` / `source <(picky completions bash)`.
//!
//! picky uses clap_complete's completion *engine*: the registration script
//! makes the shell call back into the binary (`COMPLETE=<shell> picky --`) at
//! completion time, so e.g. `picky update <ref> <TAB>` reflects the live repo.

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
    // var=COMPLETE; script identifier + bin + completer command all "picky"
    // (the binary must be on PATH for the callback to resolve).
    completer.write_registration("COMPLETE", "picky", "picky", "picky", &mut io::stdout())?;
    Ok(())
}
