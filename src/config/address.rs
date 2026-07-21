//! The address of the Lightstreamer server.

use std::fmt;

use crate::config::ConfigError;

/// The separator between a URL scheme and its authority.
const SCHEME_SEPARATOR: &str = "://";

/// The schemes that select a plain-text connection.
const INSECURE_SCHEMES: [&str; 2] = ["ws", "http"];

/// The schemes that select a TLS connection.
const SECURE_SCHEMES: [&str; 2] = ["wss", "https"];

/// The characters that end the authority and begin a path, a query or a
/// fragment.
const AUTHORITY_TERMINATORS: [char; 3] = ['/', '?', '#'];

/// The address of a Lightstreamer server, parsed and checked at construction.
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
/// | `wss://[::1]:8080` | an IPv6 literal, which must be bracketed |
///
/// # What an address may and may not contain
///
/// The address is a **scheme, a host and an optional port**, optionally
/// followed by a base path. Everything else is refused here rather than
/// somewhere further down, where the failure would be a connection error
/// instead of a configuration one:
///
/// | Rejected | Why |
/// |---|---|
/// | `wss://user:secret@host` | userinfo is a credential, and a credential in an address ends up in every log line that mentions the address |
/// | `wss://host?a=b`, `wss://host#f` | TLCP defines no query or fragment on the endpoint; carrying one would silently change what is requested |
/// | `wss://host/a/../b` | a `..` segment makes the composed endpoint depend on path normalisation this crate does not perform |
/// | `wss://host:0`, `wss://host:99999` | not a port |
/// | `wss:// host` | not a host |
///
/// # The endpoint path
///
/// The TLCP endpoint path is **not** part of the address: this crate appends
/// the `/lightstreamer` path the protocol fixes
/// [`docs/spec/01-foundations.md` §6.2.1]. An address that already ends in
/// `/lightstreamer` is accepted and not doubled. A base path is accepted too,
/// for a server published behind a reverse proxy, and the fixed path is
/// appended to it — `https://example.com/push` reaches
/// `/push/lightstreamer`.
///
/// # Canonical form
///
/// [`ServerAddress::as_str`] returns the address with the scheme lowercased
/// and any trailing `/` removed; the host is left in the caller's own
/// spelling. Because userinfo is refused, the canonical form is also the
/// redacted form: it is safe to log.
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
    /// The canonical address: scheme lowercased, no trailing `/`, and — by
    /// construction — no userinfo, query or fragment to redact.
    text: String,
    /// Whether the scheme selects TLS.
    secure: bool,
}

impl ServerAddress {
    /// Parses and checks a server address.
    ///
    /// # Errors
    ///
    /// - [`ConfigError::EmptyServerAddress`] if the text is empty or only
    ///   whitespace.
    /// - [`ConfigError::MissingScheme`] if there is no `://`.
    /// - [`ConfigError::UnsupportedScheme`] if the scheme is not one of `ws`,
    ///   `wss`, `http` or `https`.
    /// - [`ConfigError::MissingHost`] if nothing follows the scheme.
    /// - [`ConfigError::AddressHasUserinfo`] if the authority carries a
    ///   `user:password@` prefix.
    /// - [`ConfigError::InvalidHost`] if the host is empty or contains
    ///   whitespace, a control character, or an unbracketed `:`.
    /// - [`ConfigError::InvalidPort`] if the port is not a number in `1..=65535`.
    /// - [`ConfigError::InvalidAddressPath`] if a query, a fragment, a `..`
    ///   segment, whitespace or a control character follows the authority.
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
    ///
    /// // A credential in an address would reach every log line that mentions it.
    /// assert!(matches!(
    ///     ServerAddress::try_new("wss://alice:hunter2@push.example.com"),
    ///     Err(ConfigError::AddressHasUserinfo)
    /// ));
    /// ```
    #[must_use = "a checked address does nothing unless it is used"]
    pub fn try_new(address: impl Into<String>) -> Result<Self, ConfigError> {
        let address = address.into();
        let trimmed = address.trim();
        if trimmed.is_empty() {
            return Err(ConfigError::EmptyServerAddress);
        }

        let Some((scheme, rest)) = trimmed.split_once(SCHEME_SEPARATOR) else {
            // Redacted even here. Without a scheme there is no authority to
            // find a userinfo in, but `alice:hunter2@host` is exactly the
            // shape a caller who forgot the scheme writes, and an error
            // message is a thing people paste into issues.
            return Err(ConfigError::MissingScheme {
                address: without_userinfo(trimmed),
            });
        };

        let scheme = scheme.to_ascii_lowercase();
        let secure = if SECURE_SCHEMES.contains(&scheme.as_str()) {
            true
        } else if INSECURE_SCHEMES.contains(&scheme.as_str()) {
            false
        } else {
            return Err(ConfigError::UnsupportedScheme { scheme });
        };

        // The authority ends where the path, the query or the fragment
        // begins; a userinfo, if there is one, is inside it.
        let split = rest.find(AUTHORITY_TERMINATORS).unwrap_or(rest.len());
        let authority = rest.get(..split).unwrap_or(rest);
        let path = rest.get(split..).unwrap_or("");

        if authority.is_empty() {
            return Err(ConfigError::MissingHost {
                address: without_userinfo(trimmed),
            });
        }
        if authority.contains('@') {
            return Err(ConfigError::AddressHasUserinfo);
        }
        check_authority(authority)?;
        let path = check_path(path)?;

        Ok(Self {
            text: format!("{scheme}{SCHEME_SEPARATOR}{authority}{path}"),
            secure,
        })
    }

    /// The address in canonical form: scheme lowercased, no trailing `/`, and
    /// nothing in it that needs redacting.
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

/// Replaces anything before an `@` with the redaction marker.
///
/// Used on the addresses that go **into error messages**, so that a rejected
/// address cannot carry a credential into whatever log the error reaches. An
/// address that gets as far as being stored has already been refused if it had
/// a userinfo at all.
fn without_userinfo(address: &str) -> String {
    let (scheme, rest) = match address.split_once(SCHEME_SEPARATOR) {
        Some((scheme, rest)) => (format!("{scheme}{SCHEME_SEPARATOR}"), rest),
        None => (String::new(), address),
    };
    // Userinfo precedes the first `@`, which itself precedes the path.
    let end = rest.find(AUTHORITY_TERMINATORS).unwrap_or(rest.len());
    let authority = rest.get(..end).unwrap_or(rest);
    let tail = rest.get(end..).unwrap_or("");
    match authority.rsplit_once('@') {
        Some((_, host)) => format!("{scheme}{}@{host}{tail}", crate::REDACTED),
        None => address.to_owned(),
    }
}

/// Checks the host and the optional port of an authority with no userinfo.
fn check_authority(authority: &str) -> Result<(), ConfigError> {
    // An IPv6 literal must be bracketed, which is also what makes the last
    // `:` unambiguously the port separator.
    let (host, port) = match authority.strip_prefix('[') {
        Some(bracketed) => match bracketed.split_once(']') {
            Some((host, after)) => (host, after.strip_prefix(':')),
            None => {
                return Err(ConfigError::InvalidHost {
                    host: authority.to_owned(),
                });
            }
        },
        None => match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };

    let bracketed = authority.starts_with('[');
    if host.is_empty()
        || host
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '[' || c == ']' || c == '@')
        || (!bracketed && host.contains(':'))
    {
        return Err(ConfigError::InvalidHost {
            host: host.to_owned(),
        });
    }

    if let Some(port) = port
        && !matches!(port.parse::<u16>(), Ok(1..=u16::MAX))
    {
        return Err(ConfigError::InvalidPort {
            port: port.to_owned(),
        });
    }
    Ok(())
}

/// Checks whatever follows the authority and returns it in canonical form.
///
/// A query and a fragment are refused rather than carried: TLCP fixes the
/// endpoint path and defines nothing that would be passed this way
/// [`docs/spec/01-foundations.md` §6.2.1], so anything here is either a
/// mistake or an attempt to reach a different resource.
fn check_path(path: &str) -> Result<&str, ConfigError> {
    if path.starts_with('?') || path.starts_with('#') {
        return Err(ConfigError::InvalidAddressPath {
            path: path.to_owned(),
        });
    }
    let invalid = path.contains(['?', '#'])
        || path.chars().any(|c| c.is_whitespace() || c.is_control())
        || path.split('/').any(|segment| segment == "..");
    if invalid {
        return Err(ConfigError::InvalidAddressPath {
            path: path.to_owned(),
        });
    }
    Ok(path.trim_end_matches('/'))
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

    // -----------------------------------------------------------------------
    // A-05: every component of the address is parsed and checked
    // -----------------------------------------------------------------------

    #[test]
    fn test_server_address_accepts_every_host_form() {
        for address in [
            "wss://192.0.2.10",
            "wss://192.0.2.10:8080",
            "wss://[2001:db8::1]",
            "wss://[2001:db8::1]:8080",
            "wss://push.example.com",
            "wss://push.example.com:65535",
            "ws://localhost:1",
        ] {
            match ServerAddress::try_new(address) {
                Ok(parsed) => assert_eq!(parsed.as_str(), address),
                Err(error) => panic!("{address} was rejected: {error}"),
            }
        }
    }

    #[test]
    fn test_server_address_rejects_userinfo() {
        // A credential in an address would otherwise reach the connect span,
        // the connect error, and whatever log either is written to.
        for address in [
            "wss://alice:hunter2@push.example.com",
            "wss://alice@push.example.com",
            "wss://alice:hunter2@push.example.com/lightstreamer",
        ] {
            assert!(
                matches!(
                    ServerAddress::try_new(address),
                    Err(ConfigError::AddressHasUserinfo)
                ),
                "{address} was accepted"
            );
        }
    }

    #[test]
    fn test_server_address_never_renders_a_rejected_credential() {
        // Every rejection path that echoes the address must redact it —
        // including the one a caller who forgot the scheme entirely reaches.
        for address in [
            "wss://alice:hunter2@push.example.com",
            "alice:hunter2@push.example.com",
            "wss://alice:hunter2@",
        ] {
            match ServerAddress::try_new(address) {
                Err(error) => {
                    let rendered = error.to_string();
                    assert!(!rendered.contains("hunter2"), "{rendered}");
                    assert!(!rendered.contains("alice"), "{rendered}");
                }
                Ok(parsed) => panic!("{address} was accepted as {parsed}"),
            }
        }
    }

    #[test]
    fn test_server_address_rejects_a_host_that_is_not_one() {
        for address in [
            "wss:// push.example.com",
            "wss://push example.com",
            "wss://push.example.com\u{7}",
            "wss://:8080",
            "wss://[2001:db8::1",
            // An IPv6 literal must be bracketed, or the last `:` is a port.
            "wss://2001:db8::1",
        ] {
            assert!(
                matches!(
                    ServerAddress::try_new(address),
                    Err(ConfigError::InvalidHost { .. })
                ),
                "{address} was accepted"
            );
        }
    }

    #[test]
    fn test_server_address_rejects_a_port_that_is_not_one() {
        for address in [
            "wss://push.example.com:0",
            "wss://push.example.com:65536",
            "wss://push.example.com:99999",
            "wss://push.example.com:http",
            "wss://push.example.com:",
            "wss://push.example.com:-1",
            "wss://[2001:db8::1]:0",
        ] {
            assert!(
                matches!(
                    ServerAddress::try_new(address),
                    Err(ConfigError::InvalidPort { .. })
                ),
                "{address} was accepted"
            );
        }
    }

    #[test]
    fn test_server_address_rejects_a_query_a_fragment_and_a_traversal() {
        for address in [
            "wss://push.example.com?token=secret",
            "wss://push.example.com#fragment",
            "wss://push.example.com/push?a=b",
            "wss://push.example.com/a/../b",
            "wss://push.example.com/..",
            "wss://push.example.com/a b",
        ] {
            assert!(
                matches!(
                    ServerAddress::try_new(address),
                    Err(ConfigError::InvalidAddressPath { .. })
                ),
                "{address} was accepted"
            );
        }
    }

    #[test]
    fn test_server_address_keeps_a_base_path_and_canonicalizes_the_scheme() {
        // A server published behind a reverse proxy: the fixed
        // `/lightstreamer` path is appended to whatever base path was given
        // [`docs/spec/01-foundations.md` §6.2.1].
        for (given, canonical) in [
            (
                "HTTPS://push.example.com/push/",
                "https://push.example.com/push",
            ),
            (
                "wss://push.example.com/lightstreamer",
                "wss://push.example.com/lightstreamer",
            ),
            ("Ws://localhost:8080//", "ws://localhost:8080"),
        ] {
            match ServerAddress::try_new(given) {
                Ok(parsed) => assert_eq!(parsed.as_str(), canonical, "{given}"),
                Err(error) => panic!("{given} was rejected: {error}"),
            }
        }
    }
}
