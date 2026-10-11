use std::error::Error;
use std::fmt::{self, Display, Formatter};

/// Formats an error followed by every `source()` cause, for logs.
///
/// Public error messages stay short and safe; logs need the underlying cause,
/// such as the SQLite error behind "SQLite operation failed".
pub struct ErrorChain<'a, E: ?Sized>(pub &'a E);

impl<E: Error + ?Sized> Display for ErrorChain<'_, E> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)?;
        let mut source = self.0.source();
        while let Some(cause) = source {
            write!(formatter, ": {cause}")?;
            source = cause.source();
        }
        Ok(())
    }
}

/// Log a dependency failure with its full cause at the point where it is
/// replaced by a short, cause-free error value.
pub fn log_dependency_failure<E: Error + ?Sized>(context: &str, error: &E) {
    tracing::error!(error = %ErrorChain(error), "{context}");
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::fmt::{self, Display, Formatter};
    use std::io;

    use super::ErrorChain;

    #[derive(Debug)]
    struct Outer(io::Error);

    impl Display for Outer {
        fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
            formatter.write_str("storage failed")
        }
    }

    impl Error for Outer {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn includes_every_cause() {
        let error = Outer(io::Error::other("disk full"));
        assert_eq!(ErrorChain(&error).to_string(), "storage failed: disk full");
        let boxed: Box<dyn Error + Send + Sync> = Box::new(Outer(io::Error::other("locked")));
        assert_eq!(
            ErrorChain(boxed.as_ref()).to_string(),
            "storage failed: locked"
        );
    }
}
