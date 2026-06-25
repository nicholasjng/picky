//! Styled progress / status output, routed through a pluggable [`Sink`]. The
//! default sink ([`Console::new`]) colors output to stdout/stderr via `anstream`
//! (honoring `NO_COLOR`/`FORCE_COLOR`/TTY); an embedder can supply its own with
//! [`Console::with_sink`] to capture messages as `(Level, &str)` events instead.

use anstyle::{AnsiColor, Color, Style};
use std::fmt::Display;

/// The kind of a message, so a custom [`Sink`] can route or style it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum Level {
    /// A `>> …` progress step.
    Step,
    /// An indented sub-line.
    Item,
    /// Extra detail (shown only when verbose).
    Detail,
    /// A terminal success line.
    Success,
    /// A section heading.
    Heading,
    /// A warning (stderr in the terminal sink).
    Warn,
    /// An error (stderr in the terminal sink).
    Error,
    /// Unstyled output (status rows etc.).
    Plain,
}

impl Level {
    /// A lowercase label (`"step"`, `"error"`, …), handy for forwarding to a UI.
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Step => "step",
            Level::Item => "item",
            Level::Detail => "detail",
            Level::Success => "success",
            Level::Heading => "heading",
            Level::Warn => "warn",
            Level::Error => "error",
            Level::Plain => "plain",
        }
    }
}

/// A destination for console messages. Blanket-implemented for any
/// `Fn(Level, &str) + Send + Sync`, so a closure works as a sink.
pub trait Sink: Send + Sync {
    fn emit(&self, level: Level, message: &str);
}

impl<F: Fn(Level, &str) + Send + Sync> Sink for F {
    fn emit(&self, level: Level, message: &str) {
        self(level, message)
    }
}

fn bold(color: AnsiColor) -> Style {
    Style::new().bold().fg_color(Some(Color::Ansi(color)))
}

/// The default sink: colored output to stdout, warnings/errors to stderr.
struct TerminalSink;

impl Sink for TerminalSink {
    fn emit(&self, level: Level, msg: &str) {
        match level {
            Level::Step => {
                let s = bold(AnsiColor::Green);
                anstream::println!("{s}>>{s:#} {msg}");
            }
            Level::Item => anstream::println!("   {msg}"),
            Level::Detail => {
                let s = Style::new().dimmed();
                anstream::println!("   {s}{msg}{s:#}");
            }
            Level::Success => {
                let s = bold(AnsiColor::Green);
                anstream::println!("{s}✓{s:#} {msg}");
            }
            Level::Heading => {
                let s = bold(AnsiColor::Cyan);
                anstream::println!("{s}{msg}{s:#}");
            }
            Level::Warn => {
                let s = bold(AnsiColor::Yellow);
                anstream::eprintln!("{s}warning:{s:#} {msg}");
            }
            Level::Error => {
                let s = bold(AnsiColor::Red);
                anstream::eprintln!("{s}error:{s:#} {msg}");
            }
            Level::Plain => anstream::println!("{msg}"),
        }
    }
}

pub struct Console {
    quiet: bool,
    verbose: bool,
    sink: Box<dyn Sink>,
}

impl Console {
    /// CLI console: colored output to stdout/stderr, gated by `quiet`/`verbose`.
    pub fn new(quiet: bool, verbose: bool) -> Self {
        Self {
            quiet,
            verbose,
            sink: Box::new(TerminalSink),
        }
    }

    /// Console that forwards messages to a custom [`Sink`] (a value or a
    /// `Fn(Level, &str)` closure) — for embedding, e.g. piping progress into a
    /// channel or a Tauri event. Filtering is off (`quiet = false`,
    /// `verbose = true`), so the sink sees every level, including `Detail`.
    pub fn with_sink(sink: impl Sink + 'static) -> Self {
        Self {
            quiet: false,
            verbose: true,
            sink: Box::new(sink),
        }
    }

    /// A console that discards every message — the `/dev/null` sink. Use this to
    /// run picky completely silently (e.g. when embedding and driving the UI
    /// yourself), unlike `--quiet`/[`Console::new`], which still emits headings,
    /// results and diagnostics.
    pub fn silent() -> Self {
        Self::with_sink(|_: Level, _: &str| {})
    }

    fn emit(&self, level: Level, msg: impl Display) {
        self.sink.emit(level, &msg.to_string());
    }

    /// A `>> step…` progress line, mirroring the reference scripts.
    pub fn step(&self, msg: impl Display) {
        if !self.quiet {
            self.emit(Level::Step, msg);
        }
    }

    /// An indented sub-line (e.g. each applied patch).
    pub fn item(&self, msg: impl Display) {
        if !self.quiet {
            self.emit(Level::Item, msg);
        }
    }

    /// Extra detail, only shown under `--verbose`.
    pub fn detail(&self, msg: impl Display) {
        if self.verbose {
            self.emit(Level::Detail, msg);
        }
    }

    /// A terminal success line for a finished operation.
    pub fn success(&self, msg: impl Display) {
        if !self.quiet {
            self.emit(Level::Success, msg);
        }
    }

    /// A section heading.
    pub fn heading(&self, msg: impl Display) {
        self.emit(Level::Heading, msg);
    }

    pub fn warn(&self, msg: impl Display) {
        self.emit(Level::Warn, msg);
    }

    pub fn error(&self, msg: impl Display) {
        self.emit(Level::Error, msg);
    }

    /// Unstyled output (status rows etc.), printed even under `--quiet`.
    pub fn plain(&self, msg: impl Display) {
        self.emit(Level::Plain, msg);
    }
}
