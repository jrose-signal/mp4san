//! Error types returned by the public API.

use std::any::type_name;
use std::fmt;
use std::fmt::{Debug, Display};
use std::io;
use std::panic::Location;
use std::result::Result as StdResult;

use derive_more::Display;

//
// public types
//

/// Error type returned by `mediasan`.
#[derive(Debug, thiserror::Error)]
pub enum Error<E: Display> {
    /// An IO error occurred while reading the given input.
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    /// The input could not be parsed as a media file.
    #[error("Parse error: {0}")]
    Parse(#[from] Report<E>),
}

/// A report with additional debugging info for an error.
///
/// A `Report<E>` can be used to identify exactly where the error `E` occurred in `mediasan`. The [`Debug`]
/// implementation will print a human-readable parser stack trace. The underlying error of type `E` can also be
/// retrieved e.g. for matching against with [`get_ref`](Self::get_ref) or [`into_inner`](Self::into_inner).
#[derive(thiserror::Error)]
#[error("{error}")]
pub struct Report<E> {
    #[source]
    error: E,
    inner: Box<ReportInner>,
}

/// A [`Display`]-able indicating there was extra trailing input after parsing.
#[derive(Clone, Copy, Debug, Display)]
#[display(fmt = "extra unparsed input")]
pub struct ExtraUnparsedInput;

/// A [`Display`]-able indicating an error occurred while parsing a certain type.
#[derive(Clone, Copy, Debug, Display)]
#[display(fmt = "while parsing value of type `{}`", _0)]
pub struct WhileParsingType(&'static str);

//
// private types
//

/// A convenience type alias for a [`Result`](std::result::Result) containing an error wrapped by a [`Report`].
pub type Result<T, E> = StdResult<T, Report<E>>;

/// An trait providing [`Report`]-related extensions for [`Result`](std::result::Result).
pub trait ResultExt: Sized {
    #[track_caller]
    /// Attach a [`Display`]-able type to the error [`Report`]'s stack trace.
    fn attach_printable<P: Display + Send + Sync + 'static>(self, printable: P) -> Self;

    #[track_caller]
    /// Attach the message "while parsing type T" to the error [`Report`]'s stack trace.
    fn while_parsing_type(self) -> Self;
}

struct ReportInner {
    location: &'static Location<'static>,
    stack: ReportStack,
}

#[derive(Default)]
struct ReportStack {
    entries: Vec<ReportEntry>,
}

#[derive(derive_more::Display)]
#[display(fmt = "{message} at {location}")]
struct ReportEntry {
    message: Box<dyn Display + Send + Sync + 'static>,
    location: &'static Location<'static>,
}

//
// Report impls
//

impl<E> Report<E> {
    /// Get a reference to the underlying error.
    pub fn get_ref(&self) -> &E {
        &self.error
    }

    /// Unwrap this report, returning the underlying error.
    pub fn into_inner(self) -> E {
        self.error
    }

    #[track_caller]
    /// Attach a [`Display`]-able type to the stack trace.
    pub fn attach_printable<P: Display + Send + Sync + 'static>(mut self, message: P) -> Self {
        let entry = ReportEntry { message: Box::new(message), location: Location::caller() };
        self.inner.stack.entries.push(entry);
        self
    }
}

impl<E> From<E> for Report<E> {
    #[track_caller]
    fn from(error: E) -> Self {
        Self { error, inner: Box::new(ReportInner { location: Location::caller(), stack: Default::default() }) }
    }
}

impl<E: Display> Debug for Report<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { error, inner } = self;
        let ReportInner { location, stack } = &**inner;
        write!(f, "{error} at {location}\n{stack}")
    }
}

//
// WhileParsingType impls
//

impl WhileParsingType {
    /// Construct a new [`WhileParsingType`] where the type described is `T`.
    pub fn new<T: ?Sized>() -> Self {
        Self(type_name::<T>())
    }
}

//
// ReportStack impls
//

impl Display for ReportStack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for entry in &self.entries[..self.entries.len().saturating_sub(1)] {
            writeln!(f, " - {entry}")?;
        }
        if let Some(entry) = self.entries.last() {
            write!(f, " - {entry}")?;
        }
        Ok(())
    }
}

//
// ResultExt impls
//

impl<T, E> ResultExt for Result<T, E> {
    #[track_caller]
    fn attach_printable<P: Display + Send + Sync + 'static>(self, printable: P) -> Self {
        match self {
            Ok(value) => Ok(value),
            Err(err) => Err(err.attach_printable(printable)),
        }
    }

    #[track_caller]
    fn while_parsing_type(self) -> Self {
        self.attach_printable(WhileParsingType::new::<T>())
    }
}

impl<T, E: Display> ResultExt for StdResult<T, Error<E>> {
    #[track_caller]
    fn attach_printable<P: Display + Send + Sync + 'static>(self, printable: P) -> Self {
        match self {
            Err(Error::Io(err)) => Err(Error::Io(err)),
            Err(Error::Parse(err)) => Err(Error::Parse(err.attach_printable(printable))),
            _ => self,
        }
    }

    #[track_caller]
    fn while_parsing_type(self) -> Self {
        self.attach_printable(WhileParsingType::new::<T>())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    const TEST_ERROR_DISPLAY: &str = "test error display";
    const TEST_ATTACHMENT: &str = "test attachment";

    #[derive(Debug, thiserror::Error)]
    #[error("{}", TEST_ERROR_DISPLAY)]
    struct TestError;

    fn test_report() -> Report<TestError> {
        report_attach!(TestError, TEST_ATTACHMENT)
    }

    #[test]
    fn test_report_display() {
        assert_eq!(test_report().to_string(), TEST_ERROR_DISPLAY);
    }

    #[test]
    fn test_report_debug() {
        let report_debug = format!("{report:?}", report = test_report());
        assert!(report_debug.starts_with(TEST_ERROR_DISPLAY));
        assert!(report_debug.contains(TEST_ATTACHMENT));
    }
}
