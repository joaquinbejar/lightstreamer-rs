//! The address of the Lightstreamer server.

use std::fmt;

use crate::config::ConfigError;

/// The separator between a URL scheme and its authority.
const SCHEME_SEPARATOR: &str = "://";

/// The schemes that select a plain-text connection.
const INSECURE_SCHEMES: [&str; 2] = ["ws", "http"];

/// The schemes that select a TLS connection.
const SECURE_SCHEMES: [&str; 2] = ["wss", "https"];

/// The address of a Lightstreamer server, checked at construction.
///
/// A Lightstreamer server is reached at a host and port, and this crate speaks
/// to it over a WebSocket. Both spellings are accepted, because deployments
/// publish their endpoint either way:
///
/// | You write | Meaning |
/// |---|---|
/// | `wss://push.example.com` | WebSocket over TLS |
/// | `https://push.example.com` | the same thing |
/// | `ws://localhost:8080` | WebSocket, no TLS |
/// | `http://localhost:8080` | the same thing |
///
/// The endpoint path is **not** part of the address: this crate appends the
/// `/lightstreamer` path the protocol fixes [`docs/spec/01-foundations.md`
/// §6.2.1]. An address that already ends in `/lightstreamer` is accepted and
/// not doubled.
///
/// # Examples
///
/// ```
/// use lightstreamer_rs::ServerAddress;
///
/// let address = ServerAddress::try_new("https://push.lightstreamer.com")?;
/// assert!(address.is_secure());
/// # Ok::<(), lightstreamer_rs::ConfigError>(())
/// ```
///
/// A missing scheme is rejected rather than guessed at, because guessing
/// `https` for an address the caller meant as plain `http` would silently
/// change the security properties of the connection:
///
/// ```
/// use lightstreamer_rs::ServerAddress;
///
/// assert!(ServerAddress::try_new("push.lightstreamer.com").is_err());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServerAddress {
    /// The address as supplied, trimmed of surrounding whitespace and of any
    /// trailing `/`. Kept in the caller's own spelling so that logs and errors
    /// show what was configured rather than a rewritten form.
    text: String,
    /// Whether the scheme selects TLS.
    secure: bool,
}

impl ServerAddress {
    /// Checks and wraps a server address.
    ///
    /// # Errors
    ///
    /// - [`ConfigError::EmptyServerAddress`] if the text is empty or only
    ///   whitespace.
    /// - [`ConfigError::MissingScheme`] if there is no `://`.
    /// - [`ConfigError::UnsupportedScheme`] if the scheme is not one of `ws`,
    ///   `wss`, `http` or `https`.
    /// - [`ConfigError::MissingHost`] if nothing follows the scheme.
    ///
    /// # Examples
    ///
    /// ```
    /// use lightstreamer_rs::{ConfigError, ServerAddress};
    ///
    /// assert!(matches!(
    ///     ServerAddress::try_new("ftp://push.example.com"),
    ///     Err(ConfigError::UnsupportedScheme { .. })
    /// ));
    /// ```
    #[must_use = "a checked address does nothing unless it is used"]
    pub fn try_new(address: impl Into<String>) -> Result<Self, ConfigError> {
        let address = address.into();
        let trimmed = address.trim();
        if trimmed.is_empty() {
            return Err(ConfigError::EmptyServerAddress);
        }

        let Some((scheme, authority)) = trimmed.split_once(SCHEME_SEPARATOR) else {
            return Err(ConfigError::MissingScheme {
                address: trimmed.to_owned(),
            });
        };

        let lowered = scheme.to_ascii_lowercase();
        let secure = if SECURE_SCHEMES.contains(&lowered.as_str()) {
            true
        } else if INSECURE_SCHEMES.contains(&lowered.as_str()) {
            false
        } else {
            return Err(ConfigError::UnsupportedScheme { scheme: lowered });
        };

        if authority.trim_end_matches('/').is_empty() {
            return Err(ConfigError::MissingHost {
                address: trimmed.to_owned(),
            });
        }

        Ok(Self {
            text: trimmed.trim_end_matches('/').to_owned(),
            secure,
        })
    }

    /// The address as it was configured.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Whether the connection will be encrypted.
    ///
    /// True for the `wss` and `https` schemes, false for `ws` and `http`.
    #[must_use]
    #[inline]
    pub const fn is_secure(&self) -> bool {
        self.secure
    }
}

impl fmt::Display for ServerAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_address_accepts_every_supported_scheme() {
        for (address, secure) in [
            ("ws://localhost:8080", false),
            ("http://localhost:8080", false),
            ("wss://push.example.com", true),
            ("https://push.example.com", true),
        ] {
            match ServerAddress::try_new(address) {
                Ok(parsed) => {
                    assert_eq!(parsed.as_str(), address);
                    assert_eq!(parsed.is_secure(), secure, "{address}");
                }
                Err(error) => panic!("{address} was rejected: {error}"),
            }
        }
    }

    #[test]
    fn test_server_address_scheme_is_case_insensitive() {
        assert!(matches!(
            ServerAddress::try_new("WSS://push.example.com"),
            Ok(address) if address.is_secure()
        ));
    }

    #[test]
    fn test_server_address_trims_surrounding_whitespace() {
        assert!(matches!(
            ServerAddress::try_new("  wss://push.example.com  "),
            Ok(address) if address.as_str() == "wss://push.example.com"
        ));
    }

    #[test]
    fn test_server_address_strips_a_trailing_slash() {
        assert!(matches!(
            ServerAddress::try_new("wss://push.example.com/"),
            Ok(address) if address.as_str() == "wss://push.example.com"
        ));
    }

    #[test]
    fn test_server_address_rejects_an_empty_string() {
        assert!(matches!(
            ServerAddress::try_new("   "),
            Err(ConfigError::EmptyServerAddress)
        ));
    }

    #[test]
    fn test_server_address_rejects_a_missing_scheme() {
        assert!(matches!(
            ServerAddress::try_new("push.example.com"),
            Err(ConfigError::MissingScheme { .. })
        ));
    }

    #[test]
    fn test_server_address_rejects_an_unsupported_scheme() {
        assert!(matches!(
            ServerAddress::try_new("ftp://push.example.com"),
            Err(ConfigError::UnsupportedScheme { scheme }) if scheme == "ftp"
        ));
    }

    #[test]
    fn test_server_address_rejects_a_missing_host() {
        assert!(matches!(
            ServerAddress::try_new("wss://"),
            Err(ConfigError::MissingHost { .. })
        ));
        assert!(matches!(
            ServerAddress::try_new("wss:///"),
            Err(ConfigError::MissingHost { .. })
        ));
    }

    #[test]
    fn test_server_address_displays_what_was_configured() {
        match ServerAddress::try_new("wss://push.example.com:443") {
            Ok(address) => assert_eq!(address.to_string(), "wss://push.example.com:443"),
            Err(error) => panic!("rejected: {error}"),
        }
    }
}
