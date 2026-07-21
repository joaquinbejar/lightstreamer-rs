//! Who the session is opened as, and which adapters serve it.
//!
//! The protocol groups `LS_user`, `LS_password` and `LS_adapter_set` together
//! as parameters of one request [`docs/spec/03-requests.md` §2.1]. This module
//! splits them into two types, which is a choice of this crate rather than a
//! protocol requirement: the first two are a secret the caller supplies and
//! this crate must never leak, the third is a routing label with no secrecy at
//! all. Keeping them apart means the redacting `Debug` on [`Credentials`]
//! never has to hide something a caller would rather see.

use std::fmt;

use crate::config::ConfigError;

/// The name of the Adapter Set that will serve a session.
///
/// A Lightstreamer server hosts one or more **Adapter Sets** — a Metadata
/// Adapter plus the Data Adapters it fronts. The name selects which one this
/// session talks to; the server assumes an Adapter Set named `DEFAULT` when
/// none is given [`docs/spec/03-requests.md` §2.1].
///
/// The public demo server used by the specification's own transcripts serves
/// an Adapter Set named `DEMO` [`docs/spec/02-session-lifecycle.md` §10].
///
/// # Examples
///
/// ```
/// use lightstreamer_rs::AdapterSet;
///
/// let adapters = AdapterSet::try_new("DEMO")?;
/// assert_eq!(adapters.as_str(), "DEMO");
/// # Ok::<(), lightstreamer_rs::ConfigError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AdapterSet(String);

impl AdapterSet {
    /// Checks and wraps an Adapter Set name.
    ///
    /// # Errors
    ///
    /// - [`ConfigError::EmptyAdapterSet`] if the name is empty or only
    ///   whitespace. Omitting the Adapter Set entirely and naming it the empty
    ///   string are different requests, and the second is never what a caller
    ///   means; to get the server's `DEFAULT`, leave the name unset.
    /// - [`ConfigError::AdapterSetControlCharacter`] if the name contains an
    ///   ASCII control character. This is a guard of this crate, not a
    ///   protocol rule: the specification gives no grammar for the name, and
    ///   the request encoder would percent-escape a stray `CR` faithfully — but
    ///   a control character in a logical adapter name is a mistake in every
    ///   case, and failing here beats a server-side rejection.
    #[must_use = "a checked adapter-set name does nothing unless it is used"]
    pub fn try_new(name: impl Into<String>) -> Result<Self, ConfigError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(ConfigError::EmptyAdapterSet);
        }
        if name.chars().any(char::is_control) {
            return Err(ConfigError::AdapterSetControlCharacter);
        }
        Ok(Self(name))
    }

    /// The name as it goes on the wire.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AdapterSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The user name and password a session is created with.
///
/// Both are free strings, interpreted and verified by the server's **Metadata
/// Adapter** — this crate never inspects them and imposes no format
/// [`docs/spec/03-requests.md` §2.1]. Many Adapter Sets, including the public
/// demo one, authenticate nobody and accept [`Credentials::anonymous`].
///
/// # Secrecy
///
/// This crate never reads credentials from the environment, from a file, or
/// from anywhere else: you supply them. The password is never logged and never
/// appears in any [`Error`](crate::Error) this crate produces, and the
/// hand-written [`Debug`] implementation redacts it, so a `{:?}` of a config
/// struct that happens to contain one is safe.
///
/// ```
/// use lightstreamer_rs::Credentials;
///
/// let credentials = Credentials::new("alice", "hunter2");
/// assert!(!format!("{credentials:?}").contains("hunter2"));
/// ```
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Credentials {
    user: Option<String>,
    password: Option<String>,
}

impl Credentials {
    /// A user name and password pair.
    #[must_use]
    pub fn new(user: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            user: Some(user.into()),
            password: Some(password.into()),
        }
    }

    /// No user name and no password.
    ///
    /// The Metadata Adapter is still asked to authenticate the session — it
    /// simply receives a null user name, and may well accept it
    /// [`docs/spec/03-requests.md` §2.1]. This is what the public demo server
    /// expects.
    #[must_use]
    #[inline]
    pub const fn anonymous() -> Self {
        Self {
            user: None,
            password: None,
        }
    }

    /// A user name with no password, for Adapter Sets that identify a user
    /// without authenticating one.
    #[must_use]
    pub fn user_only(user: impl Into<String>) -> Self {
        Self {
            user: Some(user.into()),
            password: None,
        }
    }

    /// The configured user name, if any.
    #[must_use]
    #[inline]
    pub fn user(&self) -> Option<&str> {
        self.user.as_deref()
    }

    /// Whether a password was supplied.
    ///
    /// There is deliberately no accessor that returns the password itself: it
    /// goes to the server and nowhere else.
    #[must_use]
    #[inline]
    pub const fn has_password(&self) -> bool {
        self.password.is_some()
    }

    /// Splits into the parts the session layer needs.
    ///
    /// Internal, so the password leaves this type only on its way to the wire.
    pub(crate) fn into_parts(self) -> (Option<String>, Option<String>) {
        (self.user, self.password)
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("user", &self.user)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_adapter_set_accepts_a_plain_name() {
        assert!(matches!(
            AdapterSet::try_new("DEMO"),
            Ok(set) if set.as_str() == "DEMO"
        ));
    }

    #[test]
    fn test_adapter_set_rejects_an_empty_name() {
        assert!(matches!(
            AdapterSet::try_new(""),
            Err(ConfigError::EmptyAdapterSet)
        ));
        assert!(matches!(
            AdapterSet::try_new("  \t "),
            Err(ConfigError::EmptyAdapterSet)
        ));
    }

    #[test]
    fn test_adapter_set_rejects_a_control_character() {
        assert!(matches!(
            AdapterSet::try_new("DE\r\nMO"),
            Err(ConfigError::AdapterSetControlCharacter)
        ));
        assert!(matches!(
            AdapterSet::try_new("DE\u{0}MO"),
            Err(ConfigError::AdapterSetControlCharacter)
        ));
    }

    #[test]
    fn test_credentials_debug_redacts_the_password() {
        let credentials = Credentials::new("alice", "hunter2");
        let rendered = format!("{credentials:?}");
        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(rendered.contains("<redacted>"), "{rendered}");
        assert!(rendered.contains("alice"), "{rendered}");
    }

    #[test]
    fn test_credentials_anonymous_carries_nothing() {
        let credentials = Credentials::anonymous();
        assert_eq!(credentials.user(), None);
        assert!(!credentials.has_password());
    }

    #[test]
    fn test_credentials_user_only_has_no_password() {
        let credentials = Credentials::user_only("alice");
        assert_eq!(credentials.user(), Some("alice"));
        assert!(!credentials.has_password());
    }
}
