//! Styled progress / status output. Color is gated by `anstream`, which honors
//! `NO_COLOR`, `CLICOLOR_FORCE`/`FORCE_COLOR` and TTY detection automatically.

use anstyle::{AnsiColor, Color, Style};
use std::fmt::Display;

pub struct Console {
    quiet: bool,
    verbose: bool,
}

fn bold(color: AnsiColor) -> Style {
    Style::new().bold().fg_color(Some(Color::Ansi(color)))
}

impl Console {
    pub fn new(quiet: bool, verbose: bool) -> Self {
        Self { quiet, verbose }
    }

    /// A `>> step…` progress line, mirroring the reference scripts.
    pub fn step(&self, msg: impl Display) {
        if self.quiet {
            return;
        }
        let s = bold(AnsiColor::Green);
        anstream::println!("{s}>>{s:#} {msg}");
    }

    /// An indented sub-line (e.g. each applied patch).
    pub fn item(&self, msg: impl Display) {
        if self.quiet {
            return;
        }
        anstream::println!("   {msg}");
    }

    /// Extra detail, only shown under `--verbose`.
    pub fn detail(&self, msg: impl Display) {
        if !self.verbose {
            return;
        }
        let s = Style::new().dimmed();
        anstream::println!("   {s}{msg}{s:#}");
    }

    /// A terminal success line for a finished operation.
    pub fn success(&self, msg: impl Display) {
        if self.quiet {
            return;
        }
        let s = bold(AnsiColor::Green);
        anstream::println!("{s}✓{s:#} {msg}");
    }

    /// A section heading.
    pub fn heading(&self, msg: impl Display) {
        let s = bold(AnsiColor::Cyan);
        anstream::println!("{s}{msg}{s:#}");
    }

    pub fn warn(&self, msg: impl Display) {
        let s = bold(AnsiColor::Yellow);
        anstream::eprintln!("{s}warning:{s:#} {msg}");
    }

    pub fn error(&self, msg: impl Display) {
        let s = bold(AnsiColor::Red);
        anstream::eprintln!("{s}error:{s:#} {msg}");
    }

    /// Unstyled output (status rows etc.), printed even under `--quiet`.
    pub fn plain(&self, msg: impl Display) {
        anstream::println!("{msg}");
    }
}
