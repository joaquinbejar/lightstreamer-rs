//! Sending a message *to* the server.
//!
//! TLCP is not only a download: a client may push a text message up to the
//! server's Metadata Adapter, which interprets it however the application
//! wants — a chat line, an order, a command
//! [`docs/spec/03-requests.md` §12]. This crate carries the text and reports
//! what became of it; it has no opinion on the content.

use std::num::NonZeroU32;
use std::time::Duration;

use crate::config::ConfigError;
use crate::error::ServerError;
use crate::protocol::request::SequenceName as WireSequenceName;
use crate::protocol::response::MessageSequence;
use crate::session::{MessageResult as WireResult, OutgoingMessage};

/// The name of an ordered message sequence
/// [`docs/spec/03-requests.md` §12.1].
///
/// Messages sharing a sequence are delivered to the Metadata Adapter **in
/// order of their progressive number**, and a message in no sequence is
/// processed as soon as it arrives, possibly alongside others. A sequence is
/// therefore how an application says "these messages must not overtake each
/// other".
///
/// # Examples
///
/// ```
/// use lightstreamer_rs::SequenceName as Sequence;
///
/// let orders = Sequence::try_new("ORDERS")?;
/// assert_eq!(orders.as_str(), "ORDERS");
/// # Ok::<(), lightstreamer_rs::ConfigError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SequenceName(WireSequenceName);

impl SequenceName {
    /// Checks and wraps a sequence name.
    ///
    /// # Errors
    ///
    /// [`ConfigError::InvalidSequenceName`] if the name is empty, contains
    /// anything but ASCII letters, digits and underscores, or is
    /// `UNORDERED_MESSAGES` — which earlier versions of the protocol used and
    /// which this one reserves, so the server would refuse it
    /// [`docs/spec/03-requests.md` §12.1].
    ///
    /// # Examples
    ///
    /// ```
    /// use lightstreamer_rs::{ConfigError, SequenceName as Sequence};
    ///
    /// assert!(matches!(
    ///     Sequence::try_new("UNORDERED_MESSAGES"),
    ///     Err(ConfigError::InvalidSequenceName { .. })
    /// ));
    /// ```
    #[must_use = "a checked sequence name does nothing unless it is used"]
    pub fn try_new(name: impl Into<String>) -> Result<Self, ConfigError> {
        let name = name.into();
        WireSequenceName::try_new(name.clone())
            .map(Self)
            .map_err(|_| ConfigError::InvalidSequenceName { name })
    }

    /// The name as it goes on the wire.
    #[must_use]
    #[inline]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// A message on its way to the server's Metadata Adapter.
///
/// There are two shapes, and the difference is not cosmetic:
///
/// - [`Message::new`] — numbered, so the server can deduplicate it and so its
///   outcome can be reported back to you.
/// - [`Message::fire_and_forget`] — unnumbered and unreported. The server
///   cannot tell a resend from a new message and you will never learn whether
///   it worked.
///
/// # The number is yours to choose
///
/// The progressive starts at 1 and orders the messages of a sequence. This
/// crate deliberately does **not** allocate it: the server tracks the
/// numbering per session, so a counter kept here would silently restart the
/// moment a session was replaced — exactly when a caller most needs to know
/// what happened to a message. You own the numbering, and this crate never
/// rewrites it.
///
/// # Examples
///
/// ```
/// use std::num::NonZeroU32;
/// use lightstreamer_rs::{Message, SequenceName as Sequence};
///
/// let message = Message::new("BUY 100", NonZeroU32::MIN)
///     .in_sequence(Sequence::try_new("ORDERS")?);
/// # Ok::<(), lightstreamer_rs::ConfigError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    inner: OutgoingMessage,
}

impl Message {
    /// A numbered message whose outcome will be reported.
    ///
    /// The outcome arrives on the session event stream as
    /// [`SessionEvent::Message`](crate::SessionEvent::Message).
    #[must_use = "builders do nothing unless the message is sent"]
    pub fn new(text: impl Into<String>, progressive: NonZeroU32) -> Self {
        Self {
            inner: OutgoingMessage::numbered(text, progressive),
        }
    }

    /// A message with no number and no outcome report.
    ///
    /// The protocol makes these two properties inseparable: a message may omit
    /// its progressive only if it also declines the outcome notification
    /// [`docs/spec/03-requests.md` §12.1]. So this is genuinely
    /// fire-and-forget — no ordering, no deduplication, no confirmation, and
    /// no error if the Metadata Adapter rejects it.
    #[must_use = "builders do nothing unless the message is sent"]
    pub fn fire_and_forget(text: impl Into<String>) -> Self {
        Self {
            inner: OutgoingMessage::fire_and_forget(text),
        }
    }

    /// Places the message in an ordered sequence.
    ///
    /// Messages of one sequence are processed in progressive order; messages
    /// of different sequences do not constrain each other.
    #[must_use = "builders do nothing unless the message is sent"]
    pub fn in_sequence(mut self, sequence: SequenceName) -> Self {
        self.inner = self.inner.in_sequence(sequence.0);
        self
    }

    /// How long the server may wait for earlier messages of the same sequence
    /// before giving up on them and processing this one
    /// [`docs/spec/03-requests.md` §12.1].
    ///
    /// Only meaningful alongside [`Message::in_sequence`]. When the wait
    /// expires the skipped progressives are reported as failures with
    /// Appendix B codes `38` and `39` [`docs/spec/05-error-codes.md` §2].
    ///
    /// # Errors
    ///
    /// [`ConfigError::DurationTooLarge`] if the duration does not fit a whole
    /// number of milliseconds.
    pub fn with_max_wait(mut self, wait: Duration) -> Result<Self, ConfigError> {
        let millis = u64::try_from(wait.as_millis())
            .map_err(|_| ConfigError::DurationTooLarge { field: "max_wait" })?;
        self.inner.max_wait_millis = Some(millis);
        Ok(self)
    }

    /// The text that will be delivered.
    #[must_use]
    #[inline]
    pub fn text(&self) -> &str {
        &self.inner.message
    }

    /// Hands the payload to the session layer.
    pub(crate) fn into_outgoing(self) -> OutgoingMessage {
        self.inner
    }
}

/// What became of a message you sent.
///
/// A message can fail in three different places and this crate keeps them
/// apart, because the right response differs: a message that never left this
/// client can be retried immediately, one the *server* refused will be refused
/// again, and one the *Metadata Adapter* rejected is an application-level
/// answer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MessageResult {
    /// The Metadata Adapter processed the message without raising
    /// [`docs/spec/04-notifications.md` §4.1].
    Delivered {
        /// The adapter's own response text, empty when it supplied none. Its
        /// meaning is entirely the application's.
        response: String,
    },

    /// The message reached the server but was not delivered, or the Metadata
    /// Adapter failed while processing it
    /// [`docs/spec/04-notifications.md` §4.2].
    ///
    /// Appendix B codes `32`, `33`, `38` and `39` concern the sequence
    /// numbering specifically — a progressive already used, or one skipped
    /// because the server stopped waiting for it
    /// [`docs/spec/05-error-codes.md` §2].
    Failed(ServerError),

    /// The server refused the request outright, so no delivery outcome will
    /// ever follow [`docs/spec/03-requests.md` §13.2].
    Refused(ServerError),

    /// The message never reached the server.
    ///
    /// **Messages are not buffered and not resent.** A subscription is a
    /// statement of desired state and is safely re-created on whatever session
    /// comes next; a message is a one-shot side effect whose ordering and
    /// deduplication the server tracks *per session*. Replaying one across a
    /// session replacement would restart the numbering the server
    /// deduplicates against, so this crate fails the send rather than promise
    /// an ordering it cannot keep. Whether to retry is yours to decide,
    /// because you are the only one who can decide it correctly.
    NotSent {
        /// What stopped it. Never contains a credential.
        reason: String,
    },
}

impl From<WireResult> for MessageResult {
    fn from(result: WireResult) -> Self {
        match result {
            WireResult::Done { response } => Self::Delivered { response },
            WireResult::Failed { cause } => Self::Failed(cause.into()),
            WireResult::Refused { cause } => Self::Refused(cause.into()),
            WireResult::NotSent { reason } => Self::NotSent { reason },
        }
    }
}

/// The report of one message's fate.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct MessageOutcome {
    /// The sequence the message was sent in, or `None` when it was sent in
    /// none.
    pub sequence: Option<String>,
    /// The progressive that identifies the message, or `None` for a
    /// fire-and-forget one — which can only ever be reported as
    /// [`MessageResult::NotSent`] or [`MessageResult::Refused`].
    pub progressive: Option<u64>,
    /// What became of it.
    pub result: MessageResult,
}

impl MessageOutcome {
    /// Builds the public report from the session layer's.
    pub(crate) fn new(
        sequence: MessageSequence,
        progressive: Option<u64>,
        result: WireResult,
    ) -> Self {
        Self {
            sequence: match sequence {
                MessageSequence::Named(name) => Some(name),
                MessageSequence::Unspecified => None,
            },
            progressive,
            result: result.into(),
        }
    }

    /// Whether the Metadata Adapter processed the message.
    #[must_use]
    #[inline]
    pub const fn is_delivered(&self) -> bool {
        matches!(self.result, MessageResult::Delivered { .. })
    }

    /// Assembles one field by field, for [`crate::test_util`].
    #[cfg(feature = "test-util")]
    #[must_use]
    pub(crate) const fn from_parts(
        sequence: Option<String>,
        progressive: Option<u64>,
        result: MessageResult,
    ) -> Self {
        Self {
            sequence,
            progressive,
            result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::ServerCause;

    #[test]
    fn test_sequence_name_accepts_an_alphanumeric_identifier() {
        assert!(matches!(
            SequenceName::try_new("ORDERS_1"),
            Ok(name) if name.as_str() == "ORDERS_1"
        ));
    }

    #[test]
    fn test_sequence_name_rejects_the_reserved_identifier() {
        assert!(matches!(
            SequenceName::try_new("UNORDERED_MESSAGES"),
            Err(ConfigError::InvalidSequenceName { name }) if name == "UNORDERED_MESSAGES"
        ));
    }

    #[test]
    fn test_sequence_name_rejects_an_empty_or_punctuated_name() {
        assert!(matches!(
            SequenceName::try_new(""),
            Err(ConfigError::InvalidSequenceName { .. })
        ));
        assert!(matches!(
            SequenceName::try_new("with space"),
            Err(ConfigError::InvalidSequenceName { .. })
        ));
        assert!(matches!(
            SequenceName::try_new("dash-ed"),
            Err(ConfigError::InvalidSequenceName { .. })
        ));
    }

    #[test]
    fn test_fire_and_forget_carries_no_progressive() {
        let message = Message::fire_and_forget("hello");
        assert_eq!(message.text(), "hello");
        assert_eq!(message.into_outgoing().msg_prog, None);
    }

    #[test]
    fn test_a_numbered_message_keeps_the_callers_number() {
        let outgoing = Message::new("hello", NonZeroU32::MIN).into_outgoing();
        assert_eq!(outgoing.msg_prog, Some(NonZeroU32::MIN));
    }

    #[test]
    fn test_max_wait_rejects_a_duration_that_does_not_fit_milliseconds() {
        assert!(matches!(
            Message::fire_and_forget("x").with_max_wait(Duration::MAX),
            Err(ConfigError::DurationTooLarge { field: "max_wait" })
        ));
    }

    #[test]
    fn test_outcome_distinguishes_where_a_message_failed() {
        let delivered = MessageOutcome::new(
            MessageSequence::Named("ORDERS".to_owned()),
            Some(1),
            WireResult::Done {
                response: "ok".to_owned(),
            },
        );
        assert!(delivered.is_delivered());
        assert_eq!(delivered.sequence.as_deref(), Some("ORDERS"));

        let refused = MessageOutcome::new(
            MessageSequence::Unspecified,
            None,
            WireResult::Refused {
                cause: ServerCause {
                    code: 65,
                    message: "Syntax error".to_owned(),
                },
            },
        );
        assert!(!refused.is_delivered());
        assert_eq!(refused.sequence, None);
        assert!(matches!(refused.result, MessageResult::Refused(cause) if cause.code() == 65));

        let never_sent = MessageOutcome::new(
            MessageSequence::Unspecified,
            None,
            WireResult::NotSent {
                reason: "no session".to_owned(),
            },
        );
        assert!(matches!(never_sent.result, MessageResult::NotSent { .. }));
    }
}
