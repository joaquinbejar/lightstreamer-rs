/// Unified error type for the Lightstreamer client library.
///
/// This enum consolidates all error types that can occur during Lightstreamer
/// operations, providing typed error handling throughout the library.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LightstreamerError {
    /// An illegal or inappropriate argument was passed to a method.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// A method was invoked at an illegal or inappropriate time,
    /// or the object is in an inappropriate state.
    #[error("invalid state: {0}")]
    InvalidState(String),

    /// A connection-related error occurred.
    #[error("connection error: {0}")]
    Connection(String),

    /// A subscription-related error occurred.
    #[error("subscription error: {0}")]
    Subscription(String),

    /// A configuration-related error occurred.
    #[error("configuration error: {0}")]
    Configuration(String),

    /// A protocol-related error occurred during message parsing or handling.
    #[error("protocol error: {0}")]
    Protocol(String),

    /// A channel communication error occurred.
    #[error("channel error: {0}")]
    Channel(String),

    /// A parsing error occurred.
    #[error("parse error: {0}")]
    Parse(String),

    /// An authentication error occurred.
    #[error("authentication error: {0}")]
    Authentication(String),

    /// A network error occurred.
    #[error("network error: {0}")]
    Network(String),

    /// A timeout occurred.
    #[error("timeout: {0}")]
    Timeout(String),

    /// A lock acquisition error occurred.
    #[error("lock error: {0}")]
    Lock(String),
}

impl LightstreamerError {
    /// Creates an invalid argument error.
    #[cold]
    #[must_use]
    #[inline]
    pub fn invalid_argument(msg: impl Into<String>) -> Self {
        Self::InvalidArgument(msg.into())
    }

    /// Creates an invalid state error.
    #[cold]
    #[must_use]
    #[inline]
    pub fn invalid_state(msg: impl Into<String>) -> Self {
        Self::InvalidState(msg.into())
    }

    /// Creates a connection error.
    #[cold]
    #[must_use]
    #[inline]
    pub fn connection(msg: impl Into<String>) -> Self {
        Self::Connection(msg.into())
    }

    /// Creates a subscription error.
    #[cold]
    #[must_use]
    #[inline]
    pub fn subscription(msg: impl Into<String>) -> Self {
        Self::Subscription(msg.into())
    }

    /// Creates a configuration error.
    #[cold]
    #[must_use]
    #[inline]
    pub fn configuration(msg: impl Into<String>) -> Self {
        Self::Configuration(msg.into())
    }

    /// Creates a protocol error.
    #[cold]
    #[must_use]
    #[inline]
    pub fn protocol(msg: impl Into<String>) -> Self {
        Self::Protocol(msg.into())
    }

    /// Creates a channel error.
    #[cold]
    #[must_use]
    #[inline]
    pub fn channel(msg: impl Into<String>) -> Self {
        Self::Channel(msg.into())
    }

    /// Creates a parse error.
    #[cold]
    #[must_use]
    #[inline]
    pub fn parse(msg: impl Into<String>) -> Self {
        Self::Parse(msg.into())
    }

    /// Creates a lock error.
    #[cold]
    #[must_use]
    #[inline]
    pub fn lock(msg: impl Into<String>) -> Self {
        Self::Lock(msg.into())
    }
}

/// Type alias for Results using LightstreamerError.
pub type Result<T> = std::result::Result<T, LightstreamerError>;

impl From<String> for LightstreamerError {
    fn from(s: String) -> Self {
        LightstreamerError::InvalidArgument(s)
    }
}

impl From<&str> for LightstreamerError {
    fn from(s: &str) -> Self {
        LightstreamerError::InvalidArgument(s.to_string())
    }
}

impl From<Box<dyn std::error::Error>> for LightstreamerError {
    fn from(e: Box<dyn std::error::Error>) -> Self {
        LightstreamerError::InvalidState(e.to_string())
    }
}

impl From<Box<dyn std::error::Error + Send + Sync>> for LightstreamerError {
    fn from(e: Box<dyn std::error::Error + Send + Sync>) -> Self {
        LightstreamerError::InvalidState(e.to_string())
    }
}

/// Legacy exception type for backward compatibility.
/// Prefer using `LightstreamerError::InvalidArgument` directly.
#[deprecated(
    since = "0.3.0",
    note = "Use LightstreamerError::InvalidArgument instead"
)]
pub type IllegalArgumentException = LightstreamerError;

/// Legacy exception type for backward compatibility.
/// Prefer using `LightstreamerError::InvalidState` directly.
#[deprecated(since = "0.3.0", note = "Use LightstreamerError::InvalidState instead")]
pub type IllegalStateException = LightstreamerError;

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn test_invalid_argument_error() -> Result<()> {
        let error = LightstreamerError::invalid_argument("Test error message");

        // Test Debug implementation
        let debug_output = format!("{:?}", error);
        assert!(debug_output.contains("InvalidArgument"));
        assert!(debug_output.contains("Test error message"));

        // Test Display implementation
        assert_eq!(error.to_string(), "invalid argument: Test error message");

        Ok(())
    }

    #[test]
    fn test_invalid_state_error() -> Result<()> {
        let error = LightstreamerError::invalid_state("Test state error");

        // Test Debug implementation
        let debug_output = format!("{:?}", error);
        assert!(debug_output.contains("InvalidState"));
        assert!(debug_output.contains("Test state error"));

        // Test Display implementation
        assert_eq!(error.to_string(), "invalid state: Test state error");

        Ok(())
    }

    #[test]
    fn test_all_error_variants() -> Result<()> {
        let errors = vec![
            (
                LightstreamerError::connection("conn"),
                "connection error: conn",
            ),
            (
                LightstreamerError::subscription("sub"),
                "subscription error: sub",
            ),
            (
                LightstreamerError::configuration("cfg"),
                "configuration error: cfg",
            ),
            (
                LightstreamerError::protocol("proto"),
                "protocol error: proto",
            ),
            (LightstreamerError::channel("chan"), "channel error: chan"),
            (LightstreamerError::parse("parse"), "parse error: parse"),
            (LightstreamerError::lock("lock"), "lock error: lock"),
        ];

        for (error, expected_msg) in errors {
            assert_eq!(error.to_string(), expected_msg);
        }

        Ok(())
    }

    #[test]
    fn test_error_propagation() -> Result<()> {
        fn function_that_fails() -> Result<()> {
            Err(LightstreamerError::invalid_argument("Test propagation"))
        }

        fn propagate_error() -> Result<()> {
            function_that_fails()?;
            Ok(())
        }

        let result = propagate_error();
        assert!(result.is_err());
        if let Err(error) = result {
            assert_eq!(error.to_string(), "invalid argument: Test propagation");
        }

        Ok(())
    }

    #[test]
    fn test_error_conversion_to_box_dyn() -> Result<()> {
        let error = LightstreamerError::invalid_argument("Test conversion");
        let boxed_error: Box<dyn Error> = Box::new(error);
        assert_eq!(boxed_error.to_string(), "invalid argument: Test conversion");

        Ok(())
    }

    #[test]
    fn test_error_as_return_type() -> Result<()> {
        fn may_fail(value: i32) -> Result<()> {
            if value < 0 {
                Err(LightstreamerError::invalid_argument(
                    "Value cannot be negative",
                ))
            } else if value > 100 {
                Err(LightstreamerError::invalid_state("Value too large"))
            } else {
                Ok(())
            }
        }

        let result = may_fail(-10);
        assert!(result.is_err());
        if let Err(error) = result {
            assert_eq!(
                error.to_string(),
                "invalid argument: Value cannot be negative"
            );
        }

        let result = may_fail(150);
        assert!(result.is_err());
        if let Err(error) = result {
            assert_eq!(error.to_string(), "invalid state: Value too large");
        }

        let result = may_fail(50);
        assert!(result.is_ok());

        Ok(())
    }

    #[test]
    fn test_error_trait_methods() -> Result<()> {
        let error = LightstreamerError::invalid_argument("Test error");
        let error_ref: &dyn Error = &error;

        // Test source method (should be None since we don't have a cause)
        assert!(error_ref.source().is_none());

        Ok(())
    }

    #[test]
    fn test_error_equality() -> Result<()> {
        let error1 = LightstreamerError::invalid_argument("test");
        let error2 = LightstreamerError::invalid_argument("test");
        let error3 = LightstreamerError::invalid_state("test");

        assert_eq!(error1, error2);
        assert_ne!(error1, error3);

        Ok(())
    }

    #[test]
    fn test_error_clone() -> Result<()> {
        let error = LightstreamerError::connection("test connection");
        let cloned = error.clone();
        assert_eq!(error, cloned);

        Ok(())
    }
}
