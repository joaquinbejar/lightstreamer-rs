//! Parsing of the lines the server sends: notifications on the stream
//! connection and responses to requests.
//!
//! Both share one syntax — `<tag>,<argument1>,...,<argumentN>` — so both go
//! through the single entry point [`parse_line`]
//! [`docs/spec/01-foundations.md` §7.1].
//!
//! # The parsing algorithm
//!
//! This module implements the specification's own normative algorithm
//! [`docs/spec/01-foundations.md` §7.4] literally:
//!
//! 1. Trim the trailing line terminator.
//! 2. Find the **first** comma from the left; the text before it is the tag
//!    (or the whole line, when there is no comma at all).
//! 3. Switch on the tag. Each tag has a **fixed argument count**, so the
//!    remainder is split exactly that many times — **the last argument keeps
//!    any further commas**. This is why an error message or a message
//!    response may contain commas without being escaped, and splitting the
//!    remainder on every comma would corrupt them.
//! 4. Percent-decode non-numeric arguments, **except** the last argument of a
//!    real-time update (tag `U`), which has a second-level syntax of its own
//!    and must not be touched in the first pass
//!    [`docs/spec/01-foundations.md` §7.2, §7.4].
//!
//! # Framing is the caller's job
//!
//! [`parse_line`] expects one line with its CR-LF terminator already removed
//! by the transport layer, which owns framing. It is nevertheless tolerant of
//! **one** trailing `\r\n`, `\n`, or `\r`, so that a caller that splits on
//! `\n` and leaves the `\r` behind still parses correctly. Only a single
//! terminator is stripped, so that the raw value list of a `U` keeps every
//! other byte the server sent.
//!
//! # What this module does not do
//!
//! The third argument of a `U` needs the subscription's field schema and the
//! previous value of every field to be decoded, neither of which the pure
//! line parser has. It is therefore carried raw and undecoded in
//! [`Notification::Update::raw_values`]; `src/subscription/update.rs` owns the
//! second-level decoding [`docs/spec/04-notifications.md` §2.2-2.3].
//!
//! MPN (Mobile Push Notifications) is out of scope for v1
//! [`docs/spec/06-mpn.md`]: the tags `MPNREG`, `MPNZERO`, `MPNOK`, `MPNCONF`
//! and `MPNDEL` therefore fall through to
//! [`ProtocolError::UnknownTag`] rather than being silently dropped, so a
//! caller can log or surface them.

use std::str::FromStr;

use crate::protocol::ProtocolError;
use crate::protocol::escaping::percent_decode;

/// One line received from the server, parsed into its typed form.
///
/// Every variant corresponds to exactly one wire tag. Numeric arguments are
/// integers; error-bearing lines carry their code and message as separate
/// fields rather than as one flattened string.
///
/// The complete tag list, with argument counts, is in
/// [`docs/spec/04-notifications.md` §6] for notifications and in
/// [`docs/spec/03-requests.md` §16] for request responses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Notification {
    /// `U` — a real-time update (or a piece of an item's snapshot) for one
    /// item of one subscription [`docs/spec/04-notifications.md` §2].
    ///
    /// Sent continuously once a subscription is active. Whether a given `U`
    /// is snapshot or real-time depends on the subscription mode and on
    /// whether `EOS` has already arrived for the item
    /// [`docs/spec/04-notifications.md` §2.4].
    Update {
        /// The client-generated ID of the subscription this update belongs to
        /// [`docs/spec/04-notifications.md` §2.1].
        subscription_id: u32,
        /// 1-based index of the updated item within the subscription's group.
        /// The item order is decided by the Metadata Adapter
        /// [`docs/spec/04-notifications.md` §2.1].
        item_index: u32,
        /// The pipe-separated field value list, **exactly as it arrived**:
        /// not percent-decoded, not split, not interpreted.
        ///
        /// The first pass must not touch it
        /// [`docs/spec/01-foundations.md` §7.4 note `**`]; decoding it needs
        /// the subscription's schema width and the previous value of every
        /// field, and lives in `src/subscription/update.rs`
        /// [`docs/spec/04-notifications.md` §2.3].
        raw_values: String,
    },

    /// `SUBOK` — a subscription has been activated on the server
    /// [`docs/spec/04-notifications.md` §3.1].
    ///
    /// Sent for every mode except `COMMAND`, which gets `SUBCMD` instead.
    /// Real-time updates for the subscription start after this line.
    SubscriptionOk {
        /// The ID of the subscription that was activated.
        subscription_id: u32,
        /// How many items the subscription resolved to; their order is
        /// decided by the Metadata Adapter.
        item_count: u32,
        /// How many fields the schema resolved to. This is the schema width
        /// the client must use when decoding the value list of subsequent
        /// `U` lines for this subscription.
        field_count: u32,
    },

    /// `SUBCMD` — a `COMMAND`-mode subscription has been activated
    /// [`docs/spec/04-notifications.md` §3.2].
    ///
    /// Replaces `SUBOK` for `COMMAND` mode and additionally reports where the
    /// key and command fields sit in the schema.
    SubscriptionCommandOk {
        /// The ID of the subscription that was activated.
        subscription_id: u32,
        /// How many items the subscription resolved to.
        item_count: u32,
        /// How many fields the schema resolved to.
        field_count: u32,
        /// 1-based index, within the schema, of the field carrying the key.
        key_field_index: u32,
        /// 1-based index, within the schema, of the field carrying the
        /// command.
        command_field_index: u32,
    },

    /// `UNSUB` — a subscription has been deactivated
    /// [`docs/spec/04-notifications.md` §3.4].
    ///
    /// After this line no further update for the subscription will be sent,
    /// so its client-side state may be discarded. Note that updates may still
    /// arrive after the unsubscription *response* and before this
    /// notification.
    Unsubscribed {
        /// The ID of the subscription that was deactivated.
        subscription_id: u32,
    },

    /// `EOS` — the snapshot of one item is complete
    /// [`docs/spec/04-notifications.md` §3.5].
    ///
    /// Sent immediately after the last snapshot-bearing `U` for the item, in
    /// `DISTINCT` and `COMMAND` modes. `MERGE` snapshots are a single `U` and
    /// get no `EOS`; `RAW` has no snapshot at all.
    EndOfSnapshot {
        /// The ID of the subscription the item belongs to.
        subscription_id: u32,
        /// 1-based index of the item whose snapshot has ended.
        item_index: u32,
    },

    /// `CS` — the snapshot of one item has been cleared
    /// [`docs/spec/04-notifications.md` §3.6].
    ///
    /// Sent when the Data Adapter asks the server to clear an item's
    /// snapshot, for `DISTINCT` and `COMMAND` modes, so the client can drop
    /// its accumulated representation of the item. `MERGE` mode instead
    /// receives an ordinary update with null fields.
    ClearSnapshot {
        /// The ID of the subscription the item belongs to.
        subscription_id: u32,
        /// 1-based index of the item whose snapshot has been cleared.
        item_index: u32,
    },

    /// `OV` — the server dropped update events for an item because of its own
    /// buffer limits [`docs/spec/04-notifications.md` §3.7].
    ///
    /// Only possible for `RAW`, for `COMMAND` (ADD and DELETE events), and
    /// for subscriptions that requested unfiltered dispatching.
    Overflow {
        /// The ID of the subscription the item belongs to.
        subscription_id: u32,
        /// 1-based index of the item whose events were dropped.
        item_index: u32,
        /// How many real-time update events were dropped.
        dropped_count: u64,
    },

    /// `CONF` — a subscription's frequency configuration
    /// [`docs/spec/04-notifications.md` §3.8].
    ///
    /// Sent once at the start of a subscription and again after every
    /// successful reconfiguration request, even when the value is unchanged.
    SubscriptionReconfigured {
        /// The ID of the subscription that was configured.
        subscription_id: u32,
        /// The maximum update frequency now in force for every item of the
        /// subscription.
        max_frequency: MaxFrequency,
        /// Whether updates for this subscription may be filtered.
        filtering: FilteringMode,
    },

    /// `MSGDONE` — an upstream message was delivered and processed
    /// [`docs/spec/04-notifications.md` §4.1].
    ///
    /// Sent once the Metadata Adapter's `notifyUserMessage` has returned
    /// without raising.
    MessageDone {
        /// The sequence the message was sent in, or
        /// [`MessageSequence::Unspecified`] when the send request named none.
        sequence: MessageSequence,
        /// The progressive number the client gave the message within its
        /// sequence.
        prog: u64,
        /// The Metadata Adapter's response text. Empty when the adapter
        /// supplied none. Being the last argument, it may contain commas.
        response: String,
    },

    /// `MSGFAIL` — an upstream message was not delivered, or its processing
    /// failed [`docs/spec/04-notifications.md` §4.2].
    MessageFailed {
        /// The sequence the message was sent in, or
        /// [`MessageSequence::Unspecified`] when the send request named none.
        sequence: MessageSequence,
        /// The progressive number the client gave the message within its
        /// sequence.
        prog: u64,
        /// Error code from the server kernel or the Metadata Adapter; codes
        /// `<= 0` are adapter-defined and application-specific
        /// [`docs/spec/05-error-codes.md` §3.8].
        code: i64,
        /// Human-readable error text; may be empty, and being the last
        /// argument may contain commas.
        message: String,
    },

    /// `CONS` — the session's bandwidth constraint
    /// [`docs/spec/04-notifications.md` §5.1].
    ///
    /// Sent at the beginning of the session and after any change to the
    /// constraint, whether client-requested or server-imposed.
    ConstraintsChanged {
        /// The maximum bandwidth now in force for the session.
        bandwidth: Bandwidth,
    },

    /// `SYNC` — how long the server thinks the session has been bound
    /// [`docs/spec/04-notifications.md` §5.2].
    ///
    /// Sent periodically on streaming connections only (never on polling
    /// ones); a client compares it against its own elapsed time to detect
    /// that it is falling behind the update flow.
    Sync {
        /// Seconds elapsed on the server since the session was bound.
        elapsed_seconds: u64,
    },

    /// `CLIENTIP` — the client's address as the server sees it
    /// [`docs/spec/04-notifications.md` §5.3].
    ///
    /// Reissued at each bind unless unchanged; a change signals that the
    /// client moved network. May be disabled by server configuration.
    ClientIp {
        /// The textual IPv4 or IPv6 address, e.g. `127.0.0.1` or `::1`.
        address: String,
    },

    /// `SERVNAME` — the configured name of the server's listening port
    /// [`docs/spec/04-notifications.md` §5.4].
    ///
    /// Reissued at each bind unless unchanged. Diagnostic only.
    ServerName {
        /// The name configured for the listening port.
        name: String,
    },

    /// `PROG` — where a recovered response flow starts
    /// [`docs/spec/04-notifications.md` §5.5].
    ///
    /// Sent only when the bind request asked for recovery via
    /// `LS_recovery_from`. The value may be **lower** than the one requested,
    /// in which case the client must skip incoming *data* notifications until
    /// the requested progressive is reached, while still obeying every
    /// notification of any other kind.
    Progressive {
        /// Count of data notifications already sent on this session — i.e.
        /// the progressive of the last one.
        progressive: u64,
    },

    /// `NOOP` — filler content with no meaning
    /// [`docs/spec/04-notifications.md` §5.6].
    ///
    /// Sent during session setup to push past the receive buffering some
    /// clients and operating systems apply to an HTTP response. The payload
    /// is to be ignored.
    NoOp {
        /// The dummy payload. Retained rather than dropped so a caller can
        /// log it, but it carries no protocol meaning.
        preamble: String,
    },

    /// `PROBE` — keep-alive [`docs/spec/04-notifications.md` §5.7].
    ///
    /// Sent when nothing else has been sent on the stream connection for the
    /// keep-alive interval reported by `CONOK`. Never sent on polling
    /// connections. Its absence past that interval means the connection is
    /// stalled. This is the only tag with no comma at all.
    Probe,

    /// `LOOP` — the session must be rebound
    /// [`docs/spec/04-notifications.md` §5.8].
    ///
    /// Sent when the polling cycle completes, when an HTTP stream connection
    /// reaches its content length, or after a client-requested rebind. Marks
    /// the end of the stream connection.
    Loop {
        /// Milliseconds the client should wait before rebinding. `0` means
        /// rebind as soon as possible; a value greater than `0` occurs only
        /// on synchronous polling sessions.
        expected_delay_millis: u64,
    },

    /// `END` — the server has closed the session
    /// [`docs/spec/04-notifications.md` §5.9].
    ///
    /// Arrives either as a notification on the stream connection or as the
    /// response to a rebind of a session that has just been forcibly closed
    /// [`docs/spec/03-requests.md` §4.2].
    End {
        /// Cause code from the server kernel; codes `<= 0` may be a custom
        /// cause supplied by an administrator's destroy command
        /// [`docs/spec/05-error-codes.md` §3.8].
        cause_code: i64,
        /// Short cause description; being the last argument it may contain
        /// commas.
        cause_message: String,
    },

    /// `CONOK` — a session was created or bound successfully
    /// [`docs/spec/03-requests.md` §4.1].
    ///
    /// The first line of a successful `create_session` or `bind_session`
    /// response. `SERVNAME`, `CLIENTIP`, `CONS` and (when recovery was asked
    /// for) `PROG` follow immediately, in any order.
    ConnectionOk {
        /// The server-internal string identifying the session.
        session_id: String,
        /// Maximum length, in bytes, the server accepts for a single client
        /// request. Relevant when batching control requests.
        request_limit_bytes: u64,
        /// Longest guaranteed inactivity, in milliseconds. On a streaming
        /// connection a `PROBE` arrives after this long without traffic; on a
        /// polling connection an empty response does. Silence for longer than
        /// this signals a problem.
        keep_alive_millis: u64,
        /// Address to which subsequent rebind and control requests must be
        /// directed when they need a new socket, for session affinity under
        /// clustering.
        ///
        /// `None` when the server sent the literal `*`, meaning no control
        /// link is configured and everything goes to the address this stream
        /// connection was opened to.
        control_link: Option<String>,
    },

    /// `CONERR` — a session could not be created or bound
    /// [`docs/spec/03-requests.md` §4.2], [`docs/spec/05-error-codes.md` §4.1].
    ///
    /// Terminates the creation or binding attempt.
    ConnectionError {
        /// Error code from the server kernel or the Metadata Adapter; codes
        /// `<= 0` are adapter-defined [`docs/spec/05-error-codes.md` §3.8].
        code: i64,
        /// Human-readable error text; may be empty when supplied by the
        /// Metadata Adapter, and may contain commas.
        message: String,
    },

    /// `REQOK` — a request succeeded [`docs/spec/03-requests.md` §13.1].
    ///
    /// Sent for every control request and for the HTTP `heartbeat`
    /// pseudo-request. Carries no outcome beyond "accepted"; the effect, if
    /// any, is reported by a separate notification on the stream connection.
    RequestOk {
        /// The request ID being answered, as the client generated it — any
        /// combination of letters and digits, so it is kept as text
        /// [`docs/spec/01-foundations.md` §5.1].
        ///
        /// `None` for the bare `REQOK` that answers an HTTP `heartbeat`,
        /// which carries no request ID because `heartbeat` takes no
        /// `LS_reqId` [`docs/spec/03-requests.md` §14.3].
        request_id: Option<String>,
    },

    /// `REQERR` — a request was understood but failed
    /// [`docs/spec/03-requests.md` §13.2], [`docs/spec/05-error-codes.md` §4.1].
    ///
    /// The only error shape that carries a request ID: it is used exactly
    /// when the server could parse one.
    RequestError {
        /// The request ID being answered, kept as text
        /// [`docs/spec/01-foundations.md` §5.1].
        request_id: String,
        /// Error code from the server kernel or the Metadata Adapter.
        code: i64,
        /// Human-readable error text; may be empty, and may contain commas.
        message: String,
    },

    /// `ERROR` — a request could not be interpreted or processed at all
    /// [`docs/spec/03-requests.md` §13.2], [`docs/spec/05-error-codes.md` §4.1].
    ///
    /// Carries no request ID, because the server could not parse one — which
    /// is precisely what distinguishes it from `REQERR`. It is also the error
    /// shape used for a malformed `heartbeat`
    /// [`docs/spec/03-requests.md` §14.3].
    Error {
        /// Error code from the server kernel.
        code: i64,
        /// Human-readable error text; may contain commas.
        message: String,
    },

    /// `WSOK` — the echo answering a `wsok` establishment check
    /// [`docs/spec/03-requests.md` §15.2].
    ///
    /// Confirms a freshly opened WebSocket carries TLCP end to end with no
    /// interference from intermediate nodes. Guaranteed to precede the
    /// responses to any later request. It has no error counterpart.
    WsOk,
}

/// The maximum update frequency reported by a `CONF`
/// [`docs/spec/04-notifications.md` §3.8].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MaxFrequency {
    /// A frequency limit expressed in updates per second.
    Limited {
        /// The limit **exactly as it arrived on the wire**.
        ///
        /// The spec calls it "a decimal number" and shows `3.0`, but never
        /// fixes its lexical form, and this crate never interprets a
        /// server-supplied number for the caller. Keeping the wire text loses
        /// nothing and invents nothing.
        updates_per_second: String,
    },
    /// The literal `unlimited`: no frequency limit is applied.
    Unlimited,
    /// Neither a decimal number nor a literal this version knows.
    ///
    /// Kept apart from [`MaxFrequency::Limited`] so that a value which is not a
    /// number cannot be handed to a caller as though it were one; the literal
    /// is preserved so a future alternative is still visible
    /// [`docs/spec/04-notifications.md` §3.8].
    Unrecognized {
        /// The argument as it arrived, after percent-decoding.
        literal: String,
    },
}

/// Whether a subscription's updates may be filtered
/// [`docs/spec/04-notifications.md` §3.8].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FilteringMode {
    /// The literal `filtered`.
    Filtered,
    /// The literal `unfiltered`.
    Unfiltered,
    /// Anything else. Preserved rather than rejected so that a future server
    /// version cannot break an older client over one argument of an otherwise
    /// well-formed line.
    Unrecognized {
        /// The literal as it arrived, after percent-decoding.
        literal: String,
    },
}

/// The session bandwidth constraint reported by a `CONS`
/// [`docs/spec/04-notifications.md` §5.1].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Bandwidth {
    /// A bandwidth limit expressed in kbps.
    Limited {
        /// The limit **exactly as it arrived on the wire**, for the same
        /// reason as [`MaxFrequency::Limited::updates_per_second`].
        kbps: String,
    },
    /// The literal `unlimited`: no bandwidth limit is applied.
    Unlimited,
    /// The literal `unmanaged`: equivalent to [`Bandwidth::Unlimited`], and
    /// additionally the client is **not allowed** to limit the bandwidth of
    /// this session.
    Unmanaged,
    /// Neither a decimal number nor a literal this version knows — see
    /// [`MaxFrequency::Unrecognized`]
    /// [`docs/spec/04-notifications.md` §5.1].
    Unrecognized {
        /// The argument as it arrived, after percent-decoding.
        literal: String,
    },
}

/// The sequence a message was sent in, as echoed by `MSGDONE` / `MSGFAIL`
/// [`docs/spec/04-notifications.md` §4.1].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MessageSequence {
    /// The send request named no sequence; the server echoes the literal `*`.
    Unspecified,
    /// The sequence identifier the client supplied.
    Named(String),
}

/// Parse one line of the TLCP response/notification syntax.
///
/// The line must already have been separated from the stream by the transport
/// layer; a single trailing `\r\n`, `\n` or `\r` is tolerated and stripped.
///
/// # Errors
///
/// - [`ProtocolError::UnknownTag`] when the tag is not one of the ones this
///   client knows — deliberately **not** fatal, so a newer server cannot break
///   an older client. MPN tags land here for v1.
/// - [`ProtocolError::ArgumentCount`] when the line carries fewer (or, for
///   zero-argument tags, more) arguments than the tag requires.
/// - [`ProtocolError::NotNumeric`] when an argument the spec types as numeric
///   is not.
/// - [`ProtocolError::Escaping`] when a percent-escape in a decoded argument
///   is malformed.
#[must_use = "a parsed line carries the server's instruction; dropping it loses it"]
pub(crate) fn parse_line(line: &str) -> Result<Notification, ProtocolError> {
    let line = strip_terminator(line);

    // §7.4 steps 2-3: the tag is the text up to the first comma, or the whole
    // line when there is none [`docs/spec/01-foundations.md` §7.4].
    let (tag, rest) = match line.split_once(',') {
        Some((tag, rest)) => (tag, Some(rest)),
        None => (line, None),
    };

    // §7.4 step 4: switch on the tag; each arm knows its own fixed argument
    // count and lets the last argument keep any further commas.
    match tag {
        // `U`, 3 arguments [`docs/spec/04-notifications.md` §2.1]. Arguments 1
        // and 2 are numeric; argument 3 is the pipe-separated value list and
        // is the one argument in the whole protocol that must NOT be
        // percent-decoded in this pass [`docs/spec/01-foundations.md` §7.4].
        "U" => {
            let [subscription_id, item_index, raw_values] = split_fixed::<3>(tag, rest)?;
            Ok(Notification::Update {
                subscription_id: number(tag, "subscription-ID", subscription_id)?,
                item_index: number(tag, "item", item_index)?,
                raw_values: raw_values.to_owned(),
            })
        }

        // `SUBOK`, 3 arguments, all numeric
        // [`docs/spec/04-notifications.md` §3.1].
        "SUBOK" => {
            let [subscription_id, item_count, field_count] = split_fixed::<3>(tag, rest)?;
            Ok(Notification::SubscriptionOk {
                subscription_id: number(tag, "subscription-ID", subscription_id)?,
                item_count: number(tag, "num-items", item_count)?,
                field_count: number(tag, "num-fields", field_count)?,
            })
        }

        // `SUBCMD`, 5 arguments, all numeric
        // [`docs/spec/04-notifications.md` §3.2].
        "SUBCMD" => {
            let [
                subscription_id,
                item_count,
                field_count,
                key_field,
                command_field,
            ] = split_fixed::<5>(tag, rest)?;
            Ok(Notification::SubscriptionCommandOk {
                subscription_id: number(tag, "subscription-ID", subscription_id)?,
                item_count: number(tag, "num-items", item_count)?,
                field_count: number(tag, "num-fields", field_count)?,
                key_field_index: number(tag, "key-field", key_field)?,
                command_field_index: number(tag, "command-field", command_field)?,
            })
        }

        // `UNSUB`, 1 numeric argument
        // [`docs/spec/04-notifications.md` §3.4].
        "UNSUB" => {
            let [subscription_id] = split_fixed::<1>(tag, rest)?;
            Ok(Notification::Unsubscribed {
                subscription_id: number(tag, "subscription-ID", subscription_id)?,
            })
        }

        // `EOS`, 2 numeric arguments
        // [`docs/spec/04-notifications.md` §3.5].
        "EOS" => {
            let [subscription_id, item_index] = split_fixed::<2>(tag, rest)?;
            Ok(Notification::EndOfSnapshot {
                subscription_id: number(tag, "subscription-ID", subscription_id)?,
                item_index: number(tag, "item", item_index)?,
            })
        }

        // `CS`, 2 numeric arguments [`docs/spec/04-notifications.md` §3.6].
        "CS" => {
            let [subscription_id, item_index] = split_fixed::<2>(tag, rest)?;
            Ok(Notification::ClearSnapshot {
                subscription_id: number(tag, "subscription-ID", subscription_id)?,
                item_index: number(tag, "item", item_index)?,
            })
        }

        // `OV`, 3 numeric arguments [`docs/spec/04-notifications.md` §3.7].
        "OV" => {
            let [subscription_id, item_index, overflow_size] = split_fixed::<3>(tag, rest)?;
            Ok(Notification::Overflow {
                subscription_id: number(tag, "subscription-ID", subscription_id)?,
                item_index: number(tag, "item", item_index)?,
                dropped_count: number(tag, "overflow-size", overflow_size)?,
            })
        }

        // `CONF`, 3 arguments [`docs/spec/04-notifications.md` §3.8]:
        // numeric ID, then a decimal-or-`unlimited` frequency, then the
        // `filtered`/`unfiltered` literal. The last two are non-numeric and
        // are percent-decoded [`docs/spec/01-foundations.md` §7.4].
        "CONF" => {
            let [subscription_id, max_frequency, filtering] = split_fixed::<3>(tag, rest)?;
            Ok(Notification::SubscriptionReconfigured {
                subscription_id: number(tag, "subscription-ID", subscription_id)?,
                max_frequency: parse_max_frequency(&decode(max_frequency)?),
                filtering: parse_filtering(decode(filtering)?),
            })
        }

        // `MSGDONE`, 3 arguments [`docs/spec/04-notifications.md` §4.1]. The
        // response is the last argument and keeps its commas — the example on
        // p.60 shows free-form adapter text there.
        "MSGDONE" => {
            let [sequence, prog, response] = split_fixed::<3>(tag, rest)?;
            Ok(Notification::MessageDone {
                sequence: parse_sequence(decode(sequence)?),
                prog: number(tag, "prog", prog)?,
                response: decode(response)?,
            })
        }

        // `MSGFAIL`, 4 arguments [`docs/spec/04-notifications.md` §4.2],
        // [`docs/spec/05-error-codes.md` §4.1 shape 5]: the code sits at
        // position 3 and the message at position 4, which is last and keeps
        // its commas.
        "MSGFAIL" => {
            let [sequence, prog, code, message] = split_fixed::<4>(tag, rest)?;
            Ok(Notification::MessageFailed {
                sequence: parse_sequence(decode(sequence)?),
                prog: number(tag, "prog", prog)?,
                code: number(tag, "error-code", code)?,
                message: decode(message)?,
            })
        }

        // `CONS`, 1 argument: a decimal, or `unlimited`, or `unmanaged`
        // [`docs/spec/04-notifications.md` §5.1].
        "CONS" => {
            let [bandwidth] = split_fixed::<1>(tag, rest)?;
            Ok(Notification::ConstraintsChanged {
                bandwidth: parse_bandwidth(&decode(bandwidth)?),
            })
        }

        // `SYNC`, 1 numeric argument in seconds
        // [`docs/spec/04-notifications.md` §5.2].
        "SYNC" => {
            let [seconds] = split_fixed::<1>(tag, rest)?;
            Ok(Notification::Sync {
                elapsed_seconds: number(tag, "seconds-since-initial-header", seconds)?,
            })
        }

        // `CLIENTIP`, 1 string argument
        // [`docs/spec/04-notifications.md` §5.3]. An IPv6 address contains
        // colons but no commas, so it survives the comma split intact.
        "CLIENTIP" => {
            let [address] = split_fixed::<1>(tag, rest)?;
            Ok(Notification::ClientIp {
                address: decode(address)?,
            })
        }

        // `SERVNAME`, 1 string argument
        // [`docs/spec/04-notifications.md` §5.4].
        "SERVNAME" => {
            let [name] = split_fixed::<1>(tag, rest)?;
            Ok(Notification::ServerName {
                name: decode(name)?,
            })
        }

        // `PROG`, 1 numeric argument
        // [`docs/spec/04-notifications.md` §5.5].
        "PROG" => {
            let [progressive] = split_fixed::<1>(tag, rest)?;
            Ok(Notification::Progressive {
                progressive: number(tag, "progressive", progressive)?,
            })
        }

        // `NOOP`, 1 string argument to be ignored
        // [`docs/spec/04-notifications.md` §5.6].
        "NOOP" => {
            let [preamble] = split_fixed::<1>(tag, rest)?;
            Ok(Notification::NoOp {
                preamble: decode(preamble)?,
            })
        }

        // `PROBE`, 0 arguments — a bare tag with no comma at all
        // [`docs/spec/04-notifications.md` §5.7].
        "PROBE" => {
            let [] = split_fixed::<0>(tag, rest)?;
            Ok(Notification::Probe)
        }

        // `LOOP`, 1 numeric argument in milliseconds
        // [`docs/spec/04-notifications.md` §5.8].
        "LOOP" => {
            let [delay] = split_fixed::<1>(tag, rest)?;
            Ok(Notification::Loop {
                expected_delay_millis: number(tag, "expected-delay", delay)?,
            })
        }

        // `END`, 2 arguments [`docs/spec/04-notifications.md` §5.9],
        // [`docs/spec/05-error-codes.md` §4.1 shape 2].
        //
        // SPEC-AMBIGUITY: the spec prints "Arguments: 1" for `END` in both
        // places it documents the tag, then documents two arguments, and
        // every format line and worked example carries two — flagged as
        // ambiguity 19 in [`docs/spec/04-notifications.md` §7] and in the
        // `END` note of [`docs/spec/05-error-codes.md` §4.1]. Two is taken as
        // correct, which is also the tolerant reading: with a count of 2 a
        // one-argument `END` is rejected, whereas a count of 1 would silently
        // glue the code and the message together into one field.
        "END" => {
            let [cause_code, cause_message] = split_fixed::<2>(tag, rest)?;
            Ok(Notification::End {
                cause_code: number(tag, "cause-code", cause_code)?,
                cause_message: decode(cause_message)?,
            })
        }

        // `CONOK`, 4 arguments [`docs/spec/03-requests.md` §4.1]: string
        // session ID, two numerics, then the control link, where the literal
        // `*` (UTF-8 `0x2a`) means "no control link configured".
        "CONOK" => {
            let [session_id, request_limit, keep_alive, control_link] =
                split_fixed::<4>(tag, rest)?;
            let control_link = decode(control_link)?;
            Ok(Notification::ConnectionOk {
                session_id: decode(session_id)?,
                request_limit_bytes: number(tag, "request-limit", request_limit)?,
                keep_alive_millis: number(tag, "keep-alive", keep_alive)?,
                control_link: if control_link == "*" {
                    None
                } else {
                    Some(control_link)
                },
            })
        }

        // `CONERR`, 2 arguments [`docs/spec/03-requests.md` §4.2],
        // [`docs/spec/05-error-codes.md` §4.1 shape 1].
        "CONERR" => {
            let [code, message] = split_fixed::<2>(tag, rest)?;
            Ok(Notification::ConnectionError {
                code: number(tag, "error-code", code)?,
                message: decode(message)?,
            })
        }

        // `REQOK`, 1 argument as a control response
        // [`docs/spec/03-requests.md` §13.1] — but **0 arguments** as the
        // HTTP `heartbeat` response [`docs/spec/03-requests.md` §14.3].
        //
        // SPEC-AMBIGUITY: this is the one tag that contradicts the "each
        // different tag has a fixed number of arguments" premise of the
        // parsing algorithm [`docs/spec/01-foundations.md` §7.1], and neither
        // section acknowledges the other. Both forms are accepted and the
        // difference is preserved in `request_id`, which is the only choice
        // that never discards information: rejecting the bare form would
        // break `heartbeat` over HTTP, and rejecting the one-argument form
        // would break every control response.
        //
        // The request ID is kept as text because the spec allows "any
        // combination of letters and numbers"
        // [`docs/spec/01-foundations.md` §5.1]; parsing it as an integer
        // would reject legal IDs.
        "REQOK" => match rest {
            None => Ok(Notification::RequestOk { request_id: None }),
            Some(_) => {
                let [request_id] = split_fixed::<1>(tag, rest)?;
                Ok(Notification::RequestOk {
                    request_id: Some(decode(request_id)?),
                })
            }
        },

        // `REQERR`, 3 arguments [`docs/spec/03-requests.md` §13.2],
        // [`docs/spec/05-error-codes.md` §4.1 shape 3]: the code sits at
        // position 2, not 1, and the message is last.
        //
        // SPEC-AMBIGUITY: the spec's own worked example,
        // `REQERR,19,Specified subscription not found`
        // [`docs/spec/03-requests.md` §13.2, p.45], carries only **two**
        // arguments, and `19` is the Appendix B code for exactly that message
        // [`docs/spec/05-error-codes.md` §2, p.91] — so the example omits the
        // request ID that its own format line declares. Neither document
        // flags this. The declared 3-argument format wins here: a
        // two-argument `REQERR` is rejected rather than parsed by guessing
        // which argument is missing, because guessing would report a request
        // ID of `19` for an unrelated request, and a caller acting on that is
        // worse off than a caller told the line was malformed.
        "REQERR" => {
            let [request_id, code, message] = split_fixed::<3>(tag, rest)?;
            Ok(Notification::RequestError {
                request_id: decode(request_id)?,
                code: number(tag, "error-code", code)?,
                message: decode(message)?,
            })
        }

        // `ERROR`, 2 arguments [`docs/spec/03-requests.md` §13.2, §14.3],
        // [`docs/spec/05-error-codes.md` §4.1 shape 4]. Unlike `REQERR` it
        // carries no request ID, because the server could not parse one.
        "ERROR" => {
            let [code, message] = split_fixed::<2>(tag, rest)?;
            Ok(Notification::Error {
                code: number(tag, "error-code", code)?,
                message: decode(message)?,
            })
        }

        // `WSOK`, 0 arguments [`docs/spec/03-requests.md` §15.2].
        "WSOK" => {
            let [] = split_fixed::<0>(tag, rest)?;
            Ok(Notification::WsOk)
        }

        // Anything else, including the MPN tags deferred out of v1
        // (`MPNREG`, `MPNZERO`, `MPNOK`, `MPNCONF`, `MPNDEL`
        // [`docs/spec/06-mpn.md`]).
        //
        // SPEC-AMBIGUITY: [`docs/spec/01-foundations.md` §7.4] flags that the
        // spec never says whether an unrecognised tag must be ignored, must
        // abort parsing, or must drop the connection. The defensive choice is
        // to report it with the whole line preserved and let the caller
        // decide; nothing is dropped and nothing is fatal here.
        _ => Err(ProtocolError::UnknownTag {
            tag: tag.to_owned(),
            line: line.to_owned(),
        }),
    }
}

/// Remove at most one trailing line terminator.
///
/// The transport layer owns framing, so a well-behaved caller passes a line
/// that has none. Tolerating one is cheap insurance against a caller that
/// split on `\n` and left the `\r` attached. Only **one** is removed, so that
/// the raw value list of a `U` keeps every byte the server actually sent.
#[inline]
fn strip_terminator(line: &str) -> &str {
    match line.strip_suffix("\r\n") {
        Some(trimmed) => trimmed,
        None => match line.strip_suffix('\n') {
            Some(trimmed) => trimmed,
            None => line.strip_suffix('\r').unwrap_or(line),
        },
    }
}

/// Split the part of the line after the tag into exactly `N` arguments.
///
/// This is the heart of [`docs/spec/01-foundations.md` §7.4]: the split stops
/// after `N` pieces, so the **last** argument absorbs every remaining comma.
/// A message such as `Session count limit reached, try later` therefore stays
/// intact instead of being torn into two arguments.
///
/// `rest` is `None` when the line had no comma at all, which is how the
/// zero-argument tags (`PROBE`, `WSOK`, and the bare `heartbeat` `REQOK`)
/// arrive.
///
/// # Errors
///
/// [`ProtocolError::ArgumentCount`] when the line supplies a different number
/// of arguments than the tag's fixed count.
fn split_fixed<'a, const N: usize>(
    tag: &str,
    rest: Option<&'a str>,
) -> Result<[&'a str; N], ProtocolError> {
    let Some(rest) = rest else {
        // No comma at all: the line supplied zero arguments.
        if N == 0 {
            return Ok([""; N]);
        }
        return Err(argument_count(tag, N, 0));
    };

    if N == 0 {
        // A comma is present where the tag admits no arguments.
        return Err(argument_count(tag, 0, rest.split(',').count()));
    }

    let mut arguments = [""; N];
    let mut parts = rest.splitn(N, ',');
    // `index` is how many arguments were filled before this one, which is
    // exactly what the error has to report — counted by the iterator rather
    // than by an addition of our own, so there is no arithmetic to overflow.
    for (index, slot) in arguments.iter_mut().enumerate() {
        match parts.next() {
            Some(part) => *slot = part,
            None => return Err(argument_count(tag, N, index)),
        }
    }
    Ok(arguments)
}

/// Percent-decode one argument [`docs/spec/01-foundations.md` §7.2].
///
/// # Errors
///
/// [`ProtocolError::Escaping`] when the escape sequence is malformed.
#[inline]
fn decode(argument: &str) -> Result<String, ProtocolError> {
    Ok(percent_decode(argument)?.into_owned())
}

/// Parse an argument the spec types as numeric.
///
/// Numeric arguments are read from the raw text, without percent-decoding:
/// §7.4 says to decode "non-numeric arguments", and a decimal integer has
/// nothing a server would encode. (The spec itself flags that it never says
/// whether decoding a numeric argument would be harmful or merely
/// unnecessary — [`docs/spec/01-foundations.md` §7.4].)
///
/// # Errors
///
/// [`ProtocolError::NotNumeric`] when the text is not a valid `T`.
#[inline]
fn number<T: FromStr>(tag: &str, argument: &'static str, value: &str) -> Result<T, ProtocolError> {
    value
        .parse::<T>()
        .map_err(|_| not_numeric(tag, argument, value))
}

/// Classify a `CONF` `<max-frequency>` argument
/// [`docs/spec/04-notifications.md` §3.8].
///
/// Comparison against `unlimited` is exact and case-sensitive: the spec gives
/// the token in lowercase and says nothing about other casings, and accepting
/// `UNLIMITED` would silently reinterpret a value the server did not send.
#[must_use]
fn parse_max_frequency(argument: &str) -> MaxFrequency {
    if argument == "unlimited" {
        MaxFrequency::Unlimited
    } else if is_decimal_number(argument) {
        // The value is kept verbatim: this crate does not convert
        // server-supplied numbers on the caller's behalf.
        MaxFrequency::Limited {
            updates_per_second: argument.to_owned(),
        }
    } else {
        MaxFrequency::Unrecognized {
            literal: argument.to_owned(),
        }
    }
}

/// Classify a `CONS` `<bandwidth>` argument
/// [`docs/spec/04-notifications.md` §5.1].
///
/// The two literals are matched exactly, as in [`parse_max_frequency`].
#[must_use]
fn parse_bandwidth(argument: &str) -> Bandwidth {
    match argument {
        "unlimited" => Bandwidth::Unlimited,
        "unmanaged" => Bandwidth::Unmanaged,
        _ if is_decimal_number(argument) => Bandwidth::Limited {
            kbps: argument.to_owned(),
        },
        _ => Bandwidth::Unrecognized {
            literal: argument.to_owned(),
        },
    }
}

/// Whether a `CONF` or `CONS` argument is the decimal alternative.
///
/// SPEC-AMBIGUITY: the two notification sections type these arguments as "a
/// decimal number" and give no grammar — flagged as ambiguity 16 in
/// [`docs/spec/04-notifications.md` §7] for `CONF`, and §5.1 says no more for
/// `CONS`. The grammar checked here is the one the specification *does* fix,
/// for the request parameters that carry the same two quantities in the other
/// direction: "a decimal number, with a **dot** as decimal separator"
/// [`docs/spec/03-requests.md` §8.1, §9.1]. It is deliberately the very same
/// predicate the encoder enforces on what this client sends, so the client
/// cannot demand of the server a spelling it would not produce itself.
///
/// The strictness is therefore a **choice**, not a quotation: a server that
/// wrote `+3`, `3.`, `3e2` or `3,0` would be reported as
/// [`MaxFrequency::Unrecognized`] rather than as a limit. That is the point —
/// a caller told "this is 3 updates/sec" when the server said something else
/// acts on a number nobody sent.
#[must_use]
fn is_decimal_number(argument: &str) -> bool {
    crate::protocol::request::DecimalNumber::try_new(argument).is_ok()
}

/// Classify a `CONF` `<filtered|unfiltered>` argument
/// [`docs/spec/04-notifications.md` §3.8].
#[must_use]
fn parse_filtering(argument: String) -> FilteringMode {
    match argument.as_str() {
        "filtered" => FilteringMode::Filtered,
        "unfiltered" => FilteringMode::Unfiltered,
        // SPEC-AMBIGUITY: the spec states the argument is one of exactly two
        // literals and does not say what a client should do with a third —
        // [`docs/spec/04-notifications.md` §3.8] carries no flag for this
        // case. Preserving it beats rejecting the whole line: the rest of the
        // notification is well-formed and useful, and "unknown is not fatal"
        // is this crate's standing rule for forward compatibility.
        _ => FilteringMode::Unrecognized { literal: argument },
    }
}

/// Classify a `MSGDONE` / `MSGFAIL` `<sequence>` argument
/// [`docs/spec/04-notifications.md` §4.1].
///
/// SPEC-AMBIGUITY: the server sends the literal `*` when the send request
/// named no sequence, and the spec neither forbids a client from naming a
/// sequence `*` nor says how the two would be told apart — no flag covers
/// this in [`docs/spec/04-notifications.md` §4]. A received `*` is therefore
/// read as "no sequence", matching the documented server behaviour; a client
/// that literally names a sequence `*` is choosing the collision.
#[must_use]
fn parse_sequence(argument: String) -> MessageSequence {
    if argument == "*" {
        MessageSequence::Unspecified
    } else {
        MessageSequence::Named(argument)
    }
}

/// Build an [`ProtocolError::ArgumentCount`].
#[cold]
#[inline(never)]
fn argument_count(tag: &str, expected: usize, found: usize) -> ProtocolError {
    ProtocolError::ArgumentCount {
        tag: tag.to_owned(),
        expected,
        found,
    }
}

/// Build a [`ProtocolError::NotNumeric`].
#[cold]
#[inline(never)]
fn not_numeric(tag: &str, argument: &'static str, value: &str) -> ProtocolError {
    ProtocolError::NotNumeric {
        tag: tag.to_owned(),
        argument,
        value: value.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------
    // Real-time update — `U`
    // ---------------------------------------------------------------------

    #[test]
    fn test_parse_line_update_spec_example_ok() {
        // Verbatim from `docs/spec/04-notifications.md` §2.5 Example 1 [p.51].
        let line = "U,3,1,20:00:33|3.04|0.0|2.41|3.67|3.03|3.04|#|#|$";
        assert_eq!(
            parse_line(line),
            Ok(Notification::Update {
                subscription_id: 3,
                item_index: 1,
                raw_values: "20:00:33|3.04|0.0|2.41|3.67|3.03|3.04|#|#|$".to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_line_update_unchanged_run_ok() {
        // `docs/spec/04-notifications.md` §2.5 Example 4 [p.52].
        assert_eq!(
            parse_line("U,3,1,20:04:40|^4|3.02|3.03|||"),
            Ok(Notification::Update {
                subscription_id: 3,
                item_index: 1,
                raw_values: "20:04:40|^4|3.02|3.03|||".to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_line_update_raw_values_preserved_byte_for_byte() {
        // `docs/spec/04-notifications.md` §2.6 diff example 2 [p.53]: the
        // value list carries commas (inside JSON) and a `%7C` escape. Both
        // must survive untouched — the commas because argument 3 is last, the
        // escape because argument 3 is exempt from first-pass decoding
        // [`docs/spec/01-foundations.md` §7.4].
        let values = "20:00:54|^P[{\"op\":\"replace\",\"path\":\"/text\",\"value\":\"aa%7Cbb%7Ccc\"},{\"op\":\"add\",\"path\":\"/attributes/1\",\"value\":\"bold\"}]";
        let line = format!("U,5,1,{values}");
        assert_eq!(
            parse_line(&line),
            Ok(Notification::Update {
                subscription_id: 5,
                item_index: 1,
                raw_values: values.to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_line_update_empty_value_list_ok() {
        // `docs/spec/04-notifications.md` §2.6 diff example 3 [p.53]: a
        // single empty token, meaning "unchanged".
        assert_eq!(
            parse_line("U,5,1,20:04:16|"),
            Ok(Notification::Update {
                subscription_id: 5,
                item_index: 1,
                raw_values: "20:04:16|".to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_line_update_missing_values_argument_is_error() {
        assert_eq!(
            parse_line("U,3,1"),
            Err(ProtocolError::ArgumentCount {
                tag: "U".to_owned(),
                expected: 3,
                found: 2,
            })
        );
    }

    #[test]
    fn test_parse_line_update_non_numeric_subscription_id_is_error() {
        assert_eq!(
            parse_line("U,x,1,a|b"),
            Err(ProtocolError::NotNumeric {
                tag: "U".to_owned(),
                argument: "subscription-ID",
                value: "x".to_owned(),
            })
        );
    }

    // ---------------------------------------------------------------------
    // Subscription notifications
    // ---------------------------------------------------------------------

    #[test]
    fn test_parse_line_subok_spec_example_ok() {
        // `docs/spec/04-notifications.md` §3.1 [p.54].
        assert_eq!(
            parse_line("SUBOK,3,1,10"),
            Ok(Notification::SubscriptionOk {
                subscription_id: 3,
                item_count: 1,
                field_count: 10,
            })
        );
    }

    #[test]
    fn test_parse_line_subcmd_spec_example_ok() {
        // `docs/spec/04-notifications.md` §3.2 [p.54].
        assert_eq!(
            parse_line("SUBCMD,3,1,10,1,2"),
            Ok(Notification::SubscriptionCommandOk {
                subscription_id: 3,
                item_count: 1,
                field_count: 10,
                key_field_index: 1,
                command_field_index: 2,
            })
        );
    }

    #[test]
    fn test_parse_line_subcmd_too_few_arguments_is_error() {
        assert_eq!(
            parse_line("SUBCMD,3,1,10"),
            Err(ProtocolError::ArgumentCount {
                tag: "SUBCMD".to_owned(),
                expected: 5,
                found: 3,
            })
        );
    }

    #[test]
    fn test_parse_line_unsub_spec_example_ok() {
        // `docs/spec/04-notifications.md` §3.4 [p.55].
        assert_eq!(
            parse_line("UNSUB,3"),
            Ok(Notification::Unsubscribed { subscription_id: 3 })
        );
    }

    #[test]
    fn test_parse_line_eos_spec_example_ok() {
        // `docs/spec/04-notifications.md` §3.5 [p.55].
        assert_eq!(
            parse_line("EOS,3,1"),
            Ok(Notification::EndOfSnapshot {
                subscription_id: 3,
                item_index: 1,
            })
        );
    }

    #[test]
    fn test_parse_line_cs_spec_example_ok() {
        // `docs/spec/04-notifications.md` §3.6 [p.56].
        assert_eq!(
            parse_line("CS,3,1"),
            Ok(Notification::ClearSnapshot {
                subscription_id: 3,
                item_index: 1,
            })
        );
    }

    #[test]
    fn test_parse_line_ov_spec_example_ok() {
        // `docs/spec/04-notifications.md` §3.7 [p.56].
        assert_eq!(
            parse_line("OV,3,1,5"),
            Ok(Notification::Overflow {
                subscription_id: 3,
                item_index: 1,
                dropped_count: 5,
            })
        );
    }

    #[test]
    fn test_parse_line_conf_spec_example_ok() {
        // `docs/spec/04-notifications.md` §3.8 [p.57].
        assert_eq!(
            parse_line("CONF,3,3.0,filtered"),
            Ok(Notification::SubscriptionReconfigured {
                subscription_id: 3,
                max_frequency: MaxFrequency::Limited {
                    updates_per_second: "3.0".to_owned(),
                },
                filtering: FilteringMode::Filtered,
            })
        );
    }

    #[test]
    fn test_parse_line_conf_unlimited_unfiltered_ok() {
        // Both literals of `docs/spec/04-notifications.md` §3.8 [p.57].
        assert_eq!(
            parse_line("CONF,7,unlimited,unfiltered"),
            Ok(Notification::SubscriptionReconfigured {
                subscription_id: 7,
                max_frequency: MaxFrequency::Unlimited,
                filtering: FilteringMode::Unfiltered,
            })
        );
    }

    #[test]
    fn test_parse_line_conf_unrecognized_filtering_is_preserved() {
        // SPEC-AMBIGUITY in `parse_filtering`: a third literal is kept, not
        // rejected.
        assert_eq!(
            parse_line("CONF,7,1.5,whatever"),
            Ok(Notification::SubscriptionReconfigured {
                subscription_id: 7,
                max_frequency: MaxFrequency::Limited {
                    updates_per_second: "1.5".to_owned(),
                },
                filtering: FilteringMode::Unrecognized {
                    literal: "whatever".to_owned(),
                },
            })
        );
    }

    #[test]
    fn test_parse_max_frequency_accepts_only_the_two_specified_alternatives() {
        // The decimal alternative, in the shapes the grammar allows
        // [`docs/spec/03-requests.md` §8.1].
        for text in ["3.0", "3", "0", "0.5", "12345.678", "007"] {
            assert_eq!(
                parse_max_frequency(text),
                MaxFrequency::Limited {
                    updates_per_second: text.to_owned(),
                },
                "{text:?} is a decimal number"
            );
        }
        // The literal alternative, matched exactly [§3.8].
        assert_eq!(parse_max_frequency("unlimited"), MaxFrequency::Unlimited);
    }

    #[test]
    fn test_parse_max_frequency_does_not_pass_off_garbage_as_a_limit() {
        // Empty, sign-only, whitespace, an exponent, a comma separator, a
        // dangling dot, trailing garbage, a casing the spec does not give, and
        // a hypothetical future literal. None of these is a number, and the
        // caller must be able to tell that from the value alone.
        for text in [
            "",
            "+",
            "-3",
            "+3.0",
            " 3.0",
            "3.0 ",
            "3e2",
            "3,0",
            "3.",
            ".5",
            "3.0x",
            "UNLIMITED",
            "unfiltered",
            "adaptive",
        ] {
            assert_eq!(
                parse_max_frequency(text),
                MaxFrequency::Unrecognized {
                    literal: text.to_owned(),
                },
                "{text:?} must not be reported as a frequency limit"
            );
        }
    }

    #[test]
    fn test_parse_max_frequency_keeps_an_overlong_number_verbatim() {
        // A number far past any integer type is still a decimal number, and
        // this crate never converts one: it is reported as the server wrote it
        // rather than saturated or rejected.
        let long = "1".repeat(200);
        assert_eq!(
            parse_max_frequency(&long),
            MaxFrequency::Limited {
                updates_per_second: long.clone(),
            }
        );
    }

    #[test]
    fn test_parse_line_conf_with_an_unreadable_frequency() {
        assert_eq!(
            parse_line("CONF,7,banana,filtered"),
            Ok(Notification::SubscriptionReconfigured {
                subscription_id: 7,
                max_frequency: MaxFrequency::Unrecognized {
                    literal: "banana".to_owned(),
                },
                // The rest of the line is well formed and still useful.
                filtering: FilteringMode::Filtered,
            })
        );
    }

    // ---------------------------------------------------------------------
    // Message notifications
    // ---------------------------------------------------------------------

    #[test]
    fn test_parse_line_msgdone_spec_example_ok() {
        // `docs/spec/04-notifications.md` §4.1 [p.60].
        assert_eq!(
            parse_line("MSGDONE,Orders_Sequence,3,Processed with ID 32652506"),
            Ok(Notification::MessageDone {
                sequence: MessageSequence::Named("Orders_Sequence".to_owned()),
                prog: 3,
                response: "Processed with ID 32652506".to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_line_msgdone_unspecified_sequence_ok() {
        // `*` means the send request named no sequence
        // [`docs/spec/04-notifications.md` §4.1, p.60].
        assert_eq!(
            parse_line("MSGDONE,*,1,"),
            Ok(Notification::MessageDone {
                sequence: MessageSequence::Unspecified,
                prog: 1,
                response: String::new(),
            })
        );
    }

    #[test]
    fn test_parse_line_msgfail_spec_example_ok() {
        // `docs/spec/04-notifications.md` §4.2 [p.61].
        assert_eq!(
            parse_line(
                "MSGFAIL,Orders_Sequence,4,38,The specified progressive number has been skipped by timeout"
            ),
            Ok(Notification::MessageFailed {
                sequence: MessageSequence::Named("Orders_Sequence".to_owned()),
                prog: 4,
                code: 38,
                message: "The specified progressive number has been skipped by timeout".to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_line_msgfail_message_with_commas_is_not_split() {
        // The fixed-argument-count rule: `MSGFAIL` has 4 arguments, so every
        // comma past the fourth belongs to the message
        // [`docs/spec/01-foundations.md` §7.4].
        assert_eq!(
            parse_line("MSGFAIL,Seq,4,38,rejected, retry later, or give up"),
            Ok(Notification::MessageFailed {
                sequence: MessageSequence::Named("Seq".to_owned()),
                prog: 4,
                code: 38,
                message: "rejected, retry later, or give up".to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_line_msgfail_adapter_supplied_negative_code_ok() {
        // Codes `<= 0` come from the Metadata Adapter and have no stated
        // lower bound [`docs/spec/05-error-codes.md` §3.8].
        assert_eq!(
            parse_line("MSGFAIL,*,1,-7,application specific"),
            Ok(Notification::MessageFailed {
                sequence: MessageSequence::Unspecified,
                prog: 1,
                code: -7,
                message: "application specific".to_owned(),
            })
        );
    }

    // ---------------------------------------------------------------------
    // Session notifications
    // ---------------------------------------------------------------------

    #[test]
    fn test_parse_line_cons_spec_example_ok() {
        // `docs/spec/04-notifications.md` §5.1 [p.61].
        assert_eq!(
            parse_line("CONS,50"),
            Ok(Notification::ConstraintsChanged {
                bandwidth: Bandwidth::Limited {
                    kbps: "50".to_owned()
                },
            })
        );
    }

    #[test]
    fn test_parse_line_cons_unlimited_and_unmanaged_ok() {
        // Both literals of `docs/spec/04-notifications.md` §5.1 [p.61].
        assert_eq!(
            parse_line("CONS,unlimited"),
            Ok(Notification::ConstraintsChanged {
                bandwidth: Bandwidth::Unlimited
            })
        );
        assert_eq!(
            parse_line("CONS,unmanaged"),
            Ok(Notification::ConstraintsChanged {
                bandwidth: Bandwidth::Unmanaged
            })
        );
    }

    #[test]
    fn test_parse_line_cons_with_an_unreadable_bandwidth() {
        // An empty argument is the sharp case: `CONS,` used to become a
        // bandwidth limit of `""`, which a caller cannot tell from a real
        // constraint [`docs/spec/04-notifications.md` §5.1].
        assert_eq!(
            parse_line("CONS,"),
            Ok(Notification::ConstraintsChanged {
                bandwidth: Bandwidth::Unrecognized {
                    literal: String::new(),
                },
            })
        );
        for text in ["-50", "50kbps", "5e3", "50,0", "UNMANAGED", "throttled"] {
            assert_eq!(
                parse_line(&format!("CONS,{text}")),
                Ok(Notification::ConstraintsChanged {
                    bandwidth: Bandwidth::Unrecognized {
                        literal: text.to_owned(),
                    },
                }),
                "{text:?} must not be reported as a bandwidth limit"
            );
        }
        // The decimal alternative still parses, in both spellings the spec
        // shows for these quantities [§5.1, p.61; §3.8, p.57].
        assert_eq!(
            parse_line("CONS,50.5"),
            Ok(Notification::ConstraintsChanged {
                bandwidth: Bandwidth::Limited {
                    kbps: "50.5".to_owned(),
                },
            })
        );
    }

    #[test]
    fn test_parse_line_sync_spec_example_ok() {
        // `docs/spec/04-notifications.md` §5.2 [p.62].
        assert_eq!(
            parse_line("SYNC,120"),
            Ok(Notification::Sync {
                elapsed_seconds: 120
            })
        );
    }

    #[test]
    fn test_parse_line_clientip_ipv4_spec_example_ok() {
        // `docs/spec/04-notifications.md` §5.3 [p.62].
        assert_eq!(
            parse_line("CLIENTIP,127.0.0.1"),
            Ok(Notification::ClientIp {
                address: "127.0.0.1".to_owned()
            })
        );
    }

    #[test]
    fn test_parse_line_clientip_ipv6_spec_example_ok() {
        // `docs/spec/04-notifications.md` §5.3 [p.62]: colons, no commas.
        assert_eq!(
            parse_line("CLIENTIP,::1"),
            Ok(Notification::ClientIp {
                address: "::1".to_owned()
            })
        );
    }

    #[test]
    fn test_parse_line_servname_spec_example_ok() {
        // `docs/spec/04-notifications.md` §5.4 [p.62].
        assert_eq!(
            parse_line("SERVNAME,Lightstreamer HTTP Server"),
            Ok(Notification::ServerName {
                name: "Lightstreamer HTTP Server".to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_line_servname_percent_encoded_argument_is_decoded() {
        // A comma inside an argument arrives percent-encoded
        // [`docs/spec/01-foundations.md` §7.2]; `%2C` is a comma and `%20` a
        // space, so the decoded name contains a comma that never acted as a
        // separator.
        assert_eq!(
            parse_line("SERVNAME,Acme%2C%20Inc.%20Server"),
            Ok(Notification::ServerName {
                name: "Acme, Inc. Server".to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_line_prog_spec_example_ok() {
        // `docs/spec/04-notifications.md` §5.5 [p.63].
        assert_eq!(
            parse_line("PROG,24576"),
            Ok(Notification::Progressive { progressive: 24576 })
        );
    }

    #[test]
    fn test_parse_line_noop_spec_example_ok() {
        // `docs/spec/04-notifications.md` §5.6 [p.63].
        assert_eq!(
            parse_line("NOOP,sending placeholder data"),
            Ok(Notification::NoOp {
                preamble: "sending placeholder data".to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_line_probe_spec_example_ok() {
        // `docs/spec/04-notifications.md` §5.7 [p.64]: a bare tag, no comma.
        assert_eq!(parse_line("PROBE"), Ok(Notification::Probe));
    }

    #[test]
    fn test_parse_line_probe_with_argument_is_error() {
        // `PROBE` takes zero arguments, so a comma is malformed.
        assert_eq!(
            parse_line("PROBE,1"),
            Err(ProtocolError::ArgumentCount {
                tag: "PROBE".to_owned(),
                expected: 0,
                found: 1,
            })
        );
    }

    #[test]
    fn test_parse_line_loop_spec_example_ok() {
        // `docs/spec/04-notifications.md` §5.8 [p.64].
        assert_eq!(
            parse_line("LOOP,5000"),
            Ok(Notification::Loop {
                expected_delay_millis: 5000
            })
        );
    }

    #[test]
    fn test_parse_line_end_spec_example_ok() {
        // `docs/spec/04-notifications.md` §5.9 [p.65].
        assert_eq!(
            parse_line("END,8,Session count limit reached"),
            Ok(Notification::End {
                cause_code: 8,
                cause_message: "Session count limit reached".to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_line_end_cause_message_with_commas_is_not_split() {
        // `END` has 2 arguments, so the cause description keeps its commas.
        assert_eq!(
            parse_line("END,31,Destroy invoked by client, no retry"),
            Ok(Notification::End {
                cause_code: 31,
                cause_message: "Destroy invoked by client, no retry".to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_line_end_single_argument_is_error() {
        // The consequence of reading the documented two arguments rather than
        // the printed count of one — see the SPEC-AMBIGUITY on the `END` arm.
        assert_eq!(
            parse_line("END,8"),
            Err(ProtocolError::ArgumentCount {
                tag: "END".to_owned(),
                expected: 2,
                found: 1,
            })
        );
    }

    // ---------------------------------------------------------------------
    // Request responses
    // ---------------------------------------------------------------------

    #[test]
    fn test_parse_line_conok_spec_example_ok() {
        // `docs/spec/03-requests.md` §4.1 [p.27]; `*` means no control link.
        assert_eq!(
            parse_line("CONOK,S73d162c183916f0dT2729905,50000,5000,*"),
            Ok(Notification::ConnectionOk {
                session_id: "S73d162c183916f0dT2729905".to_owned(),
                request_limit_bytes: 50000,
                keep_alive_millis: 5000,
                control_link: None,
            })
        );
    }

    #[test]
    fn test_parse_line_conok_with_control_link_ok() {
        // A configured control link is an address, not `*`
        // [`docs/spec/03-requests.md` §4.1, p.27].
        assert_eq!(
            parse_line("CONOK,S1,50000,5000,push2.example.com:443"),
            Ok(Notification::ConnectionOk {
                session_id: "S1".to_owned(),
                request_limit_bytes: 50000,
                keep_alive_millis: 5000,
                control_link: Some("push2.example.com:443".to_owned()),
            })
        );
    }

    #[test]
    fn test_parse_line_conerr_spec_example_ok() {
        // `docs/spec/03-requests.md` §4.2 [p.28].
        assert_eq!(
            parse_line("CONERR,2,Requested Adapter Set not available"),
            Ok(Notification::ConnectionError {
                code: 2,
                message: "Requested Adapter Set not available".to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_line_conerr_empty_message_ok() {
        // "A message sent by the Metadata Adapter may be empty"
        // [`docs/spec/05-error-codes.md` §4.2].
        assert_eq!(
            parse_line("CONERR,-3,"),
            Ok(Notification::ConnectionError {
                code: -3,
                message: String::new(),
            })
        );
    }

    #[test]
    fn test_parse_line_reqok_spec_example_ok() {
        // `docs/spec/03-requests.md` §13.1 [p.44].
        assert_eq!(
            parse_line("REQOK,1"),
            Ok(Notification::RequestOk {
                request_id: Some("1".to_owned()),
            })
        );
    }

    #[test]
    fn test_parse_line_reqok_bare_heartbeat_form_ok() {
        // The HTTP `heartbeat` response is a bare `REQOK`
        // [`docs/spec/03-requests.md` §14.3, p.47] — see the SPEC-AMBIGUITY
        // on the `REQOK` arm.
        assert_eq!(
            parse_line("REQOK"),
            Ok(Notification::RequestOk { request_id: None })
        );
    }

    #[test]
    fn test_parse_line_reqok_alphanumeric_request_id_ok() {
        // Request IDs may be "any combination of letters and numbers"
        // [`docs/spec/01-foundations.md` §5.1], so they stay text.
        assert_eq!(
            parse_line("REQOK,a7f3"),
            Ok(Notification::RequestOk {
                request_id: Some("a7f3".to_owned()),
            })
        );
    }

    #[test]
    fn test_parse_line_reqerr_three_arguments_ok() {
        // The declared format `REQERR,<request-ID>,<error-code>,<error-message>`
        // [`docs/spec/03-requests.md` §13.2, p.45], filled with the code and
        // message of the spec's own example [`docs/spec/05-error-codes.md`
        // §2, p.91] and the request ID the example omits.
        assert_eq!(
            parse_line("REQERR,7,19,Specified subscription not found"),
            Ok(Notification::RequestError {
                request_id: "7".to_owned(),
                code: 19,
                message: "Specified subscription not found".to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_line_reqerr_spec_example_is_rejected_as_malformed() {
        // The spec's verbatim example is `REQERR,19,Specified subscription
        // not found` [`docs/spec/03-requests.md` §13.2, p.45] — two arguments
        // where the format declares three, with `19` being the error code and
        // no request ID present. See the SPEC-AMBIGUITY on the `REQERR` arm:
        // the declared count wins and the line is reported rather than
        // silently misread as request ID `19`.
        assert_eq!(
            parse_line("REQERR,19,Specified subscription not found"),
            Err(ProtocolError::ArgumentCount {
                tag: "REQERR".to_owned(),
                expected: 3,
                found: 2,
            })
        );
    }

    #[test]
    fn test_parse_line_reqerr_message_with_commas_is_not_split() {
        // Argument 3 is last and absorbs every further comma
        // [`docs/spec/01-foundations.md` §7.4].
        assert_eq!(
            parse_line("REQERR,7,65,Malformed request, check LS_op, then retry"),
            Ok(Notification::RequestError {
                request_id: "7".to_owned(),
                code: 65,
                message: "Malformed request, check LS_op, then retry".to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_line_reqerr_too_few_arguments_is_error() {
        assert_eq!(
            parse_line("REQERR,19"),
            Err(ProtocolError::ArgumentCount {
                tag: "REQERR".to_owned(),
                expected: 3,
                found: 1,
            })
        );
    }

    #[test]
    fn test_parse_line_error_spec_example_ok() {
        // `docs/spec/03-requests.md` §13.2 [p.45].
        assert_eq!(
            parse_line("ERROR,68,Internal error during request processing"),
            Ok(Notification::Error {
                code: 68,
                message: "Internal error during request processing".to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_line_wsok_spec_example_ok() {
        // `docs/spec/03-requests.md` §15.2 [p.48].
        assert_eq!(parse_line("WSOK"), Ok(Notification::WsOk));
    }

    // ---------------------------------------------------------------------
    // Cross-cutting behaviour
    // ---------------------------------------------------------------------

    #[test]
    fn test_parse_line_unknown_tag_is_reported_not_dropped() {
        // MPN is out of scope for v1 [`docs/spec/06-mpn.md`], so its tags
        // surface as unknown rather than vanishing.
        assert_eq!(
            parse_line("MPNREG,devid,adapter"),
            Err(ProtocolError::UnknownTag {
                tag: "MPNREG".to_owned(),
                line: "MPNREG,devid,adapter".to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_line_unknown_bare_tag_is_reported() {
        assert_eq!(
            parse_line("FUTURE"),
            Err(ProtocolError::UnknownTag {
                tag: "FUTURE".to_owned(),
                line: "FUTURE".to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_line_empty_line_is_unknown_tag() {
        // An empty line has an empty tag. Reporting it is the non-fatal
        // choice; the spec does not describe this case.
        assert_eq!(
            parse_line(""),
            Err(ProtocolError::UnknownTag {
                tag: String::new(),
                line: String::new(),
            })
        );
    }

    #[test]
    fn test_parse_line_unknown_tag_preserves_the_whole_line() {
        // An unknown tag is reported with the line intact, whatever it carries:
        // the arguments are not counted, not split and not decoded, because the
        // argument count of a tag this client does not know is not knowable
        // [`docs/spec/01-foundations.md` §7.1]. That is what lets a caller log
        // or forward the original bytes.
        let line = "FUTURE,1,two,three,with,commas,and%20an%20escape";
        assert_eq!(
            parse_line(line),
            Err(ProtocolError::UnknownTag {
                tag: "FUTURE".to_owned(),
                line: line.to_owned(),
            })
        );

        // The reported line is the one the parser worked on: the terminator the
        // transport left behind is stripped, and nothing else is.
        assert_eq!(
            parse_line("FUTURE,a,b\r\n"),
            Err(ProtocolError::UnknownTag {
                tag: "FUTURE".to_owned(),
                line: "FUTURE,a,b".to_owned(),
            })
        );

        // A tag with a trailing comma and no argument at all still reports the
        // line verbatim rather than an argument-count error.
        assert_eq!(
            parse_line("FUTURE,"),
            Err(ProtocolError::UnknownTag {
                tag: "FUTURE".to_owned(),
                line: "FUTURE,".to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_line_trailing_crlf_is_tolerated() {
        // Framing belongs to the transport, but a stray terminator must not
        // turn a good line into a parse error.
        assert_eq!(parse_line("PROBE\r\n"), Ok(Notification::Probe));
        assert_eq!(
            parse_line("EOS,3,1\n"),
            Ok(Notification::EndOfSnapshot {
                subscription_id: 3,
                item_index: 1,
            })
        );
        assert_eq!(
            parse_line("EOS,3,1\r"),
            Ok(Notification::EndOfSnapshot {
                subscription_id: 3,
                item_index: 1,
            })
        );
    }

    #[test]
    fn test_parse_line_trailing_terminator_stripped_only_once() {
        // Only one terminator is removed, so a `U` value list keeps every
        // other byte the server sent.
        assert_eq!(
            parse_line("U,3,1,a|b\n\r\n"),
            Ok(Notification::Update {
                subscription_id: 3,
                item_index: 1,
                raw_values: "a|b\n".to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_line_numeric_overflow_is_not_numeric_error() {
        // Totality: a value too large for the field's type is an error, not a
        // panic and not a wrapped number.
        assert_eq!(
            parse_line("UNSUB,99999999999999999999"),
            Err(ProtocolError::NotNumeric {
                tag: "UNSUB".to_owned(),
                argument: "subscription-ID",
                value: "99999999999999999999".to_owned(),
            })
        );
    }

    #[test]
    fn test_parse_line_empty_numeric_argument_is_not_numeric_error() {
        assert_eq!(
            parse_line("UNSUB,"),
            Err(ProtocolError::NotNumeric {
                tag: "UNSUB".to_owned(),
                argument: "subscription-ID",
                value: String::new(),
            })
        );
    }
}
