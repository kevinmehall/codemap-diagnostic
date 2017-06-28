extern crate term;
extern crate codemap;
extern crate isatty;

use codemap::Span;

mod lock;
mod snippet;
mod styled_buffer;
mod emitter;

pub use emitter::{ ColorConfig, Emitter };

/// A diagnostic message
#[derive(Clone, Debug)]
pub struct Diagnostic {
    /// The severity of the message, used to set color scheme
    pub level: Level,

    /// Message used as the headline of the error
    pub message: String,

    /// A short error number or code
    pub code: Option<String>,

    /// Locations to underline in the code
    pub spans: Vec<SpanLabel>,
}

/// A level representing the severity of a Diagnostic
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Level {
    Bug,
    Error,
    Warning,
    Note,
    Help,
}

impl ::std::fmt::Display for Level {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        self.to_str().fmt(f)
    }
}

impl Level {
    fn color(self) -> term::color::Color {
        use self::Level::*;
        match self {
            Bug | Error => term::color::BRIGHT_RED,
            Warning => {
                if cfg!(windows) {
                    term::color::BRIGHT_YELLOW
                } else {
                    term::color::YELLOW
                }
            }
            Note => term::color::BRIGHT_GREEN,
            Help => term::color::BRIGHT_CYAN,
        }
    }

    pub fn to_str(self) -> &'static str {
        use self::Level::*;

        match self {
            Bug => "error: internal compiler error",
            Error => "error",
            Warning => "warning",
            Note => "note",
            Help => "help",
        }
    }
}

/// A labeled region of the code related to a Diagnostic.
#[derive(Clone, Debug)]
pub struct SpanLabel {
    /// The location in the code.
    ///
    /// This Span must come from the same CodeMap used to construct the Emitter.
    pub span: Span,

    /// A label used to provide context for the underlined code.
    pub label: Option<String>,

    /// A style used to set the character used for the underline.
    pub style: SpanStyle,
}

/// Underline style for a SpanLabel.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum SpanStyle {
    Primary,
    Secondary,
}
