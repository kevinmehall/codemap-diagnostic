//! A library for formatting compiler error messages,
//! [extracted from rustc](https://github.com/rust-lang/rust/tree/master/src/librustc_errors)
//! and built on the types from the [codemap](https://github.com/kevinmehall/codemap) crate.
//!
//! # Example
//! ```
//! extern crate codemap;
//! extern crate codemap_diagnostic;
//! use codemap::CodeMap;
//! use codemap_diagnostic::{ Level, SpanLabel, SpanStyle, Diagnostic, ColorConfig, Emitter };
//!
//! fn main() {
//!   let code = "foo + bar";
//!   let mut codemap = CodeMap::new();
//!   let file_span = codemap.add_file("test.rs".to_owned(), code.to_owned()).span;
//!   let name_span = file_span.subspan(0, 3);
//!
//!   let label = SpanLabel {
//!       span: name_span,
//!       style: SpanStyle::Primary,
//!       label: Some("undefined variable".to_owned())
//!   };
//!   let d = Diagnostic {
//!       level: Level::Error,
//!       message: "cannot find value `foo` in this scope".to_owned(),
//!       code: Some("C000".to_owned()),
//!       spans: vec![label]
//!   };
//!
//!   let mut emitter = Emitter::stderr(ColorConfig::Always, Some(&codemap));
//!   emitter.emit(&[d]);
//! }
//! ```

extern crate termcolor;
extern crate codemap;
extern crate atty;

use codemap::Span;

mod lock;
mod snippet;
mod styled_buffer;
mod emitter;

pub use emitter::{ ColorConfig, Emitter };
use termcolor::{ ColorSpec, Color };

/// A diagnostic message.
#[derive(Clone, Debug, PartialEq, Eq)]
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

/// A level representing the severity of a Diagnostic.
///
/// These result in different output styling.
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
    fn color(self) -> ColorSpec {
        let mut spec = ColorSpec::new();
        use self::Level::*;
        match self {
            Bug | Error => {
                spec.set_fg(Some(Color::Red))
                    .set_intense(true);
            }
            Warning => {
                spec.set_fg(Some(Color::Yellow))
                    .set_intense(cfg!(windows));
            }
            Note => {
                spec.set_fg(Some(Color::Green))
                    .set_intense(true);
            }
            Help => {
                spec.set_fg(Some(Color::Cyan))
                    .set_intense(true);
            }
        }
        spec
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
