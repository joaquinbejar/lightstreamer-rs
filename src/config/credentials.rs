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

use crate::REDACTED;
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
/// from anywhere else: you supply them. Neither half is ever logged, neither
/// appears in any [`Error`](crate::Error) this crate produces, and the
/// hand-written [`Debug`] implementation redacts **both**, so a `{:?}` of a
/// config struct that happens to contain one is safe.
///
/// The user name is redacted as well as the password because it is not always
/// the innocuous half: adapters routinely authenticate with an account
/// identifier or an API key as the user name, and a `Debug` that printed it
/// would put that in a log by accident. When you want it, ask for it —
/// [`Credentials::user`] returns it and says so at the call site.
///
/// ```
/// use lightstreamer_rs::Credentials;
///
/// let credentials = Credentials::new("alice", "hunter2");
/// let rendered = format!("{credentials:?}");
/// assert!(!rendered.contains("hunter2"));
/// assert!(!rendered.contains("alice"));
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
    /// Reports which halves are set and neither of their values.
    ///
    /// Written by hand rather than derived so that no `{:?}` anywhere — this
    /// crate's, the caller's, or a panic message's — can print a credential.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("user", &self.user.as_ref().map(|_| REDACTED))
            .field("password", &self.password.as_ref().map(|_| REDACTED))
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
    fn test_credentials_debug_redacts_both_halves() {
        // A-01: neither half may reach a `{:?}`. The user name is not the
        // innocuous one — with many adapters it *is* the account identifier.
        let credentials = Credentials::new("alice", "hunter2");
        let rendered = format!("{credentials:?}");
        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(!rendered.contains("alice"), "{rendered}");
        assert!(rendered.contains(REDACTED), "{rendered}");
    }

    #[test]
    fn test_credentials_debug_still_says_which_halves_are_set() {
        // Redaction must not cost the diagnostic value: "is a password set at
        // all" is the question a caller debugging an authentication failure
        // actually asks.
        let anonymous = format!("{:?}", Credentials::anonymous());
        assert!(anonymous.contains("None"), "{anonymous}");
        let user_only = format!("{:?}", Credentials::user_only("alice"));
        assert!(user_only.contains(REDACTED), "{user_only}");
        assert!(user_only.contains("None"), "{user_only}");
    }

    #[test]
    fn test_a_config_holding_credentials_leaks_nothing_through_debug() {
        // The realistic path: nobody formats `Credentials`, they format the
        // whole configuration.
        let Ok(address) = crate::ServerAddress::try_new("wss://push.example.com") else {
            unreachable!("the fixture address is valid");
        };
        let built = crate::ClientConfig::builder(address)
            .with_credentials(Credentials::new("alice", "hunter2"))
            .build();
        match built {
            Ok(config) => {
                let rendered = format!("{config:?}");
                assert!(!rendered.contains("hunter2"), "{rendered}");
                assert!(!rendered.contains("alice"), "{rendered}");
            }
            Err(error) => panic!("the fixture configuration is valid: {error}"),
        }
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
