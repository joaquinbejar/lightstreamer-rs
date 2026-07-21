//! The pure TLCP wire layer: bytes in, typed values out.
//!
//! This module performs **no I/O and no async work**. It has no knowledge of
//! sockets, transports, timers, or sessions, which is what makes every wire
//! behavior here exhaustively testable from fixtures lifted out of the
//! specification. A transport type appearing in this module is a layering
//! violation.
//!
//! Source: `docs/spec/01-foundations.md` (line format, escaping, the
//! normative parsing algorithm), `docs/spec/03-requests.md` (request
//! encoding), `docs/spec/04-notifications.md` (notification parsing).

pub(crate) mod diff;
pub(crate) mod escaping;
pub(crate) mod request;
pub(crate) mod response;

/// A failure to encode a request or to interpret a server line as TLCP.
///
/// Parsing is total: malformed input produces one of these, never a panic and
/// never a silently dropped field.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ProtocolError {
    /// A line did not carry the number of arguments its tag requires.
    ///
    /// Each tag has a fixed argument count, which is what makes the last
    /// argument allowed to contain commas
    /// [`docs/spec/01-foundations.md` §7.1].
    #[error("tag `{tag}` requires {expected} arguments, found {found}")]
    ArgumentCount {
        /// The tag that was parsed.
        tag: String,
        /// How many arguments that tag requires.
        expected: usize,
        /// How many arguments the line actually carried.
        found: usize,
    },

    /// An argument that must be numeric was not.
    #[error("argument `{argument}` of tag `{tag}` is not a valid number: {value:?}")]
    NotNumeric {
        /// The tag being parsed.
        tag: String,
        /// The name of the offending argument.
        argument: &'static str,
        /// The raw text that failed to parse.
        value: String,
    },

    /// A percent-escape was malformed — a `%` not followed by two hex digits,
    /// or a sequence that does not decode to valid UTF-8
    /// [`docs/spec/01-foundations.md` §7.2].
    #[error("malformed percent-encoding at byte {position}: {reason}")]
    Escaping {
        /// Byte offset of the offending sequence within the input.
        position: usize,
        /// What was wrong with it.
        reason: &'static str,
    },

    /// The line's tag is not one this client recognizes.
    ///
    /// This is **not** treated as fatal by callers: a future server version
    /// must not break an older client. The raw line is preserved so it can be
    /// surfaced or logged intact.
    #[error("unrecognized notification tag `{tag}`")]
    UnknownTag {
        /// The tag as it appeared on the wire.
        tag: String,
        /// The complete line, with its terminator already stripped.
        line: String,
    },

    /// A request could not be encoded because its parameters violate a
    /// constraint the specification places on their combination or on an
    /// individual value [`docs/spec/03-requests.md`].
    #[error("cannot encode `{request}` request: {reason}")]
    Request {
        /// The TLCP request name, e.g. `control` or `create_session`.
        request: &'static str,
        /// Which parameter or combination was rejected, and why.
        reason: String,
    },

    /// A field value could not be decoded against the current item state —
    /// a malformed marker, an unknown diff algorithm, or a diff that does not
    /// apply to the previous value
    /// [`docs/spec/04-notifications.md` §2].
    #[error("cannot decode field value: {reason}")]
    FieldValue {
        /// What was wrong with the value.
        reason: String,
    },
}

/// Builds the error every field-value failure reports.
///
/// A malformed value list is never a panic and never a silently dropped field
/// [`docs/spec/04-notifications.md` §2.3]. Shared by the diff decoders here and
/// by the value-list decoder above them, so both spell the same failure the
/// same way.
#[cold]
#[inline(never)]
pub(crate) fn field_value_error(reason: impl Into<String>) -> ProtocolError {
    ProtocolError::FieldValue {
        reason: reason.into(),
    }
}
