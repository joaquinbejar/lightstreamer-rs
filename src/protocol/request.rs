//! Request encoding: every client-to-server message TLCP defines.
//!
//! One type per request. Each carries its parameters as typed fields, knows
//! its own request name and HTTP path as associated constants, and encodes
//! itself into the `<param>=<value>&...` line both transports share.
//!
//! Source: `docs/spec/03-requests.md` (the complete parameter tables and the
//! spec's own verbatim examples) and `docs/spec/01-foundations.md` §6 (the
//! framing rules for the HTTP and WS transports).
//!
//! # What this module does not do
//!
//! No I/O, no async, no sockets — a request is encoded into a `String` and
//! handed to whoever owns the connection. Choosing *when* to send one, and
//! allocating the `LS_reqId` sequence across a session, belong to the session
//! layer.
//!
//! MPN (mobile push notification) requests are out of scope; see
//! `docs/spec/06-mpn.md`.
//!
//! # Encoding rules honored here
//!
//! - Optional parameters are `Option<T>` and are **omitted entirely** when
//!   `None`, never emitted with an empty value. The server's documented
//!   default then applies, and each field's doc comment says what it is
//!   [`docs/spec/03-requests.md` §2.1, §3.1, §6.1, §11.1, §12.1].
//! - Values are escaped exactly once, at the boundary, through
//!   [`encode_form_value`]. Callers hand this module logical values
//!   [`docs/spec/01-foundations.md` §6.1.3].
//! - Parameter names are verbatim from the spec, including their mixed casing
//!   (`LS_reqId`, `LS_subId`).
//! - Parameter order is not constrained by the spec [`docs/spec/03-requests.md`
//!   §2.2], so the order chosen here is the order of the spec's own request
//!   examples, which lets the tests assert against them byte for byte.
//!
//! # Parameters the server would silently ignore
//!
//! Several parameters are documented not as illegal in some combination, but
//! as *"considered only if …"* or *"ignored if …"*: the server accepts the
//! request and drops the parameter without saying so
//! [`docs/spec/03-requests.md` §6.1, §12.1]. This encoder **refuses** such a
//! combination instead of emitting it, on one policy: a silently dropped
//! parameter yields a subscription or a message that differs from the one the
//! caller asked for, with nothing on the wire and nothing in the response to
//! reveal it, whereas a rejection is visible at the call site.
//!
//! That is stricter than the specification requires — the server would have
//! accepted every one of these requests. It is therefore a reversible policy
//! choice rather than a protocol rule, and each site carries a
//! `// SPEC-AMBIGUITY:` comment saying so, naming the spec sentence it goes
//! beyond. A rejection whose spec sentence reads *"admitted only if …"* is a
//! genuine protocol rule and carries no such marker.
//!
//! `LS_ack` is the single exception to the policy. It is "ignored with HTTP
//! transport" (§6.1) yet entirely meaningful on WS, so a caller that sets it
//! and then encodes for HTTP is not refused: the parameter is simply omitted.
//! Refusing would make a request's validity depend on the transport chosen for
//! it, which the session layer may change under recovery.

use std::fmt;
use std::num::NonZeroU32;

use crate::protocol::ProtocolError;
use crate::protocol::escaping::encode_form_value;

// ---------------------------------------------------------------------------
// Framing constants
// ---------------------------------------------------------------------------

/// The protocol version this crate speaks, as it appears in the required
/// `LS_protocol` query-string parameter and in the WebSocket subprotocol
/// header [`docs/spec/01-foundations.md` §6.1.1, §6.2.1].
// No non-test caller: `LS_protocol` belongs to the HTTP request target, and
// only the WebSocket transport is implemented (`src/transport/ws.rs`).
#[allow(dead_code)]
pub(crate) const PROTOCOL_VERSION: &str = "TLCP-2.5.0";

/// The `Sec-WebSocket-Protocol` value a TLCP WebSocket must be opened with
/// [`docs/spec/01-foundations.md` §6.2.1].
// No in-crate caller: `src/transport/ws.rs` carries its own copy of the
// subprotocol string and the endpoint path.
#[allow(dead_code)]
pub(crate) const WS_SUBPROTOCOL: &str = "TLCP-2.5.0.lightstreamer.com";

/// The WebSocket endpoint path [`docs/spec/01-foundations.md` §6.2.1].
// No in-crate caller: see `WS_SUBPROTOCOL`.
#[allow(dead_code)]
pub(crate) const WS_PATH: &str = "/lightstreamer";

/// The line separator, for both the HTTP body and the WS message
/// [`docs/spec/01-foundations.md` §6.1.1, §6.2.2]. It is always the full
/// `CR-LF` pair, never a bare `CR` or `LF`
/// [`docs/spec/03-requests.md` §1.3].
pub(crate) const CRLF: &str = "\r\n";

/// The client identifier every custom-developed client must send on
/// `create_session` [`docs/spec/03-requests.md` §2.1].
///
/// Emitted **verbatim**, without passing through [`encode_form_value`]: the
/// spec states the value as the literal string below, which already contains a
/// percent-escape, and escaping it again would turn its `%` into `%25` and
/// change the value the server sees. Emitting these exact bytes reproduces the
/// spec's own p.24 examples.
//
// SPEC-AMBIGUITY: `docs/spec/03-requests.md` §2.1 says `LS_cid` "must be set
// with the special string `mgQkwtwdysogQz2BJ4Ji%20kOj2Bg`" and both p.24
// examples show exactly those bytes on the wire, but the spec never says
// whether the logical value is that string or the space-containing
// `mgQkwtwdysogQz2BJ4Ji kOj2Bg` of which it would be the percent-encoding.
// Reproducing the spec's examples byte for byte is the defensive choice. Both
// readings happen to agree with this crate's escaper, which encodes a space as
// `%20`, so the ambiguity has no effect on the bytes sent — but it would
// matter to any escaper that left a space alone, and it is recorded for that
// reason.
pub(crate) const CLIENT_IDENTIFIER: &str = "mgQkwtwdysogQz2BJ4Ji%20kOj2Bg";

// ---------------------------------------------------------------------------
// Transport selection
// ---------------------------------------------------------------------------

/// Which transport's rules apply when encoding a request.
///
/// This is a pure descriptor of the spec's transport-dependent *encoding*
/// rules — which parameters are required, which are ignored, how the request
/// is framed. It carries no connection and performs no I/O.
///
/// Three rules in `docs/spec/03-requests.md` depend on it:
///
/// - `LS_session` is required with HTTP and optional with WS, where it
///   defaults to the last session bound to that WS connection (§3.1, §5.1).
/// - `LS_ack` is meaningful only on WS; with HTTP it is ignored, because an
///   HTTP response is always sent (§6.1, §7.1, §12.1).
/// - With HTTP, a batch of control requests may carry `LS_session` on the
///   query string as a default for every body line (§1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub(crate) enum TransportKind {
    /// A single HTTP request. `LS_session` must be present in the body.
    Http,
    /// One line of an HTTP control batch whose `LS_session` is supplied on the
    /// query string as the default for every line
    /// [`docs/spec/03-requests.md` §1.1].
    // Never constructed: control batching is an HTTP-only shape and only the
    // WebSocket transport is implemented (`src/transport/ws.rs`).
    #[allow(dead_code)]
    HttpBatched,
    /// A WebSocket text message [`docs/spec/01-foundations.md` §6.2.2].
    WebSocket,
}

impl TransportKind {
    /// Whether `LS_session` must appear in this request's own parameter line.
    ///
    /// Only true for a single HTTP request: WS defaults it to the last bound
    /// session, and an HTTP batch may carry it on the query string
    /// [`docs/spec/03-requests.md` §1.1, §5.1].
    #[must_use]
    #[inline]
    const fn requires_session(self) -> bool {
        matches!(self, Self::Http)
    }

    /// Whether `LS_ack` has any effect on this transport.
    ///
    /// `LS_ack` is "ignored if used with HTTP transport, as an HTTP response is
    /// always sent" [`docs/spec/03-requests.md` §6.1], so it is not emitted
    /// there at all rather than emitted and discarded.
    #[must_use]
    #[inline]
    const fn honors_ack(self) -> bool {
        matches!(self, Self::WebSocket)
    }
}

// ---------------------------------------------------------------------------
// Error construction
// ---------------------------------------------------------------------------

/// Builds the error returned when a request's parameters cannot legally be
/// combined, or when a single value violates its documented grammar.
#[cold]
#[inline(never)]
fn invalid(request: &'static str, reason: impl Into<String>) -> ProtocolError {
    ProtocolError::Request {
        request,
        reason: reason.into(),
    }
}

// ---------------------------------------------------------------------------
// Parameter-line construction
// ---------------------------------------------------------------------------

/// Accumulates the `<param1>=<value1>&...&<paramN>=<valueN>` line that both
/// transports carry [`docs/spec/01-foundations.md` §6.1.1, §6.2.2].
#[derive(Debug, Default)]
struct ParameterLine {
    buf: String,
}

impl ParameterLine {
    #[must_use]
    #[inline]
    fn new() -> Self {
        Self { buf: String::new() }
    }

    /// Appends `name=value` with the value taken verbatim.
    ///
    /// Only for values whose grammar excludes every reserved character
    /// (`CR`, `LF`, `&`, `=`, `%`, `+`) — enum op-codes, numbers, booleans, and
    /// the already-encoded [`CLIENT_IDENTIFIER`]
    /// [`docs/spec/01-foundations.md` §6.1.3].
    fn verbatim(&mut self, name: &str, value: &str) {
        if !self.buf.is_empty() {
            self.buf.push('&');
        }
        self.buf.push_str(name);
        self.buf.push('=');
        self.buf.push_str(value);
    }

    /// Appends `name=value`, percent-encoding the value.
    ///
    /// This is the single point at which escaping is applied to a caller-
    /// supplied value [`docs/spec/01-foundations.md` §6.1.3].
    fn text(&mut self, name: &str, value: &str) {
        let encoded = encode_form_value(value);
        self.verbatim(name, encoded.as_ref());
    }

    /// Appends a `true`/`false` parameter.
    fn boolean(&mut self, name: &str, value: bool) {
        self.verbatim(name, if value { "true" } else { "false" });
    }

    /// Appends a numeric parameter. Numbers contain no reserved character.
    fn number<T: fmt::Display>(&mut self, name: &str, value: T) {
        self.verbatim(name, &value.to_string());
    }

    #[must_use]
    #[inline]
    fn finish(self) -> String {
        self.buf
    }
}

// ---------------------------------------------------------------------------
// The request trait
// ---------------------------------------------------------------------------

/// A TLCP request that can be framed for either transport.
pub(crate) trait TlcpRequest {
    /// The request name: the `<request-name>` of the HTTP path and the first
    /// line of the WS message [`docs/spec/01-foundations.md` §6.1.1, §6.2.2].
    const NAME: &'static str;

    /// The HTTP request path, extension included
    /// [`docs/spec/03-requests.md` §16].
    const PATH: &'static str;

    /// Encodes this request's parameter line for the given transport.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::Request`] if the parameters violate a constraint the
    /// specification places on their combination, or if a value required by
    /// this transport is missing.
    fn encode_parameters(&self, transport: TransportKind) -> Result<String, ProtocolError>;

    /// The full HTTP request target, carrying the required `LS_protocol`
    /// query-string parameter [`docs/spec/01-foundations.md` §6.1.1].
    // This and the two helpers below have no in-crate caller: only the
    // WebSocket transport is implemented (`src/transport/ws.rs`), and it uses
    // `ws_message`.
    #[allow(dead_code)]
    #[must_use]
    fn http_target() -> String
    where
        Self: Sized,
    {
        format!("{}?LS_protocol={PROTOCOL_VERSION}", Self::PATH)
    }

    /// The HTTP request target with `LS_session` added to the query string,
    /// where it acts as the default session for every line of a control batch
    /// [`docs/spec/03-requests.md` §1.1].
    #[allow(dead_code)]
    #[must_use]
    fn http_target_with_session(session: &str) -> String
    where
        Self: Sized,
    {
        format!(
            "{}?LS_protocol={PROTOCOL_VERSION}&LS_session={}",
            Self::PATH,
            encode_form_value(session)
        )
    }

    /// The body of a single HTTP request.
    ///
    /// # Errors
    ///
    /// As [`TlcpRequest::encode_parameters`].
    #[allow(dead_code)]
    fn http_body(&self) -> Result<String, ProtocolError> {
        self.encode_parameters(TransportKind::Http)
    }

    /// The complete WS text message: the request name, `CR-LF`, then the
    /// parameter line [`docs/spec/01-foundations.md` §6.2.2].
    ///
    /// When the parameter line is empty the trailing `CR-LF` is still present,
    /// because the separator is optional after the last line only when that
    /// line is non-empty [`docs/spec/01-foundations.md` §6.2.2;
    /// `docs/spec/03-requests.md` §14.2].
    ///
    /// # Errors
    ///
    /// As [`TlcpRequest::encode_parameters`].
    fn ws_message(&self) -> Result<String, ProtocolError> {
        let parameters = self.encode_parameters(TransportKind::WebSocket)?;
        Ok(format!("{}{CRLF}{parameters}", Self::NAME))
    }
}

/// Joins already-encoded parameter lines into an HTTP control-batch body.
///
/// Each line is a separate control request; the separator is `CR-LF` and is
/// omitted after the last line, which is never empty here
/// [`docs/spec/03-requests.md` §1.1]. The requests of a batch are executed
/// concurrently and their responses may arrive out of order.
///
/// # Errors
///
/// [`ProtocolError::Request`] if `lines` is empty: an empty body would encode
/// not as a batch of zero requests but as one request with no parameters,
/// which no control request accepts — every one of them requires at least
/// `LS_reqId` and `LS_op` [`docs/spec/03-requests.md` §5.1].
//
// SPEC-AMBIGUITY: the specification neither permits nor forbids an empty
// batch. `docs/spec/01-foundations.md` §6.1.2 flags that its own rule — "the
// separator is optional after the last line, unless the line is empty (which
// is possible, in principle)" — implies a control request *may* be encoded as
// an empty line, while never describing what such a line would mean. Refusing
// is the defensive reading; the alternative would be to emit a body whose
// meaning the spec does not define.
pub(crate) fn encode_http_batch(
    request_name: &'static str,
    lines: &[String],
) -> Result<String, ProtocolError> {
    if lines.is_empty() {
        return Err(invalid(request_name, "a request batch must not be empty"));
    }
    Ok(lines.join(CRLF))
}

/// Frames already-encoded parameter lines into a single WS batch message.
///
/// The request name is on its own line, followed by one line per request
/// [`docs/spec/01-foundations.md` §6.2.3].
///
/// # Errors
///
/// [`ProtocolError::Request`] if `lines` is empty.
pub(crate) fn encode_ws_batch(
    request_name: &'static str,
    lines: &[String],
) -> Result<String, ProtocolError> {
    let body = encode_http_batch(request_name, lines)?;
    Ok(format!("{request_name}{CRLF}{body}"))
}

// ---------------------------------------------------------------------------
// Validated value types
// ---------------------------------------------------------------------------

/// A request ID: "any combination of letters and numbers", unique within the
/// connection [`docs/spec/03-requests.md` §5.1].
///
/// The server echoes it in `REQOK`, `REQERR` and the message notifications, so
/// it is the only way a client correlates a control response with the request
/// that caused it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct RequestId(String);

impl RequestId {
    /// Validates a request ID.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::Request`] if the ID contains anything other than ASCII
    /// letters and digits, or is empty. Both follow from the value syntax
    /// "**Any combination of letters and numbers**" for a parameter the same
    /// table marks Required [`docs/spec/03-requests.md` §5.1, p.29]: an empty
    /// string is neither a letter nor a number, and an empty value is how an
    /// absent parameter would encode.
    #[must_use = "builders do nothing unless the result is used"]
    pub(crate) fn try_new(id: impl Into<String>) -> Result<Self, ProtocolError> {
        let id = id.into();
        if id.is_empty() {
            return Err(invalid("control", "LS_reqId must not be empty"));
        }
        if !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(invalid(
                "control",
                format!("LS_reqId must be letters and digits only, got {id:?}"),
            ));
        }
        Ok(Self(id))
    }

    /// Builds a request ID from the progressive number clients typically use
    /// [`docs/spec/03-requests.md` §5.1].
    #[must_use]
    pub(crate) fn from_progressive(n: u64) -> Self {
        Self(n.to_string())
    }

    /// The ID as it goes on the wire.
    #[must_use]
    #[inline]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A decimal number as the wire carries it: digits with a **dot** as the
/// decimal separator [`docs/spec/03-requests.md` §2.1, §6.1, §8.1, §9.1].
///
/// Held as text rather than as a float on purpose. The spec's own examples
/// write `2.0` and `50.0`; a `f64` round-trip would emit `2` and `50` and lose
/// the exact form the server was shown in the specification. Keeping the text
/// preserves wire fidelity and costs nothing, since the client never does
/// arithmetic on these values.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct DecimalNumber(String);

impl DecimalNumber {
    /// Validates a decimal number.
    ///
    /// Accepts `123` and `123.45`; rejects an empty string, a sign, an
    /// exponent, a comma separator, a leading or trailing dot, and more than
    /// one dot.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::Request`] if the text is not such a number.
    //
    // SPEC-AMBIGUITY: `docs/spec/03-requests.md` §2.1/§6.1/§8.1/§9.1 say only
    // "a decimal number, with a dot as decimal separator" and never give the
    // grammar. Whether `.5`, `5.`, a leading `+`, or an exponent are accepted
    // is unstated; all are rejected here, since every quantity these values
    // express (kbps, updates/sec) is a non-negative magnitude and the spec's
    // examples use the plain `<digits>.<digits>` form.
    #[must_use = "builders do nothing unless the result is used"]
    pub(crate) fn try_new(text: impl Into<String>) -> Result<Self, ProtocolError> {
        let text = text.into();
        let mut parts = text.split('.');
        let integral = parts.next().unwrap_or_default();
        let fractional = parts.next();
        let well_formed = parts.next().is_none()
            && !integral.is_empty()
            && integral.bytes().all(|b| b.is_ascii_digit())
            && fractional.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()));

        if !well_formed {
            return Err(invalid(
                "control",
                format!("expected a decimal number with a dot separator, got {text:?}"),
            ));
        }
        Ok(Self(text))
    }

    /// The number as it goes on the wire.
    #[must_use]
    #[inline]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DecimalNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The name of a message sequence: an alphanumeric identifier, underscores
/// allowed [`docs/spec/03-requests.md` §12.1].
///
/// Messages sharing a sequence name are processed in order of their
/// `LS_msg_prog`. `UNORDERED_MESSAGES` is reserved by the protocol and is
/// rejected here rather than being sent for the server to refuse.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct SequenceName(String);

impl SequenceName {
    /// The identifier reserved by earlier text protocols.
    ///
    /// `docs/spec/03-requests.md` §12.1 states it twice: the `LS_sequence`
    /// row of the parameter table (p.43) reads "The special identifier
    /// `UNORDERED_MESSAGES` is **reserved and can't be used**; if specified it
    /// leads to an error", and the `LS_sequence` notes on the same page repeat
    /// "The special identifier `UNORDERED_MESSAGES`, used in previous text
    /// protocols, is now **reserved** and can't be used. If specified it leads
    /// to an error." Rejecting it here is therefore the protocol's own rule,
    /// not this encoder's policy — the server would refuse the request.
    pub(crate) const RESERVED: &'static str = "UNORDERED_MESSAGES";

    /// Validates a sequence name.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::Request`] if the name is empty, contains a character
    /// other than an ASCII letter, digit or underscore, or is
    /// [`SequenceName::RESERVED`]. All three follow from the value syntax
    /// "An **alphanumeric identifier** (the **underscore** character is also
    /// allowed)" [`docs/spec/03-requests.md` §12.1, p.43].
    #[must_use = "builders do nothing unless the result is used"]
    pub(crate) fn try_new(name: impl Into<String>) -> Result<Self, ProtocolError> {
        let name = name.into();
        if name.is_empty() {
            return Err(invalid("msg", "LS_sequence must not be empty"));
        }
        if name == Self::RESERVED {
            return Err(invalid(
                "msg",
                format!(
                    "LS_sequence {:?} is reserved by the protocol",
                    Self::RESERVED
                ),
            ));
        }
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(invalid(
                "msg",
                format!("LS_sequence must be alphanumeric or underscore, got {name:?}"),
            ));
        }
        Ok(Self(name))
    }

    /// The name as it goes on the wire.
    #[must_use]
    #[inline]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// The cause code reported in the `END` notification a `destroy` provokes
/// [`docs/spec/03-requests.md` §11.1].
///
/// "Only `0` or a negative code are supported; a positive code will be
/// collapsed to `0`" (§11.1, p.35). Construction rejects a positive code
/// rather than sending a value the server will silently rewrite into one the
/// caller did not ask for.
//
// SPEC-AMBIGUITY: that sentence describes a server that *accepts* a positive
// code and rewrites it, not one that refuses the request — so this rejection
// goes beyond the specification, under the module-level policy on parameters
// the server would silently alter. It is reversible: dropping the check and
// sending the value as given would also be spec-conformant, at the cost of an
// `END` whose code does not match the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct CauseCode(i32);

impl CauseCode {
    /// Validates a cause code.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::Request`] if the code is positive
    /// [`docs/spec/03-requests.md` §11.1].
    // No in-crate caller: `destroy` is sent without a cause by the session
    // layer, so nothing constructs a `CauseCode` outside the tests.
    #[allow(dead_code)]
    #[must_use = "builders do nothing unless the result is used"]
    pub(crate) fn try_new(code: i32) -> Result<Self, ProtocolError> {
        if code > 0 {
            return Err(invalid(
                CONTROL_NAME,
                format!("LS_cause_code must be zero or negative, got {code}"),
            ));
        }
        Ok(Self(code))
    }

    /// The code as an integer.
    #[must_use]
    #[inline]
    pub(crate) const fn get(self) -> i32 {
        self.0
    }
}

/// The cause message reported in the `END` notification a `destroy` provokes
/// [`docs/spec/03-requests.md` §11.1].
///
/// The spec allows it only alongside a cause code, which
/// [`DestroyCause`] enforces structurally.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct CauseMessage(String);

impl CauseMessage {
    /// The length past which the server may replace the message with
    /// `[Custom message skipped]`, if a content-length limit is in force
    /// [`docs/spec/03-requests.md` §11.1].
    // No in-crate caller, for the same reason as `CauseCode::try_new`.
    #[allow(dead_code)]
    pub(crate) const RECOMMENDED_MAX_LEN: usize = 35;

    /// Validates a cause message.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::Request`] if the message contains `CR` or `LF`:
    /// "multiline text is also not allowed"
    /// [`docs/spec/03-requests.md` §11.1].
    //
    // The spec's other two remarks — the message "should be in simple ASCII,
    // otherwise it might be altered" and "should be short" — are advisory
    // ("should", with a stated server-side fallback), so they are documented
    // on `RECOMMENDED_MAX_LEN` rather than enforced. Only the flat
    // prohibition on multiline text is a hard rejection.
    //
    // No in-crate caller, for the same reason as `CauseCode::try_new`.
    #[allow(dead_code)]
    #[must_use = "builders do nothing unless the result is used"]
    pub(crate) fn try_new(message: impl Into<String>) -> Result<Self, ProtocolError> {
        let message = message.into();
        if message.contains(['\r', '\n']) {
            return Err(invalid(
                "control",
                "LS_cause_message must not contain CR or LF: multiline text is not allowed",
            ));
        }
        Ok(Self(message))
    }

    /// The message as a logical (unescaped) value.
    #[must_use]
    #[inline]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// The cause a `destroy` asks the server to report in its `END` notification.
///
/// Pairing the code with an optional message makes the spec's
/// "`LS_cause_message` is considered only if `LS_cause_code` is also supplied"
/// a property of the type: a message without a code cannot be constructed
/// [`docs/spec/03-requests.md` §11.2].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct DestroyCause {
    /// The `LS_cause_code` to report in place of the default code `31`.
    pub(crate) code: CauseCode,
    /// The `LS_cause_message` to report. When `None`, the server reports the
    /// message as `null` [`docs/spec/03-requests.md` §11.1].
    pub(crate) message: Option<CauseMessage>,
}

// ---------------------------------------------------------------------------
// Enumerated parameter values
// ---------------------------------------------------------------------------

/// The subscription mode of every item in a subscription
/// [`docs/spec/03-requests.md` §6.1, `LS_mode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub(crate) enum SubscriptionMode {
    /// Every update is forwarded as-is, with no server-side filtering and no
    /// item state kept.
    Raw,
    /// The server keeps the current value of each field and merges updates
    /// into it; a snapshot is the current state.
    Merge,
    /// Every update is a distinct event; a snapshot is the most recent events.
    Distinct,
    /// Updates carry `ADD`/`UPDATE`/`DELETE` commands keyed by a key field.
    Command,
}

impl SubscriptionMode {
    /// The op-code as it goes on the wire.
    #[must_use]
    #[inline]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Raw => "RAW",
            Self::Merge => "MERGE",
            Self::Distinct => "DISTINCT",
            Self::Command => "COMMAND",
        }
    }

    /// Whether the server applies frequency filtering, buffering and snapshot
    /// management in this mode. `RAW` is the sole exception
    /// [`docs/spec/03-requests.md` §6.1].
    #[must_use]
    #[inline]
    const fn is_filtered(self) -> bool {
        !matches!(self, Self::Raw)
    }
}

/// The `LS_op` of a control request [`docs/spec/03-requests.md` §5.2].
///
/// MPN op-codes are deliberately absent; see `docs/spec/06-mpn.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub(crate) enum ControlOperation {
    /// Add a new subscription to the session.
    Add,
    /// Delete an existing subscription from the session.
    Delete,
    /// Reconfigure an existing subscription.
    Reconf,
    /// Change the constraints of an existing session.
    // Never constructed: the client surface does not expose a runtime
    // bandwidth change, so nothing builds a `ConstrainSession`.
    #[allow(dead_code)]
    Constrain,
    /// Force an existing session to rebind.
    ForceRebind,
    /// Terminate an existing session.
    Destroy,
}

impl ControlOperation {
    /// The op-code as it goes on the wire.
    #[must_use]
    #[inline]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Delete => "delete",
            Self::Reconf => "reconf",
            Self::Constrain => "constrain",
            Self::ForceRebind => "force_rebind",
            Self::Destroy => "destroy",
        }
    }
}

/// The value of `LS_requested_max_frequency` on a subscription request
/// [`docs/spec/03-requests.md` §6.1].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum RequestedMaxFrequency {
    /// `unlimited` — each update is forwarded as soon as possible, with no
    /// frequency limit. This is also the server's default when the parameter
    /// is omitted.
    Unlimited,
    /// `unfiltered` — each update is forwarded as soon as possible and
    /// **without losses**; any loss is signalled by an `OV` notification.
    /// Suppresses `LS_requested_buffer_size`.
    Unfiltered,
    /// A maximum number of updates per second.
    Limited(DecimalNumber),
}

impl RequestedMaxFrequency {
    /// The value as it goes on the wire.
    #[must_use]
    fn as_str(&self) -> &str {
        match self {
            Self::Unlimited => "unlimited",
            Self::Unfiltered => "unfiltered",
            Self::Limited(n) => n.as_str(),
        }
    }
}

/// The value of `LS_requested_max_frequency` on a **reconfiguration** request
/// [`docs/spec/03-requests.md` §8.1].
///
/// A separate type from [`RequestedMaxFrequency`] because `reconf` documents
/// only a decimal number or `unlimited`; `unfiltered` is not among its values,
/// and the request is in any case "admitted only if the subscription is not
/// subscribed to with `unfiltered` max frequency". Making `unfiltered`
/// unrepresentable here removes the possibility of asking for it.
// Never constructed outside the tests: the client surface reconfigures a
// subscription's frequency through `RequestedMaxFrequency`, and this narrower
// type exists so that `reconf` cannot be asked for `unfiltered`.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum MaxFrequencyLimit {
    /// `unlimited` — relieve any existing frequency limit.
    Unlimited,
    /// A maximum number of updates per second.
    Limited(DecimalNumber),
}

impl MaxFrequencyLimit {
    /// The value as it goes on the wire.
    #[must_use]
    fn as_str(&self) -> &str {
        match self {
            Self::Unlimited => "unlimited",
            Self::Limited(n) => n.as_str(),
        }
    }
}

/// The value of `LS_requested_max_bandwidth` on a session constrain request,
/// expressed in kbps [`docs/spec/03-requests.md` §9.1].
// No in-crate caller: the client surface does not expose a runtime bandwidth
// change, so nothing builds a `ConstrainSession` to carry this.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum MaxBandwidth {
    /// `unlimited` — relieve any existing bandwidth limit.
    Unlimited,
    /// A maximum bandwidth in kbps.
    Limited(DecimalNumber),
}

impl MaxBandwidth {
    /// The value as it goes on the wire.
    #[allow(dead_code)]
    #[must_use]
    fn as_str(&self) -> &str {
        match self {
            Self::Unlimited => "unlimited",
            Self::Limited(n) => n.as_str(),
        }
    }
}

/// The value of `LS_requested_buffer_size` [`docs/spec/03-requests.md` §6.1].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum RequestedBufferSize {
    /// `unlimited` — request no limit on the buffer. The server may still
    /// impose one.
    Unlimited,
    /// A number of update events.
    //
    // SPEC-AMBIGUITY: `docs/spec/03-requests.md` §6.1 gives the value syntax
    // as "number of update events, or the literal `unlimited`" without saying
    // whether `0` is admitted or what it would mean for a parameter described
    // as the "dimension of the buffers". `NonZeroU32` makes it unaskable.
    Events(NonZeroU32),
}

/// The value of `LS_snapshot` [`docs/spec/03-requests.md` §6.1].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Snapshot {
    /// `false` — the server does not send a snapshot. This is also the
    /// server's default when the parameter is omitted.
    Off,
    /// `true` — the server sends the snapshot, possibly empty. Considered only
    /// for `MERGE`, `DISTINCT` and `COMMAND`; with `DISTINCT` the length is
    /// decided by the server.
    On,
    /// A maximum number of snapshot events. Admitted **only** with
    /// `LS_mode=DISTINCT`. Fewer events may arrive if the server imposes a
    /// stricter limit or has fewer available.
    Length(NonZeroU32),
}

/// The value of `LS_ttl_millis` on a session creation request
/// [`docs/spec/03-requests.md` §2.1].
// Never constructed outside the tests: `LS_ttl_millis` is not among the
// connection options the client surface exposes.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum TimeToLive {
    /// `unknown` — the request is kept until completion. This is also the
    /// server's default behaviour when the parameter is omitted.
    Unknown,
    /// `unlimited` — like `unknown`, but the request is kept even if the
    /// server is overloaded.
    Unlimited,
    /// A time limit in milliseconds. If it cannot be obeyed, the server may
    /// interrupt processing and discard the request.
    Millis(u64),
}

impl fmt::Display for TimeToLive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown => f.write_str("unknown"),
            Self::Unlimited => f.write_str("unlimited"),
            Self::Millis(ms) => write!(f, "{ms}"),
        }
    }
}

/// Whether the stream connection is a streaming or a polling connection, with
/// the parameters that belong to each.
///
/// `LS_polling=true` enables `LS_polling_millis` and `LS_idle_millis` and
/// disables `LS_inactivity_millis`, `LS_keepalive_millis` and `LS_send_sync`;
/// when `LS_polling` is not `true` the converse holds. The two groups are
/// therefore mutually exclusive, and modelling them as one enum makes an
/// illegal mixture unrepresentable
/// [`docs/spec/03-requests.md` §2.2, §3.2].
///
/// `LS_polling_millis` is "required by the Server in the polling case", so it
/// is a plain field rather than an `Option`
/// [`docs/spec/03-requests.md` §2.2].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum ConnectionMode {
    /// A streaming connection: the server holds the connection open and
    /// pushes notifications as they occur. This is the default; `LS_polling`
    /// is omitted entirely rather than sent as `false`.
    Streaming {
        /// `LS_inactivity_millis`: the longest the client commits to go
        /// without issuing a request while this connection is open. A
        /// `heartbeat` pseudo-request satisfies it when nothing else is
        /// needed. Exceeding it lets the server conclude the client is stuck.
        /// Omitted: the server does not apply this check.
        inactivity_millis: Option<u64>,
        /// `LS_keepalive_millis`: the longest inactivity allowed on the
        /// connection, after which the server sends a `PROBE`. Too low or too
        /// high values are clamped to the server's configured bounds, and the
        /// value actually used is reported in the `CONOK` response. Omitted:
        /// decided by the server from its own configuration.
        keepalive_millis: Option<u64>,
        /// `LS_send_sync`: `false` instructs the server not to send `SYNC`
        /// notifications on this connection. Omitted: the server sends them
        /// (the default is `true`).
        send_sync: Option<bool>,
    },
    /// A polling connection: the server sends only the notifications ready at
    /// connection time and exits immediately, keeping the session active for
    /// subsequent rebinds. Encoded as `LS_polling=true`.
    // Never constructed: polling is an HTTP shape and only the WebSocket
    // streaming transport is implemented (`src/transport/ws.rs`).
    #[allow(dead_code)]
    Polling {
        /// `LS_polling_millis`: the expected time between the closing of this
        /// connection and the next polling connection. Required by the server
        /// in order to keep the underlying session active across polling
        /// cycles; the timeout actually used is reported in the response.
        polling_millis: u64,
        /// `LS_idle_millis`: how long the server may wait for a notification
        /// if none is ready at request time. Omitted or zero: the response is
        /// synchronous and may be empty. Positive: the response is
        /// asynchronous and may still be empty if the timeout expires.
        idle_millis: Option<u64>,
    },
}

impl Default for ConnectionMode {
    /// A streaming connection with every optional parameter left to the
    /// server, which is what omitting `LS_polling` and its companions means
    /// [`docs/spec/03-requests.md` §2.1].
    fn default() -> Self {
        Self::Streaming {
            inactivity_millis: None,
            keepalive_millis: None,
            send_sync: None,
        }
    }
}

impl ConnectionMode {
    /// Appends this group's parameters.
    ///
    /// Order follows `docs/spec/03-requests.md` §2.1: the polling switch, then
    /// the parameters it enables.
    fn encode_into(&self, params: &mut ParameterLine) {
        match self {
            Self::Streaming {
                inactivity_millis,
                keepalive_millis,
                send_sync,
            } => {
                // `LS_polling` is not emitted at all: "not a polling
                // connection" is precisely its documented default, and
                // `LS_polling=false` is nowhere given as a value
                // [`docs/spec/03-requests.md` §2.1].
                if let Some(ms) = inactivity_millis {
                    params.number("LS_inactivity_millis", ms);
                }
                if let Some(ms) = keepalive_millis {
                    params.number("LS_keepalive_millis", ms);
                }
                if let Some(send) = send_sync {
                    params.boolean("LS_send_sync", *send);
                }
            }
            Self::Polling {
                polling_millis,
                idle_millis,
            } => {
                params.boolean("LS_polling", true);
                params.number("LS_polling_millis", polling_millis);
                if let Some(ms) = idle_millis {
                    params.number("LS_idle_millis", ms);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Session creation
// ---------------------------------------------------------------------------

/// `create_session` — obtain an initial session ID and stream connection
/// [`docs/spec/03-requests.md` §2].
///
/// Two parameters have no field:
///
/// - `LS_cid` is fixed by the spec for all custom-developed clients and is
///   always emitted as [`CLIENT_IDENTIFIER`] (§2.1).
/// - `LS_supported_diffs` is derived from the decoder registry rather than
///   configured, so the advertised set can never exceed the implemented set
///   (`docs/adr/0004-supported-diffs-derived-from-decoders.md`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct CreateSession {
    /// `LS_user`: the user name, interpreted and verified by the Metadata
    /// Adapter. Omitted: a null user name is passed to the Metadata Adapter,
    /// which is still asked to authenticate the request.
    pub(crate) user: Option<String>,
    /// `LS_password`: the password, interpreted and verified by the Metadata
    /// Adapter. Omitted: no password is supplied.
    pub(crate) password: Option<String>,
    /// `LS_adapter_set`: the logical name of the Adapter Set that will serve
    /// this session. Omitted: an Adapter Set named `DEFAULT` is assumed.
    pub(crate) adapter_set: Option<String>,
    /// `LS_requested_max_bandwidth`: the maximum bandwidth requested, in kbps.
    /// Omitted: no client-side limit is requested. Bandwidth Control is an
    /// optional server feature.
    //
    // SPEC-AMBIGUITY: `docs/spec/03-requests.md` documents the literal
    // `unlimited` for `LS_requested_max_bandwidth` on `constrain` (§9.1) but
    // not on `create_session` (§2.1). Only the decimal form is offered here;
    // a caller wanting no limit omits the parameter, which is its documented
    // default.
    pub(crate) requested_max_bandwidth: Option<DecimalNumber>,
    /// `LS_content_length`: the content length to use for the connection
    /// content, in bytes. Omitted: assigned by the server from its own
    /// configuration. A value the server considers too low is also
    /// overridden.
    pub(crate) content_length: Option<u64>,
    /// Whether this is a streaming or a polling connection, with the
    /// parameters belonging to that group.
    pub(crate) connection: ConnectionMode,
    /// `LS_reduce_head`: `true` instructs the server not to send `SERVNAME`
    /// and `CLIENTIP` on this connection, and not to send `CONS` throughout
    /// the whole session. Omitted: the default is `false` and all three are
    /// sent.
    //
    // SPEC-AMBIGUITY: `docs/spec/03-requests.md` §3.2 records that
    // `LS_reduce_head` suppresses a different notification set on
    // `create_session` (`SERVNAME`, `CLIENTIP`, and `CONS` session-wide) than
    // on `bind_session` (`CONOK`, `SERVNAME`, `CLIENTIP`), and does not say
    // whether a value set here persists into later binds that omit it. This
    // encoder therefore never infers the parameter for a subsequent
    // `bind_session`; the caller sets it per request.
    pub(crate) reduce_head: Option<bool>,
    /// `LS_ttl_millis`: how long the client commits to wait for an answer
    /// before giving up, letting the server discard a request it cannot serve
    /// in time. Omitted: the `unknown` behaviour applies and the request is
    /// kept until completion.
    pub(crate) ttl: Option<TimeToLive>,
}

impl TlcpRequest for CreateSession {
    const NAME: &'static str = "create_session";
    const PATH: &'static str = "/lightstreamer/create_session.txt";

    fn encode_parameters(&self, _transport: TransportKind) -> Result<String, ProtocolError> {
        let mut params = ParameterLine::new();

        // Order below reproduces the spec's own p.24 example:
        // LS_user, LS_password, LS_adapter_set, LS_cid [§2.3].

        // `LS_user` — optional [§2.1].
        if let Some(user) = &self.user {
            params.text("LS_user", user);
        }
        // `LS_password` — optional [§2.1].
        if let Some(password) = &self.password {
            params.text("LS_password", password);
        }
        // `LS_adapter_set` — optional; `DEFAULT` when omitted [§2.1].
        if let Some(adapter_set) = &self.adapter_set {
            params.text("LS_adapter_set", adapter_set);
        }
        // `LS_cid` — required, and fixed for custom-developed clients [§2.1].
        // Verbatim: the value is already in its wire form.
        params.verbatim("LS_cid", CLIENT_IDENTIFIER);

        // `LS_requested_max_bandwidth` — optional, in kbps [§2.1].
        if let Some(bandwidth) = &self.requested_max_bandwidth {
            params.verbatim("LS_requested_max_bandwidth", bandwidth.as_str());
        }
        // `LS_content_length` — optional, in bytes [§2.1].
        if let Some(length) = self.content_length {
            params.number("LS_content_length", length);
        }

        // `LS_supported_diffs` — optional, a comma-separated list of the diff
        // format tags this client can decode [§2.1]. Derived from the decoder
        // registry, never written by hand, so the advertisement can never
        // exceed the implementation
        // (`docs/adr/0004-supported-diffs-derived-from-decoders.md`).
        //
        // An empty registry means the parameter is omitted rather than sent
        // empty. The spec does allow "a (possibly empty) comma-separated
        // list", but an empty value and an absent parameter mean opposite
        // things here: absent leaves the server free to choose any algorithm,
        // which is the correct behaviour when this client can decode none,
        // whereas the encoder must never emit an empty value that could be
        // read as a declaration.
        let diffs = crate::protocol::diff::supported_diffs();
        if !diffs.is_empty() {
            // Verbatim: the value is a comma-separated list of single-letter
            // format tags [`docs/spec/04-notifications.md` §2.3], which
            // contains no reserved character.
            params.verbatim("LS_supported_diffs", diffs);
        }

        // The polling/streaming group [§2.1, §2.2].
        self.connection.encode_into(&mut params);

        // `LS_reduce_head` — optional; `false` when omitted [§2.1].
        if let Some(reduce) = self.reduce_head {
            params.boolean("LS_reduce_head", reduce);
        }
        // `LS_ttl_millis` — optional; `unknown` when omitted [§2.1].
        if let Some(ttl) = self.ttl {
            params.verbatim("LS_ttl_millis", &ttl.to_string());
        }

        Ok(params.finish())
    }
}

// ---------------------------------------------------------------------------
// Session binding
// ---------------------------------------------------------------------------

/// `bind_session` — rebind an existing session to a new stream connection
/// [`docs/spec/03-requests.md` §3].
///
/// `bind_session` accepts none of `LS_ttl_millis`, `LS_cid`, `LS_user`,
/// `LS_password`, `LS_adapter_set`, `LS_requested_max_bandwidth` or
/// `LS_supported_diffs`; those exist only on `create_session` (§3.2), which is
/// why this struct has no fields for them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct BindSession {
    /// `LS_session`: the session to bind. **Required with HTTP.** Omitted with
    /// WS: the rebind applies to the last session bound to that WS
    /// connection, which requires that a preceding creation or binding
    /// request succeeded on it.
    pub(crate) session: Option<String>,
    /// `LS_recovery_from`: the progressive number of the last data
    /// notification received in this session, i.e. the total number received
    /// since the session began. Causes a `PROG` notification to precede any
    /// data notification in the response. Omitted: the server assumes every
    /// update sent on the previous connection (up to its `LOOP`) was received
    /// and restarts from the next one.
    pub(crate) recovery_from: Option<u64>,
    /// `LS_content_length`: the content length for the connection content.
    /// Omitted: assigned by the server; a value it considers too low is also
    /// overridden.
    //
    // SPEC-AMBIGUITY: `docs/spec/03-requests.md` §3.2 records that the units
    // stated on `create_session` ("in bytes" here, "in milliseconds" for the
    // timing parameters) are omitted from the `bind_session` table. Identity
    // of units across the two requests is implied but not stated; the same
    // Rust types are used for both on that reading.
    pub(crate) content_length: Option<u64>,
    /// Whether this is a streaming or a polling connection, with the
    /// parameters belonging to that group.
    pub(crate) connection: ConnectionMode,
    /// `LS_reduce_head`: `true` instructs the server not to send `CONOK`,
    /// `SERVNAME` and `CLIENTIP` on this connection. Omitted: the default is
    /// `false` and all three are sent.
    pub(crate) reduce_head: Option<bool>,
}

impl TlcpRequest for BindSession {
    const NAME: &'static str = "bind_session";
    const PATH: &'static str = "/lightstreamer/bind_session.txt";

    fn encode_parameters(&self, transport: TransportKind) -> Result<String, ProtocolError> {
        let mut params = ParameterLine::new();

        // `LS_session` — "always required with HTTP transport" [§3.1]. The
        // query-string default of §1.1 is introduced for control batches, so
        // it is not relied on here for either HTTP form.
        match (&self.session, transport) {
            (Some(session), _) => params.text("LS_session", session),
            (None, TransportKind::WebSocket) => {}
            (None, TransportKind::Http | TransportKind::HttpBatched) => {
                return Err(invalid(
                    Self::NAME,
                    "LS_session is required with HTTP transport",
                ));
            }
        }

        // `LS_recovery_from` — optional [§3.1].
        if let Some(progressive) = self.recovery_from {
            params.number("LS_recovery_from", progressive);
        }
        // `LS_content_length` — optional [§3.1].
        if let Some(length) = self.content_length {
            params.number("LS_content_length", length);
        }
        // The polling/streaming group [§3.1, §3.2].
        self.connection.encode_into(&mut params);
        // `LS_reduce_head` — optional; `false` when omitted [§3.1].
        if let Some(reduce) = self.reduce_head {
            params.boolean("LS_reduce_head", reduce);
        }

        Ok(params.finish())
    }
}

// ---------------------------------------------------------------------------
// Control requests — shared header
// ---------------------------------------------------------------------------

/// The request name shared by every control operation
/// [`docs/spec/03-requests.md` §5].
const CONTROL_NAME: &str = "control";

/// The HTTP path shared by every control operation
/// [`docs/spec/03-requests.md` §5].
const CONTROL_PATH: &str = "/lightstreamer/control.txt";

/// Emits the `LS_session`, `LS_reqId` and `LS_op` triple every control request
/// begins with [`docs/spec/03-requests.md` §5.1], in the order of the spec's
/// own examples (§6.4, §7.3, §8.3, §9.3, §10.3, §11.4).
fn encode_control_header(
    params: &mut ParameterLine,
    session: Option<&str>,
    request_id: &RequestId,
    operation: ControlOperation,
    transport: TransportKind,
) -> Result<(), ProtocolError> {
    // `LS_session` — required with HTTP; with WS it defaults to the last
    // session bound to the connection. With an HTTP batch it may instead sit
    // on the query string, in which case it is optional here [§1.1, §5.1].
    match session {
        Some(session) => params.text("LS_session", session),
        None if transport.requires_session() => {
            return Err(invalid(
                CONTROL_NAME,
                "LS_session is required with HTTP transport unless it is supplied on the query string of a batch",
            ));
        }
        None => {}
    }

    // `LS_reqId` — required, unique within the connection [§5.1]. Verbatim:
    // `RequestId` admits only letters and digits.
    params.verbatim("LS_reqId", request_id.as_str());
    // `LS_op` — required [§5.1].
    params.verbatim("LS_op", operation.as_str());
    Ok(())
}

/// Emits `LS_ack`, which is meaningful only on WS.
///
/// "Ignored if used with HTTP transport, as an HTTP response is always sent"
/// [`docs/spec/03-requests.md` §6.1, §7.1, §12.1], so it is left out entirely
/// on HTTP rather than sent for the server to discard.
fn encode_ack(params: &mut ParameterLine, ack: Option<bool>, transport: TransportKind) {
    if !transport.honors_ack() {
        return;
    }
    if let Some(ack) = ack {
        params.boolean("LS_ack", ack);
    }
}

// ---------------------------------------------------------------------------
// Control: add
// ---------------------------------------------------------------------------

/// `control` with `LS_op=add` — add a new subscription to the session
/// [`docs/spec/03-requests.md` §6].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Subscribe {
    /// `LS_session`: see the control-request common parameters. Required with HTTP unless
    /// supplied on the query string of a batch; on WS it defaults to the last
    /// bound session.
    pub(crate) session: Option<String>,
    /// `LS_reqId`: the ID the server echoes in `REQOK`/`REQERR`.
    pub(crate) request_id: RequestId,
    /// `LS_subId`: a progressive integer starting at 1, unique within the
    /// session. The server tags every notification for this subscription with
    /// it.
    pub(crate) subscription_id: NonZeroU32,
    /// `LS_group`: the item-group name, interpreted by the Metadata Adapter.
    /// Required.
    pub(crate) group: String,
    /// `LS_schema`: the field-schema name, interpreted by the Metadata
    /// Adapter. Required.
    pub(crate) schema: String,
    /// `LS_mode`: the subscription mode of every item in the subscription.
    /// Required.
    pub(crate) mode: SubscriptionMode,
    /// `LS_data_adapter`: the Data Adapter that will provide the data; it must
    /// supply **all** the requested items. Omitted: a Data Adapter named
    /// `DEFAULT` within the Adapter Set is assumed.
    pub(crate) data_adapter: Option<String>,
    /// `LS_selector`: the name of a selector related to the items, interpreted
    /// by the Metadata Adapter. Omitted: no selector is applied.
    pub(crate) selector: Option<String>,
    /// `LS_requested_buffer_size`: the dimension of the per-item buffers.
    /// Omitted: `1` for `MERGE`, `unlimited` for `DISTINCT`. Considered only
    /// when the mode is `MERGE` or `DISTINCT` and the requested max frequency
    /// is not `unfiltered`.
    pub(crate) requested_buffer_size: Option<RequestedBufferSize>,
    /// `LS_requested_max_frequency`: the maximum update frequency for the
    /// items. Omitted: `unlimited`. The server may impose a stricter limit.
    pub(crate) requested_max_frequency: Option<RequestedMaxFrequency>,
    /// `LS_snapshot`: the requested snapshot length. Omitted: `false`, and no
    /// snapshot is sent.
    pub(crate) snapshot: Option<Snapshot>,
    /// `LS_ack`: `false` suppresses the `REQOK` response, which is useful when
    /// issuing many subscriptions at once — success is still notified by
    /// `SUBOK` on the stream connection, and a failure still produces `REQERR`
    /// or `ERROR`. Omitted: the default is `true`. Ignored on HTTP, so this
    /// encoder does not emit it there.
    pub(crate) ack: Option<bool>,
}

impl Subscribe {
    /// The `LS_op` this request carries [`docs/spec/03-requests.md` §6.1].
    pub(crate) const OPERATION: ControlOperation = ControlOperation::Add;

    /// Rejects parameter combinations the specification does not admit, and
    /// those the server would accept but silently ignore.
    ///
    /// Only the first check below is a protocol rule; the other three go
    /// beyond the specification under the module-level policy on ignored
    /// parameters, and each says so [`docs/spec/03-requests.md` §6.1, §6.2].
    fn validate(&self) -> Result<(), ProtocolError> {
        // `LS_snapshot` as a length is "**Admitted only if `LS_mode` is
        // `DISTINCT`**" [§6.1, p.31] — an admissibility rule, not an "only
        // considered if", so this rejection is the spec's own and is the one
        // check here that carries no policy marker.
        if matches!(self.snapshot, Some(Snapshot::Length(_)))
            && self.mode != SubscriptionMode::Distinct
        {
            return Err(invalid(
                CONTROL_NAME,
                format!(
                    "LS_snapshot as a number of events is admitted only with LS_mode=DISTINCT, got {}",
                    self.mode.as_str()
                ),
            ));
        }
        // SPEC-AMBIGUITY: `docs/spec/03-requests.md` §6.1 (p.30) says of
        // `LS_snapshot=true` only that it is "Considered only if `LS_mode` is
        // `MERGE`, `DISTINCT` or `COMMAND`" — the server accepts the request
        // and drops the parameter. Refusing it is the module-level policy on
        // ignored parameters, i.e. stricter than the spec and reversible.
        if matches!(self.snapshot, Some(Snapshot::On)) && !self.mode.is_filtered() {
            return Err(invalid(
                CONTROL_NAME,
                "LS_snapshot=true is ignored by the server with LS_mode=RAW; omit it or choose another mode",
            ));
        }
        // SPEC-AMBIGUITY: `docs/spec/03-requests.md` §6.1 (p.30) says both
        // `unfiltered` and a decimal frequency are "Considered only if
        // `LS_mode` is `MERGE`, `DISTINCT` or `COMMAND`" — ignored with `RAW`,
        // not refused. Same policy, same reversibility. Note the Edition Note
        // in that section, which says a *server-imposed* global frequency
        // limit does still apply to `RAW`; that is the server's own limit, not
        // this parameter.
        if matches!(
            self.requested_max_frequency,
            Some(RequestedMaxFrequency::Unfiltered | RequestedMaxFrequency::Limited(_))
        ) && !self.mode.is_filtered()
        {
            return Err(invalid(
                CONTROL_NAME,
                "LS_requested_max_frequency is ignored by the server with LS_mode=RAW; omit it or choose another mode",
            ));
        }
        // SPEC-AMBIGUITY: `docs/spec/03-requests.md` §6.1 (p.30) states both
        // halves of this as one sentence — `LS_requested_buffer_size` is
        // "**Considered only if `LS_mode` is `MERGE` or `DISTINCT` and
        // `LS_requested_max_frequency` is not set to `unfiltered`**". Again
        // "considered only if", so the server would accept and drop it; both
        // rejections below are the module-level policy, not the protocol.
        if self.requested_buffer_size.is_some() {
            if !matches!(
                self.mode,
                SubscriptionMode::Merge | SubscriptionMode::Distinct
            ) {
                return Err(invalid(
                    CONTROL_NAME,
                    format!(
                        "LS_requested_buffer_size is ignored by the server unless LS_mode is MERGE or DISTINCT, got {}",
                        self.mode.as_str()
                    ),
                ));
            }
            if matches!(
                self.requested_max_frequency,
                Some(RequestedMaxFrequency::Unfiltered)
            ) {
                return Err(invalid(
                    CONTROL_NAME,
                    "LS_requested_buffer_size is ignored by the server when LS_requested_max_frequency is unfiltered; set one or the other",
                ));
            }
        }
        Ok(())
    }
}

impl TlcpRequest for Subscribe {
    const NAME: &'static str = CONTROL_NAME;
    const PATH: &'static str = CONTROL_PATH;

    fn encode_parameters(&self, transport: TransportKind) -> Result<String, ProtocolError> {
        self.validate()?;

        let mut params = ParameterLine::new();
        encode_control_header(
            &mut params,
            self.session.as_deref(),
            &self.request_id,
            Self::OPERATION,
            transport,
        )?;

        // Order below reproduces the spec's own p.31 example [§6.4]:
        // LS_subId, LS_group, LS_schema, LS_data_adapter, LS_mode.

        // `LS_subId` — required [§6.1].
        params.number("LS_subId", self.subscription_id);
        // `LS_group` — required [§6.1].
        params.text("LS_group", &self.group);
        // `LS_schema` — required [§6.1].
        params.text("LS_schema", &self.schema);
        // `LS_data_adapter` — optional; `DEFAULT` when omitted [§6.1].
        if let Some(adapter) = &self.data_adapter {
            params.text("LS_data_adapter", adapter);
        }
        // `LS_mode` — required [§6.1].
        params.verbatim("LS_mode", self.mode.as_str());
        // `LS_selector` — optional [§6.1].
        if let Some(selector) = &self.selector {
            params.text("LS_selector", selector);
        }
        // `LS_requested_buffer_size` — optional [§6.1].
        match &self.requested_buffer_size {
            Some(RequestedBufferSize::Unlimited) => {
                params.verbatim("LS_requested_buffer_size", "unlimited");
            }
            Some(RequestedBufferSize::Events(n)) => {
                params.number("LS_requested_buffer_size", n);
            }
            None => {}
        }
        // `LS_requested_max_frequency` — optional; `unlimited` when omitted
        // [§6.1].
        if let Some(frequency) = &self.requested_max_frequency {
            params.verbatim("LS_requested_max_frequency", frequency.as_str());
        }
        // `LS_snapshot` — optional; `false` when omitted [§6.1].
        match &self.snapshot {
            Some(Snapshot::Off) => params.boolean("LS_snapshot", false),
            Some(Snapshot::On) => params.boolean("LS_snapshot", true),
            Some(Snapshot::Length(n)) => params.number("LS_snapshot", n),
            None => {}
        }
        // `LS_ack` — WS only [§6.1].
        encode_ack(&mut params, self.ack, transport);

        Ok(params.finish())
    }
}

// ---------------------------------------------------------------------------
// Control: delete
// ---------------------------------------------------------------------------

/// `control` with `LS_op=delete` — remove an existing subscription
/// [`docs/spec/03-requests.md` §7].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unsubscribe {
    /// `LS_session`: see the control-request common parameters.
    pub(crate) session: Option<String>,
    /// `LS_reqId`: the ID the server echoes in `REQOK`/`REQERR`.
    pub(crate) request_id: RequestId,
    /// `LS_subId`: the same progressive integer used when subscribing.
    pub(crate) subscription_id: NonZeroU32,
    /// `LS_ack`: `false` suppresses the `REQOK` response; success is still
    /// notified by `UNSUB` on the stream connection, and a failure still
    /// produces `REQERR` or `ERROR`. Omitted: the default is `true`. Ignored
    /// on HTTP, so this encoder does not emit it there.
    pub(crate) ack: Option<bool>,
}

impl Unsubscribe {
    /// The `LS_op` this request carries [`docs/spec/03-requests.md` §7.1].
    pub(crate) const OPERATION: ControlOperation = ControlOperation::Delete;
}

impl TlcpRequest for Unsubscribe {
    const NAME: &'static str = CONTROL_NAME;
    const PATH: &'static str = CONTROL_PATH;

    fn encode_parameters(&self, transport: TransportKind) -> Result<String, ProtocolError> {
        let mut params = ParameterLine::new();
        encode_control_header(
            &mut params,
            self.session.as_deref(),
            &self.request_id,
            Self::OPERATION,
            transport,
        )?;
        // `LS_subId` — required [§7.1].
        params.number("LS_subId", self.subscription_id);
        // `LS_ack` — WS only [§7.1].
        encode_ack(&mut params, self.ack, transport);
        Ok(params.finish())
    }
}

// ---------------------------------------------------------------------------
// Control: reconf
// ---------------------------------------------------------------------------

/// `control` with `LS_op=reconf` — reconfigure an existing subscription
/// [`docs/spec/03-requests.md` §8].
///
/// As of TLCP-2.5.0 the maximum frequency is the only changeable parameter
/// (§8.1), which is why this struct has exactly one payload field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReconfigureSubscription {
    /// `LS_session`: see the control-request common parameters.
    pub(crate) session: Option<String>,
    /// `LS_reqId`: the ID the server echoes in `REQOK`/`REQERR`.
    pub(crate) request_id: RequestId,
    /// `LS_subId`: the same progressive integer used when subscribing.
    pub(crate) subscription_id: NonZeroU32,
    /// `LS_requested_max_frequency`: the new maximum update frequency.
    /// Omitted: no modification is requested. Admitted only if the
    /// subscription was not subscribed with an `unfiltered` max frequency, and
    /// considered only if its mode is `MERGE`, `DISTINCT` or `COMMAND` — both
    /// facts about session state that this stateless encoder cannot check.
    pub(crate) requested_max_frequency: Option<MaxFrequencyLimit>,
}

impl ReconfigureSubscription {
    /// The `LS_op` this request carries [`docs/spec/03-requests.md` §8.1].
    pub(crate) const OPERATION: ControlOperation = ControlOperation::Reconf;
}

impl TlcpRequest for ReconfigureSubscription {
    const NAME: &'static str = CONTROL_NAME;
    const PATH: &'static str = CONTROL_PATH;

    fn encode_parameters(&self, transport: TransportKind) -> Result<String, ProtocolError> {
        let mut params = ParameterLine::new();
        encode_control_header(
            &mut params,
            self.session.as_deref(),
            &self.request_id,
            Self::OPERATION,
            transport,
        )?;
        // `LS_subId` — required [§8.1].
        params.number("LS_subId", self.subscription_id);
        // `LS_requested_max_frequency` — optional [§8.1].
        if let Some(frequency) = &self.requested_max_frequency {
            params.verbatim("LS_requested_max_frequency", frequency.as_str());
        }
        // `LS_ack` is not emitted: the spec documents it only on `add`,
        // `delete` and `msg`, and leaves its status on the other control
        // operations unstated [§5.2].
        Ok(params.finish())
    }
}

// ---------------------------------------------------------------------------
// Control: constrain
// ---------------------------------------------------------------------------

/// `control` with `LS_op=constrain` — change the constraints of an existing
/// session [`docs/spec/03-requests.md` §9].
///
/// As of TLCP-2.5.0 the maximum bandwidth is the only changeable constraint
/// (§9.1), which is why this struct has exactly one payload field.
// No in-crate caller: the client surface does not expose a runtime bandwidth
// change. The encoder is kept because `control` with `LS_op=constrain` is part
// of the protocol this module models [`docs/spec/03-requests.md` §9].
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConstrainSession {
    /// `LS_session`: see the control-request common parameters.
    pub(crate) session: Option<String>,
    /// `LS_reqId`: the ID the server echoes in `REQOK`/`REQERR`.
    pub(crate) request_id: RequestId,
    /// `LS_requested_max_bandwidth`: the new maximum bandwidth in kbps, or
    /// `unlimited` to relieve any existing limit. Omitted: no modification is
    /// requested. Bandwidth Control is an optional server feature.
    pub(crate) requested_max_bandwidth: Option<MaxBandwidth>,
}

impl ConstrainSession {
    /// The `LS_op` this request carries [`docs/spec/03-requests.md` §9.1].
    #[allow(dead_code)]
    pub(crate) const OPERATION: ControlOperation = ControlOperation::Constrain;
}

impl TlcpRequest for ConstrainSession {
    const NAME: &'static str = CONTROL_NAME;
    const PATH: &'static str = CONTROL_PATH;

    fn encode_parameters(&self, transport: TransportKind) -> Result<String, ProtocolError> {
        let mut params = ParameterLine::new();
        encode_control_header(
            &mut params,
            self.session.as_deref(),
            &self.request_id,
            Self::OPERATION,
            transport,
        )?;
        // `LS_requested_max_bandwidth` — optional [§9.1]. Note there is no
        // `LS_subId`: this operation acts on the session, not a subscription.
        if let Some(bandwidth) = &self.requested_max_bandwidth {
            params.verbatim("LS_requested_max_bandwidth", bandwidth.as_str());
        }
        Ok(params.finish())
    }
}

// ---------------------------------------------------------------------------
// Control: force_rebind
// ---------------------------------------------------------------------------

/// `control` with `LS_op=force_rebind` — force an existing session to rebind
/// [`docs/spec/03-requests.md` §10].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForceRebind {
    /// `LS_session`: see the control-request common parameters.
    pub(crate) session: Option<String>,
    /// `LS_reqId`: the ID the server echoes in `REQOK`/`REQERR`.
    pub(crate) request_id: RequestId,
    /// `LS_polling_millis`: the expected time in milliseconds between the
    /// closing of the connection and the next binding request. Omitted: the
    /// server is given no expectation.
    pub(crate) polling_millis: Option<u64>,
    /// `LS_close_socket`: `true` forces the server to close the stream
    /// connection. Omitted: the connection may be left open and reused, on
    /// both HTTP and WS.
    //
    // SPEC-AMBIGUITY: `docs/spec/03-requests.md` §11.2 records that this
    // parameter's default is worded as "the stream connection **may** be left
    // open" on `force_rebind` but "**is** left open" on `destroy`, without
    // saying whether the difference is meaningful. Neither request infers a
    // value here; `None` means the parameter is not sent and the caller must
    // not assume either behaviour.
    pub(crate) close_socket: Option<bool>,
}

impl ForceRebind {
    /// The `LS_op` this request carries [`docs/spec/03-requests.md` §10.1].
    pub(crate) const OPERATION: ControlOperation = ControlOperation::ForceRebind;
}

impl TlcpRequest for ForceRebind {
    const NAME: &'static str = CONTROL_NAME;
    const PATH: &'static str = CONTROL_PATH;

    fn encode_parameters(&self, transport: TransportKind) -> Result<String, ProtocolError> {
        let mut params = ParameterLine::new();
        encode_control_header(
            &mut params,
            self.session.as_deref(),
            &self.request_id,
            Self::OPERATION,
            transport,
        )?;
        // `LS_polling_millis` — optional, in milliseconds [§10.1].
        if let Some(ms) = self.polling_millis {
            params.number("LS_polling_millis", ms);
        }
        // `LS_close_socket` — optional [§10.1].
        if let Some(close) = self.close_socket {
            params.boolean("LS_close_socket", close);
        }
        Ok(params.finish())
    }
}

// ---------------------------------------------------------------------------
// Control: destroy
// ---------------------------------------------------------------------------

/// `control` with `LS_op=destroy` — terminate an existing session
/// [`docs/spec/03-requests.md` §11].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DestroySession {
    /// `LS_session`: see the control-request common parameters.
    pub(crate) session: Option<String>,
    /// `LS_reqId`: the ID the server echoes in `REQOK`/`REQERR`.
    pub(crate) request_id: RequestId,
    /// `LS_cause_code` and `LS_cause_message`, which the server reports in the
    /// consequent `END` notification. Omitted: the `END` reports the default
    /// code `31` and a server-supplied message.
    pub(crate) cause: Option<DestroyCause>,
    /// `LS_close_socket`: `true` forces the server to close the stream
    /// connection. Omitted: the connection is left open and may be reused, on
    /// both HTTP and WS.
    pub(crate) close_socket: Option<bool>,
}

impl DestroySession {
    /// The `LS_op` this request carries [`docs/spec/03-requests.md` §11.1].
    pub(crate) const OPERATION: ControlOperation = ControlOperation::Destroy;
}

impl TlcpRequest for DestroySession {
    const NAME: &'static str = CONTROL_NAME;
    const PATH: &'static str = CONTROL_PATH;

    fn encode_parameters(&self, transport: TransportKind) -> Result<String, ProtocolError> {
        let mut params = ParameterLine::new();
        encode_control_header(
            &mut params,
            self.session.as_deref(),
            &self.request_id,
            Self::OPERATION,
            transport,
        )?;
        // `LS_cause_code` and `LS_cause_message` — optional, and the message
        // is considered only alongside the code [§11.1, §11.2]. `DestroyCause`
        // makes a message without a code unrepresentable, so no runtime check
        // is needed here.
        if let Some(cause) = &self.cause {
            params.number("LS_cause_code", cause.code.get());
            if let Some(message) = &cause.message {
                params.text("LS_cause_message", message.as_str());
            }
        }
        // `LS_close_socket` — optional [§11.1].
        if let Some(close) = self.close_socket {
            params.boolean("LS_close_socket", close);
        }
        Ok(params.finish())
    }
}

// ---------------------------------------------------------------------------
// Message send
// ---------------------------------------------------------------------------

/// `msg` — send a message to the Metadata Adapter
/// [`docs/spec/03-requests.md` §12].
///
/// A message send is a control request with a different request name, which is
/// why it carries no `LS_op` (§12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MessageSend {
    /// `LS_session`: required with HTTP; on WS it defaults to the last bound
    /// session.
    pub(crate) session: Option<String>,
    /// `LS_reqId`: the ID the server echoes in `REQOK`/`REQERR`.
    pub(crate) request_id: RequestId,
    /// `LS_message`: the payload, any text string. Its meaning is entirely up
    /// to the Metadata Adapter. Required.
    pub(crate) message: String,
    /// `LS_sequence`: the sequence this message belongs to. Messages sharing a
    /// sequence are processed in `LS_msg_prog` order. Omitted: the message is
    /// processed immediately, possibly concurrently with others.
    pub(crate) sequence: Option<SequenceName>,
    /// `LS_msg_prog`: the progressive number of this message, starting at 1.
    /// Also identifies the message in `MSGDONE`/`MSGFAIL`, even with no
    /// sequence. Mandatory whenever a sequence is given or the outcome is
    /// requested; see the encode-time check below.
    pub(crate) msg_prog: Option<NonZeroU32>,
    /// `LS_max_wait`: the longest the server may wait, in milliseconds, before
    /// processing this message when preceding messages of the same sequence
    /// have not arrived. Omitted: assigned by the server from its own
    /// configuration.
    ///
    /// The server ignores it unless [`MessageSend::sequence`] is also set, so
    /// setting it alone is refused at encode time rather than dropped — see
    /// the module-level policy on ignored parameters.
    pub(crate) max_wait_millis: Option<u64>,
    /// `LS_ack`: `false` suppresses the `REQOK` response, useful for frequent
    /// loads of messages with no processing requirements. A failure still
    /// produces `REQERR` or `ERROR`. Omitted: the default is `true`. Ignored
    /// on HTTP, so this encoder does not emit it there.
    pub(crate) ack: Option<bool>,
    /// `LS_outcome`: `false` suppresses the `MSGDONE`/`MSGFAIL` notification.
    /// Omitted: the default is `true`. With both `LS_ack` and `LS_outcome`
    /// `false` a successful submission produces no notification at all — a
    /// deliberate fire-and-forget policy.
    pub(crate) outcome: Option<bool>,
}

impl MessageSend {
    /// Rejects a message that omits a mandatory `LS_msg_prog`.
    ///
    /// `LS_msg_prog` is "mandatory whenever `LS_sequence` is specified or
    /// `LS_outcome` is set to `true`" [`docs/spec/03-requests.md` §12.1].
    //
    // SPEC-AMBIGUITY: `docs/spec/03-requests.md` §12.2 records that
    // `LS_msg_prog` is labelled Optional while being mandatory whenever
    // `LS_outcome` is `true` — whose own default is `true`. Read literally
    // that makes it mandatory in the default case. It is not stated whether
    // the rule applies only to an explicit `LS_outcome=true`. The literal
    // reading is taken here: `LS_msg_prog` is required unless `LS_outcome` is
    // explicitly `false`. That is the defensive choice, because the spec's own
    // `msg` example supplies `LS_msg_prog` (§12.4) and the alternative risks
    // a server-side rejection at run time instead of a caller-side error at
    // encode time.
    fn validate(&self) -> Result<(), ProtocolError> {
        let outcome_requested = self.outcome != Some(false);
        if self.msg_prog.is_none() && (self.sequence.is_some() || outcome_requested) {
            let cause = if self.sequence.is_some() {
                "LS_sequence is specified"
            } else {
                "LS_outcome is not explicitly false"
            };
            return Err(invalid(
                Self::NAME,
                format!("LS_msg_prog is mandatory when {cause}"),
            ));
        }

        // SPEC-AMBIGUITY: `LS_max_wait` is "**Ignored if `LS_sequence` is
        // omitted**" [§12.1, p.43] — the same "the server accepts it and drops
        // it" shape as the `Subscribe` checks, so it is refused under the same
        // module-level policy and is equally reversible. This check exists for
        // consistency: emitting `LS_max_wait` here while refusing an ignored
        // `LS_requested_buffer_size` on `add` would apply two different rules
        // to one spec construction.
        if self.max_wait_millis.is_some() && self.sequence.is_none() {
            return Err(invalid(
                Self::NAME,
                "LS_max_wait is ignored by the server unless LS_sequence is specified",
            ));
        }
        Ok(())
    }
}

impl TlcpRequest for MessageSend {
    const NAME: &'static str = "msg";
    const PATH: &'static str = "/lightstreamer/msg.txt";

    fn encode_parameters(&self, transport: TransportKind) -> Result<String, ProtocolError> {
        self.validate()?;

        let mut params = ParameterLine::new();

        // `LS_session` — required with HTTP, defaulted on WS [§5.1, §12.1].
        match &self.session {
            Some(session) => params.text("LS_session", session),
            None if transport.requires_session() => {
                return Err(invalid(
                    Self::NAME,
                    "LS_session is required with HTTP transport",
                ));
            }
            None => {}
        }
        // `LS_reqId` — required [§5.1]. There is no `LS_op`: `msg` is a
        // control request with its own request name [§12].
        params.verbatim("LS_reqId", self.request_id.as_str());

        // Order below reproduces the spec's own p.43 example [§12.4]:
        // LS_sequence, LS_msg_prog, then LS_message last.

        // `LS_sequence` — optional [§12.1].
        if let Some(sequence) = &self.sequence {
            params.verbatim("LS_sequence", sequence.as_str());
        }
        // `LS_msg_prog` — optional in name, mandatory in the cases checked by
        // `validate` [§12.1].
        if let Some(prog) = self.msg_prog {
            params.number("LS_msg_prog", prog);
        }
        // `LS_max_wait` — optional, in milliseconds [§12.1]. `validate` has
        // already established that `LS_sequence` is present, without which the
        // server would ignore this.
        if let Some(ms) = self.max_wait_millis {
            params.number("LS_max_wait", ms);
        }
        // `LS_ack` — WS only [§12.1].
        encode_ack(&mut params, self.ack, transport);
        // `LS_outcome` — optional; `true` when omitted [§12.1].
        if let Some(outcome) = self.outcome {
            params.boolean("LS_outcome", outcome);
        }
        // `LS_message` — required, any text string [§12.1].
        params.text("LS_message", &self.message);

        Ok(params.finish())
    }
}

// ---------------------------------------------------------------------------
// Heartbeat
// ---------------------------------------------------------------------------

/// `heartbeat` — the pseudo-request that keeps the channel engaged
/// [`docs/spec/03-requests.md` §14].
///
/// It exists to keep an idle socket from being closed by an intermediary, to
/// detect a WebSocket that was interrupted without being closed, and — in
/// combination with `LS_inactivity_millis` on the session request — to let the
/// server tell a stuck client from a quiet one. Any control request has the
/// same effect, so no heartbeat is needed while control requests are flowing.
///
/// It takes no `LS_reqId` and no `LS_op` (§14.1), and is not meant to receive
/// a response except on HTTP, or on WS when the request itself is malformed
/// (§14.3).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Heartbeat {
    /// `LS_session`: the session the client declares it is still listening to.
    /// Needed only for the stuck-client detection purpose. Omitted with WS: it
    /// refers to the last session bound to that WS connection, which may be
    /// currently bound or temporarily unbound during a polling cycle.
    //
    // SPEC-AMBIGUITY: `docs/spec/03-requests.md` §14.1 states the requirement
    // as "Optional with WS transport" and never repeats the "always required
    // with HTTP" wording it uses for `bind_session` (§3.1) — yet the same
    // phrasing pattern, and the HTTP example that does supply it (§14.2),
    // both point at HTTP requiring it. It is required on HTTP here, matching
    // every other request in the chapter.
    pub(crate) session: Option<String>,
}

impl TlcpRequest for Heartbeat {
    const NAME: &'static str = "heartbeat";
    const PATH: &'static str = "/lightstreamer/heartbeat.txt";

    fn encode_parameters(&self, transport: TransportKind) -> Result<String, ProtocolError> {
        let mut params = ParameterLine::new();
        // `LS_session` — the only parameter this request has [§14.1].
        match &self.session {
            Some(session) => params.text("LS_session", session),
            None if transport.requires_session() => {
                return Err(invalid(
                    Self::NAME,
                    "LS_session is required with HTTP transport",
                ));
            }
            None => {}
        }
        Ok(params.finish())
    }
}

// ---------------------------------------------------------------------------
// WebSocket establishment check
// ---------------------------------------------------------------------------

/// `wsok` — the pseudo-request that tests a newly established WebSocket
/// [`docs/spec/03-requests.md` §15].
///
/// The server simply echoes it as `WSOK`. It is allowed **only** on a
/// WebSocket, is expected to be the first request issued on it, and its
/// response is guaranteed to precede the response of any later request. There
/// are no error responses.
///
/// It does not implement [`TlcpRequest`]: it has no HTTP form, no parameters,
/// and — uniquely — no parameter line at all, whereas a parameterless
/// [`Heartbeat`] still requires an empty one (§15.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct WsOk;

impl WsOk {
    /// The request name, which is also the whole message
    /// [`docs/spec/03-requests.md` §15].
    pub(crate) const NAME: &'static str = "wsok";

    /// The complete WS text message.
    ///
    /// "Not only does the request require no parameters, but also the
    /// parameter line, needed with other types of requests (and possibly
    /// empty), is missing in this case" [`docs/spec/03-requests.md` §15.1] —
    /// hence no trailing `CR-LF`.
    #[must_use]
    #[inline]
    pub(crate) const fn ws_message() -> &'static str {
        Self::NAME
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The session ID the spec's control examples use [p.31 onward].
    const CONTROL_SESSION: &str = "Sd9fce58fb5dbbebfT2255126";
    /// The session ID the spec's binding examples use [p.26].
    const BIND_SESSION: &str = "S73d162c183916f0dT2729905";

    fn sub_id(n: u32) -> NonZeroU32 {
        match NonZeroU32::new(n) {
            Some(id) => id,
            None => panic!("a subscription ID is 1-based and never zero"),
        }
    }

    fn req_id(n: u64) -> RequestId {
        RequestId::from_progressive(n)
    }

    /// Extracts one parameter's wire value from an encoded parameter line.
    ///
    /// Splitting on `&` is sound precisely because every reserved character —
    /// `&` and `=` among them — is percent-encoded inside a value
    /// [`docs/spec/01-foundations.md` §6.1.3]. A value that still contains a
    /// raw `&` therefore shows up here as a truncated value, which is what the
    /// escaping tests below assert against.
    fn param<'a>(line: &'a str, name: &str) -> Option<&'a str> {
        line.split('&')
            .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
    }

    /// As [`param`], for a parameter the test asserts must be present.
    fn required_param<'a>(line: &'a str, name: &str) -> &'a str {
        match param(line, name) {
            Some(value) => value,
            None => panic!("`{name}` is missing from {line:?}"),
        }
    }

    /// Whether a wire value carries the given percent-escape, ignoring the
    /// hex-digit casing, which the spec does not fix.
    fn has_escape(value: &str, escape: &str) -> bool {
        value.to_ascii_uppercase().contains(escape)
    }

    /// The reserved characters that must never survive un-escaped into a wire
    /// value [`docs/spec/01-foundations.md` §6.1.3]. `%` is deliberately
    /// absent: it is reserved as an input character but is also the escape
    /// introducer, so it legitimately appears in an encoded value.
    const RESERVED: [char; 5] = ['\r', '\n', '&', '=', '+'];

    // -- create_session -----------------------------------------------------

    /// Body of the spec's own `create_session` HTTP example
    /// [`docs/spec/03-requests.md` §2.3, p.24].
    const CREATE_SESSION_EXAMPLE: &str = "LS_user=user&LS_password=password&LS_adapter_set=DEMO&LS_cid=mgQkwtwdysogQz2BJ4Ji%20kOj2Bg";

    fn spec_create_session() -> CreateSession {
        CreateSession {
            user: Some("user".to_owned()),
            password: Some("password".to_owned()),
            adapter_set: Some("DEMO".to_owned()),
            ..CreateSession::default()
        }
    }

    /// The tail this encoder appends to the spec example: `LS_supported_diffs`
    /// is always advertised, derived from the decoder registry
    /// (`docs/adr/0004-supported-diffs-derived-from-decoders.md`), so it is
    /// not present in the spec's example and must be accounted for
    /// separately.
    fn supported_diffs_tail() -> String {
        let diffs = crate::protocol::diff::supported_diffs();
        if diffs.is_empty() {
            String::new()
        } else {
            format!("&LS_supported_diffs={diffs}")
        }
    }

    #[test]
    fn test_create_session_http_body_matches_spec_example_p24() -> Result<(), ProtocolError> {
        let body = spec_create_session().http_body()?;
        let expected = format!("{CREATE_SESSION_EXAMPLE}{}", supported_diffs_tail());
        assert_eq!(body, expected);
        assert!(body.starts_with(CREATE_SESSION_EXAMPLE));

        Ok(())
    }

    #[test]
    fn test_create_session_ws_message_matches_spec_example_p24() -> Result<(), ProtocolError> {
        let message = spec_create_session().ws_message()?;
        let expected = format!(
            "create_session\r\n{CREATE_SESSION_EXAMPLE}{}",
            supported_diffs_tail()
        );
        assert_eq!(message, expected);

        Ok(())
    }

    #[test]
    fn test_create_session_http_target_carries_the_protocol_parameter() {
        // `LS_protocol` is required on the query string [§1.1, p.11].
        assert_eq!(
            CreateSession::http_target(),
            "/lightstreamer/create_session.txt?LS_protocol=TLCP-2.5.0"
        );
    }

    #[test]
    fn test_create_session_omits_every_optional_parameter_when_none() -> Result<(), ProtocolError> {
        // Only `LS_cid` is required, plus the derived `LS_supported_diffs`.
        let body = CreateSession::default().http_body()?;
        let expected = format!("LS_cid={CLIENT_IDENTIFIER}{}", supported_diffs_tail());
        assert_eq!(body, expected);
        assert!(!body.contains("LS_user"));
        assert!(!body.contains("LS_polling"));
        assert!(!body.contains("LS_ttl_millis"));

        Ok(())
    }

    #[test]
    fn test_create_session_supported_diffs_is_derived_not_literal() -> Result<(), ProtocolError> {
        // ADR-0004: the advertised set is computed from the decoder registry.
        // When the registry is empty the parameter is omitted, never sent
        // empty.
        let body = CreateSession::default().http_body()?;
        let diffs = crate::protocol::diff::supported_diffs();
        if diffs.is_empty() {
            assert!(!body.contains("LS_supported_diffs"));
        } else {
            assert!(body.contains(&format!("LS_supported_diffs={diffs}")));
            assert!(!body.contains("LS_supported_diffs=&"));
            assert!(!body.ends_with("LS_supported_diffs="));
            // A comma-separated list of non-empty tags with no spaces
            // [§2.1; `docs/spec/04-notifications.md` §2.3].
            assert!(diffs.split(',').all(|tag| !tag.is_empty()));
            assert!(!diffs.contains(' '));
        }

        Ok(())
    }

    #[test]
    fn test_create_session_streaming_group_encoded() -> Result<(), ProtocolError> {
        let request = CreateSession {
            connection: ConnectionMode::Streaming {
                inactivity_millis: Some(5000),
                keepalive_millis: Some(3000),
                send_sync: Some(false),
            },
            ..CreateSession::default()
        };
        let body = request.http_body()?;
        assert!(body.contains("LS_inactivity_millis=5000"));
        assert!(body.contains("LS_keepalive_millis=3000"));
        assert!(body.contains("LS_send_sync=false"));
        // Absence of `LS_polling` *is* the streaming request [§2.1].
        assert!(!body.contains("LS_polling"));

        Ok(())
    }

    #[test]
    fn test_create_session_polling_group_encoded_and_excludes_streaming_group()
    -> Result<(), ProtocolError> {
        let request = CreateSession {
            connection: ConnectionMode::Polling {
                polling_millis: 2000,
                idle_millis: Some(15000),
            },
            ..CreateSession::default()
        };
        let body = request.http_body()?;
        assert!(body.contains("LS_polling=true"));
        assert!(body.contains("LS_polling_millis=2000"));
        assert!(body.contains("LS_idle_millis=15000"));
        // The streaming group is unrepresentable alongside it [§2.2].
        assert!(!body.contains("LS_inactivity_millis"));
        assert!(!body.contains("LS_keepalive_millis"));
        assert!(!body.contains("LS_send_sync"));

        Ok(())
    }

    #[test]
    fn test_create_session_polling_millis_is_not_optional() -> Result<(), ProtocolError> {
        // `LS_polling_millis` is "required by the Server in the polling case"
        // [§2.2], so the type has no way to omit it.
        let request = CreateSession {
            connection: ConnectionMode::Polling {
                polling_millis: 0,
                idle_millis: None,
            },
            ..CreateSession::default()
        };
        assert!(request.http_body()?.contains("LS_polling_millis=0"));

        Ok(())
    }

    #[test]
    fn test_create_session_ttl_millis_special_values() -> Result<(), ProtocolError> {
        for (ttl, expected) in [
            (TimeToLive::Unknown, "LS_ttl_millis=unknown"),
            (TimeToLive::Unlimited, "LS_ttl_millis=unlimited"),
            (TimeToLive::Millis(750), "LS_ttl_millis=750"),
        ] {
            let request = CreateSession {
                ttl: Some(ttl),
                ..CreateSession::default()
            };
            assert!(request.http_body()?.contains(expected));
        }

        Ok(())
    }

    #[test]
    fn test_create_session_bandwidth_content_length_and_reduce_head() -> Result<(), ProtocolError> {
        let request = CreateSession {
            requested_max_bandwidth: Some(DecimalNumber::try_new("50.0")?),
            content_length: Some(4000),
            reduce_head: Some(true),
            ..CreateSession::default()
        };
        let body = request.http_body()?;
        assert!(body.contains("LS_requested_max_bandwidth=50.0"));
        assert!(body.contains("LS_content_length=4000"));
        assert!(body.contains("LS_reduce_head=true"));

        Ok(())
    }

    #[test]
    fn test_create_session_escapes_reserved_characters_in_credentials() -> Result<(), ProtocolError>
    {
        // The reserved set is CR, LF, `&`, `=`, `%`, `+`
        // [`docs/spec/01-foundations.md` §6.1.3]. Escaping is delegated to
        // `encode_form_value`; this asserts the raw value never reaches the
        // wire.
        let request = CreateSession {
            user: Some("a&b=c".to_owned()),
            password: Some("p+w%d".to_owned()),
            ..CreateSession::default()
        };
        let body = request.http_body()?;

        let user = required_param(&body, "LS_user");
        assert_ne!(user, "a&b=c");
        assert!(!user.contains(RESERVED));
        assert!(has_escape(user, "%26"));
        assert!(has_escape(user, "%3D"));

        let password = required_param(&body, "LS_password");
        assert!(!password.contains(RESERVED));
        assert!(has_escape(password, "%2B"));
        assert!(has_escape(password, "%25"));

        Ok(())
    }

    // -- bind_session -------------------------------------------------------

    #[test]
    fn test_bind_session_http_body_matches_spec_example_p26() -> Result<(), ProtocolError> {
        let request = BindSession {
            session: Some(BIND_SESSION.to_owned()),
            ..BindSession::default()
        };
        assert_eq!(request.http_body()?, "LS_session=S73d162c183916f0dT2729905");

        Ok(())
    }

    #[test]
    fn test_bind_session_ws_message_matches_spec_example_p26() -> Result<(), ProtocolError> {
        let request = BindSession {
            session: Some(BIND_SESSION.to_owned()),
            ..BindSession::default()
        };
        assert_eq!(
            request.ws_message()?,
            "bind_session\r\nLS_session=S73d162c183916f0dT2729905"
        );

        Ok(())
    }

    #[test]
    fn test_bind_session_ws_may_omit_session() -> Result<(), ProtocolError> {
        // With WS the rebind applies to the last session bound to that
        // connection [§3.1].
        let message = BindSession::default().ws_message()?;
        assert_eq!(message, "bind_session\r\n");

        Ok(())
    }

    #[test]
    fn test_bind_session_http_without_session_is_error() {
        // "It is always required with HTTP transport" [§3.1].
        assert!(matches!(
            BindSession::default().http_body(),
            Err(ProtocolError::Request {
                request: "bind_session",
                ..
            })
        ));
    }

    #[test]
    fn test_bind_session_recovery_and_content_length() -> Result<(), ProtocolError> {
        let request = BindSession {
            session: Some(BIND_SESSION.to_owned()),
            recovery_from: Some(1234),
            content_length: Some(50_000),
            reduce_head: Some(true),
            ..BindSession::default()
        };
        assert_eq!(
            request.http_body()?,
            "LS_session=S73d162c183916f0dT2729905&LS_recovery_from=1234\
             &LS_content_length=50000&LS_reduce_head=true"
        );

        Ok(())
    }

    #[test]
    fn test_bind_session_polling_group() -> Result<(), ProtocolError> {
        let request = BindSession {
            session: Some(BIND_SESSION.to_owned()),
            connection: ConnectionMode::Polling {
                polling_millis: 5000,
                idle_millis: None,
            },
            ..BindSession::default()
        };
        let body = request.http_body()?;
        assert!(body.contains("LS_polling=true&LS_polling_millis=5000"));
        assert!(!body.contains("LS_idle_millis"));

        Ok(())
    }

    // -- control: add -------------------------------------------------------

    fn spec_subscribe(session: Option<&str>) -> Subscribe {
        Subscribe {
            session: session.map(str::to_owned),
            request_id: req_id(1),
            subscription_id: sub_id(1),
            group: "item1".to_owned(),
            schema: "last_price".to_owned(),
            mode: SubscriptionMode::Merge,
            data_adapter: Some("QUOTE_ADAPTER".to_owned()),
            selector: None,
            requested_buffer_size: None,
            requested_max_frequency: None,
            snapshot: None,
            ack: None,
        }
    }

    #[test]
    fn test_subscribe_http_body_matches_spec_example_p31() -> Result<(), ProtocolError> {
        let request = spec_subscribe(Some(CONTROL_SESSION));
        assert_eq!(
            request.http_body()?,
            "LS_session=Sd9fce58fb5dbbebfT2255126&LS_reqId=1&LS_op=add&LS_subId=1\
             &LS_group=item1&LS_schema=last_price&LS_data_adapter=QUOTE_ADAPTER&LS_mode=MERGE"
        );

        Ok(())
    }

    #[test]
    fn test_subscribe_ws_message_matches_spec_example_p31() -> Result<(), ProtocolError> {
        let request = spec_subscribe(None);
        assert_eq!(
            request.ws_message()?,
            "control\r\nLS_reqId=1&LS_op=add&LS_subId=1&LS_group=item1\
             &LS_schema=last_price&LS_data_adapter=QUOTE_ADAPTER&LS_mode=MERGE"
        );

        Ok(())
    }

    #[test]
    fn test_subscribe_ws_batch_matches_spec_example_p31() -> Result<(), ProtocolError> {
        // Multiple request example with WS transport [§6.4, pp.31-32].
        let first = spec_subscribe(None);
        let second = Subscribe {
            request_id: req_id(2),
            subscription_id: sub_id(2),
            group: "item2".to_owned(),
            ..spec_subscribe(None)
        };
        let lines = vec![
            first.encode_parameters(TransportKind::WebSocket)?,
            second.encode_parameters(TransportKind::WebSocket)?,
        ];
        assert_eq!(
            encode_ws_batch(Subscribe::NAME, &lines)?,
            "control\r\n\
             LS_reqId=1&LS_op=add&LS_subId=1&LS_group=item1&LS_schema=last_price\
             &LS_data_adapter=QUOTE_ADAPTER&LS_mode=MERGE\r\n\
             LS_reqId=2&LS_op=add&LS_subId=2&LS_group=item2&LS_schema=last_price\
             &LS_data_adapter=QUOTE_ADAPTER&LS_mode=MERGE"
        );

        Ok(())
    }

    #[test]
    fn test_subscribe_http_batch_uses_query_string_session() -> Result<(), ProtocolError> {
        // Multiple request example with HTTP transport [§6.4, p.31]. The
        // printed body of that example omits the `&` between `LS_reqId` and
        // `LS_op` on both lines, which §6.4 flags as a typographical error
        // contradicting the p.11 syntax rule; the corrected form is asserted.
        let first = Subscribe {
            session: None,
            ..spec_subscribe(None)
        };
        let second = Subscribe {
            request_id: req_id(2),
            subscription_id: sub_id(2),
            group: "item2".to_owned(),
            ..spec_subscribe(None)
        };
        let lines = vec![
            first.encode_parameters(TransportKind::HttpBatched)?,
            second.encode_parameters(TransportKind::HttpBatched)?,
        ];
        assert_eq!(
            Subscribe::http_target_with_session(CONTROL_SESSION),
            "/lightstreamer/control.txt?LS_protocol=TLCP-2.5.0\
             &LS_session=Sd9fce58fb5dbbebfT2255126"
        );
        assert_eq!(
            encode_http_batch(Subscribe::NAME, &lines)?,
            "LS_reqId=1&LS_op=add&LS_subId=1&LS_group=item1&LS_schema=last_price\
             &LS_data_adapter=QUOTE_ADAPTER&LS_mode=MERGE\r\n\
             LS_reqId=2&LS_op=add&LS_subId=2&LS_group=item2&LS_schema=last_price\
             &LS_data_adapter=QUOTE_ADAPTER&LS_mode=MERGE"
        );

        Ok(())
    }

    #[test]
    fn test_encode_batch_rejects_an_empty_batch() {
        assert!(encode_ws_batch(Subscribe::NAME, &[]).is_err());
        assert!(encode_http_batch(Subscribe::NAME, &[]).is_err());
    }

    #[test]
    fn test_subscribe_http_without_session_is_error() {
        let request = spec_subscribe(None);
        assert!(matches!(
            request.http_body(),
            Err(ProtocolError::Request {
                request: "control",
                ..
            })
        ));
    }

    #[test]
    fn test_subscribe_all_optional_parameters_encoded() -> Result<(), ProtocolError> {
        let request = Subscribe {
            selector: Some("sel1".to_owned()),
            requested_buffer_size: Some(RequestedBufferSize::Events(sub_id(30))),
            requested_max_frequency: Some(RequestedMaxFrequency::Limited(DecimalNumber::try_new(
                "2.5",
            )?)),
            snapshot: Some(Snapshot::On),
            ack: Some(false),
            ..spec_subscribe(None)
        };
        assert_eq!(
            request.encode_parameters(TransportKind::WebSocket)?,
            "LS_reqId=1&LS_op=add&LS_subId=1&LS_group=item1&LS_schema=last_price\
             &LS_data_adapter=QUOTE_ADAPTER&LS_mode=MERGE&LS_selector=sel1\
             &LS_requested_buffer_size=30&LS_requested_max_frequency=2.5\
             &LS_snapshot=true&LS_ack=false"
        );

        Ok(())
    }

    #[test]
    fn test_subscribe_unlimited_buffer_size_and_unfiltered_frequency() -> Result<(), ProtocolError>
    {
        let request = Subscribe {
            requested_buffer_size: Some(RequestedBufferSize::Unlimited),
            ..spec_subscribe(None)
        };
        assert!(
            request
                .encode_parameters(TransportKind::WebSocket)?
                .contains("LS_requested_buffer_size=unlimited")
        );

        let request = Subscribe {
            requested_max_frequency: Some(RequestedMaxFrequency::Unfiltered),
            ..spec_subscribe(None)
        };
        assert!(
            request
                .encode_parameters(TransportKind::WebSocket)?
                .contains("LS_requested_max_frequency=unfiltered")
        );

        Ok(())
    }

    #[test]
    fn test_subscribe_ack_is_omitted_on_http_and_emitted_on_ws() -> Result<(), ProtocolError> {
        // "Ignored if used with HTTP transport" [§6.1].
        let request = Subscribe {
            ack: Some(false),
            ..spec_subscribe(Some(CONTROL_SESSION))
        };
        assert!(!request.http_body()?.contains("LS_ack"));
        assert!(
            request
                .encode_parameters(TransportKind::WebSocket)?
                .contains("LS_ack=false")
        );

        Ok(())
    }

    #[test]
    fn test_subscribe_snapshot_length_requires_distinct_mode() -> Result<(), ProtocolError> {
        // "Admitted only if LS_mode is DISTINCT" [§6.1].
        let request = Subscribe {
            mode: SubscriptionMode::Merge,
            snapshot: Some(Snapshot::Length(sub_id(10))),
            ..spec_subscribe(None)
        };
        assert!(request.encode_parameters(TransportKind::WebSocket).is_err());

        let request = Subscribe {
            mode: SubscriptionMode::Distinct,
            snapshot: Some(Snapshot::Length(sub_id(10))),
            ..spec_subscribe(None)
        };
        assert!(
            request
                .encode_parameters(TransportKind::WebSocket)?
                .contains("LS_snapshot=10")
        );

        Ok(())
    }

    #[test]
    fn test_subscribe_snapshot_off_is_encoded_explicitly() -> Result<(), ProtocolError> {
        // `false` is the default, but an explicit `Snapshot::Off` is a
        // deliberate statement and is emitted rather than dropped [§6.1].
        let request = Subscribe {
            snapshot: Some(Snapshot::Off),
            ..spec_subscribe(None)
        };
        assert!(
            request
                .encode_parameters(TransportKind::WebSocket)?
                .contains("LS_snapshot=false")
        );

        Ok(())
    }

    #[test]
    fn test_subscribe_snapshot_on_with_raw_mode_is_error() {
        let request = Subscribe {
            mode: SubscriptionMode::Raw,
            snapshot: Some(Snapshot::On),
            ..spec_subscribe(None)
        };
        assert!(request.encode_parameters(TransportKind::WebSocket).is_err());
    }

    #[test]
    fn test_subscribe_max_frequency_with_raw_mode_is_error() {
        let request = Subscribe {
            mode: SubscriptionMode::Raw,
            requested_max_frequency: Some(RequestedMaxFrequency::Unfiltered),
            ..spec_subscribe(None)
        };
        assert!(request.encode_parameters(TransportKind::WebSocket).is_err());
    }

    #[test]
    fn test_subscribe_buffer_size_with_command_mode_is_error() {
        // "Considered only if LS_mode is MERGE or DISTINCT" [§6.1].
        let request = Subscribe {
            mode: SubscriptionMode::Command,
            requested_buffer_size: Some(RequestedBufferSize::Unlimited),
            ..spec_subscribe(None)
        };
        assert!(request.encode_parameters(TransportKind::WebSocket).is_err());
    }

    #[test]
    fn test_subscribe_buffer_size_with_unfiltered_frequency_is_error() {
        // "…and LS_requested_max_frequency is not set to unfiltered" [§6.1].
        let request = Subscribe {
            requested_buffer_size: Some(RequestedBufferSize::Unlimited),
            requested_max_frequency: Some(RequestedMaxFrequency::Unfiltered),
            ..spec_subscribe(None)
        };
        assert!(request.encode_parameters(TransportKind::WebSocket).is_err());
    }

    #[test]
    fn test_subscribe_escapes_group_and_schema() -> Result<(), ProtocolError> {
        let request = Subscribe {
            group: "item1&item2".to_owned(),
            schema: "f1=f2".to_owned(),
            ..spec_subscribe(None)
        };
        let line = request.encode_parameters(TransportKind::WebSocket)?;

        let group = required_param(&line, "LS_group");
        assert!(!group.contains(RESERVED));
        assert!(has_escape(group, "%26"));

        let schema = required_param(&line, "LS_schema");
        assert!(!schema.contains(RESERVED));
        assert!(has_escape(schema, "%3D"));

        Ok(())
    }

    #[test]
    fn test_subscription_mode_wire_values() {
        assert_eq!(SubscriptionMode::Raw.as_str(), "RAW");
        assert_eq!(SubscriptionMode::Merge.as_str(), "MERGE");
        assert_eq!(SubscriptionMode::Distinct.as_str(), "DISTINCT");
        assert_eq!(SubscriptionMode::Command.as_str(), "COMMAND");
    }

    #[test]
    fn test_control_operation_wire_values() {
        // The six non-MPN op-codes [§5.2].
        assert_eq!(ControlOperation::Add.as_str(), "add");
        assert_eq!(ControlOperation::Delete.as_str(), "delete");
        assert_eq!(ControlOperation::Reconf.as_str(), "reconf");
        assert_eq!(ControlOperation::Constrain.as_str(), "constrain");
        assert_eq!(ControlOperation::ForceRebind.as_str(), "force_rebind");
        assert_eq!(ControlOperation::Destroy.as_str(), "destroy");
    }

    // -- control: delete ----------------------------------------------------

    #[test]
    fn test_unsubscribe_http_body_matches_spec_example_p32() -> Result<(), ProtocolError> {
        let request = Unsubscribe {
            session: Some(CONTROL_SESSION.to_owned()),
            request_id: req_id(2),
            subscription_id: sub_id(1),
            ack: None,
        };
        assert_eq!(
            request.http_body()?,
            "LS_session=Sd9fce58fb5dbbebfT2255126&LS_reqId=2&LS_op=delete&LS_subId=1"
        );

        Ok(())
    }

    #[test]
    fn test_unsubscribe_ws_message_matches_spec_example_p32() -> Result<(), ProtocolError> {
        let request = Unsubscribe {
            session: None,
            request_id: req_id(2),
            subscription_id: sub_id(1),
            ack: None,
        };
        assert_eq!(
            request.ws_message()?,
            "control\r\nLS_reqId=2&LS_op=delete&LS_subId=1"
        );

        Ok(())
    }

    #[test]
    fn test_unsubscribe_ack_only_on_ws() -> Result<(), ProtocolError> {
        let request = Unsubscribe {
            session: Some(CONTROL_SESSION.to_owned()),
            request_id: req_id(2),
            subscription_id: sub_id(1),
            ack: Some(false),
        };
        assert!(!request.http_body()?.contains("LS_ack"));
        assert!(
            request
                .encode_parameters(TransportKind::WebSocket)?
                .ends_with("&LS_ack=false")
        );

        Ok(())
    }

    // -- control: reconf ----------------------------------------------------

    #[test]
    fn test_reconf_http_body_matches_spec_example_p33() -> Result<(), ProtocolError> {
        let request = ReconfigureSubscription {
            session: Some(CONTROL_SESSION.to_owned()),
            request_id: req_id(3),
            subscription_id: sub_id(1),
            requested_max_frequency: Some(MaxFrequencyLimit::Limited(DecimalNumber::try_new(
                "2.0",
            )?)),
        };
        assert_eq!(
            request.http_body()?,
            "LS_session=Sd9fce58fb5dbbebfT2255126&LS_reqId=3&LS_op=reconf&LS_subId=1\
             &LS_requested_max_frequency=2.0"
        );

        Ok(())
    }

    #[test]
    fn test_reconf_ws_message_matches_spec_example_p33() -> Result<(), ProtocolError> {
        let request = ReconfigureSubscription {
            session: None,
            request_id: req_id(3),
            subscription_id: sub_id(1),
            requested_max_frequency: Some(MaxFrequencyLimit::Limited(DecimalNumber::try_new(
                "2.0",
            )?)),
        };
        assert_eq!(
            request.ws_message()?,
            "control\r\nLS_reqId=3&LS_op=reconf&LS_subId=1&LS_requested_max_frequency=2.0"
        );

        Ok(())
    }

    #[test]
    fn test_reconf_unlimited_relieves_the_limit_and_none_omits_it() -> Result<(), ProtocolError> {
        let request = ReconfigureSubscription {
            session: None,
            request_id: req_id(3),
            subscription_id: sub_id(1),
            requested_max_frequency: Some(MaxFrequencyLimit::Unlimited),
        };
        assert!(
            request
                .encode_parameters(TransportKind::WebSocket)?
                .ends_with("&LS_requested_max_frequency=unlimited")
        );

        let request = ReconfigureSubscription {
            requested_max_frequency: None,
            ..request
        };
        assert_eq!(
            request.encode_parameters(TransportKind::WebSocket)?,
            "LS_reqId=3&LS_op=reconf&LS_subId=1"
        );

        Ok(())
    }

    // -- control: constrain -------------------------------------------------

    #[test]
    fn test_constrain_http_body_matches_spec_example_p34() -> Result<(), ProtocolError> {
        let request = ConstrainSession {
            session: Some(CONTROL_SESSION.to_owned()),
            request_id: req_id(4),
            requested_max_bandwidth: Some(MaxBandwidth::Limited(DecimalNumber::try_new("50.0")?)),
        };
        assert_eq!(
            request.http_body()?,
            "LS_session=Sd9fce58fb5dbbebfT2255126&LS_reqId=4&LS_op=constrain\
             &LS_requested_max_bandwidth=50.0"
        );

        Ok(())
    }

    #[test]
    fn test_constrain_ws_message_matches_spec_example_p34() -> Result<(), ProtocolError> {
        let request = ConstrainSession {
            session: None,
            request_id: req_id(4),
            requested_max_bandwidth: Some(MaxBandwidth::Limited(DecimalNumber::try_new("50.0")?)),
        };
        assert_eq!(
            request.ws_message()?,
            "control\r\nLS_reqId=4&LS_op=constrain&LS_requested_max_bandwidth=50.0"
        );

        Ok(())
    }

    #[test]
    fn test_constrain_unlimited_and_omitted_bandwidth() -> Result<(), ProtocolError> {
        let request = ConstrainSession {
            session: None,
            request_id: req_id(4),
            requested_max_bandwidth: Some(MaxBandwidth::Unlimited),
        };
        assert_eq!(
            request.encode_parameters(TransportKind::WebSocket)?,
            "LS_reqId=4&LS_op=constrain&LS_requested_max_bandwidth=unlimited"
        );

        let request = ConstrainSession {
            requested_max_bandwidth: None,
            ..request
        };
        assert_eq!(
            request.encode_parameters(TransportKind::WebSocket)?,
            "LS_reqId=4&LS_op=constrain"
        );

        Ok(())
    }

    // -- control: force_rebind ----------------------------------------------

    #[test]
    fn test_force_rebind_http_body_matches_spec_example_p35() -> Result<(), ProtocolError> {
        let request = ForceRebind {
            session: Some(CONTROL_SESSION.to_owned()),
            request_id: req_id(5),
            polling_millis: None,
            close_socket: None,
        };
        assert_eq!(
            request.http_body()?,
            "LS_session=Sd9fce58fb5dbbebfT2255126&LS_reqId=5&LS_op=force_rebind"
        );

        Ok(())
    }

    #[test]
    fn test_force_rebind_ws_message_matches_spec_example_p35() -> Result<(), ProtocolError> {
        let request = ForceRebind {
            session: None,
            request_id: req_id(5),
            polling_millis: None,
            close_socket: None,
        };
        assert_eq!(
            request.ws_message()?,
            "control\r\nLS_reqId=5&LS_op=force_rebind"
        );

        Ok(())
    }

    #[test]
    fn test_force_rebind_optional_parameters() -> Result<(), ProtocolError> {
        let request = ForceRebind {
            session: None,
            request_id: req_id(5),
            polling_millis: Some(2000),
            close_socket: Some(true),
        };
        assert_eq!(
            request.encode_parameters(TransportKind::WebSocket)?,
            "LS_reqId=5&LS_op=force_rebind&LS_polling_millis=2000&LS_close_socket=true"
        );

        Ok(())
    }

    // -- control: destroy ---------------------------------------------------

    #[test]
    fn test_destroy_http_body_matches_spec_example_p36() -> Result<(), ProtocolError> {
        let request = DestroySession {
            session: Some(CONTROL_SESSION.to_owned()),
            request_id: req_id(6),
            cause: None,
            close_socket: None,
        };
        assert_eq!(
            request.http_body()?,
            "LS_session=Sd9fce58fb5dbbebfT2255126&LS_reqId=6&LS_op=destroy"
        );

        Ok(())
    }

    #[test]
    fn test_destroy_ws_message_matches_spec_example_p36() -> Result<(), ProtocolError> {
        let request = DestroySession {
            session: None,
            request_id: req_id(6),
            cause: None,
            close_socket: None,
        };
        assert_eq!(request.ws_message()?, "control\r\nLS_reqId=6&LS_op=destroy");

        Ok(())
    }

    #[test]
    fn test_destroy_cause_code_and_message() -> Result<(), ProtocolError> {
        let request = DestroySession {
            session: None,
            request_id: req_id(6),
            cause: Some(DestroyCause {
                code: CauseCode::try_new(-7)?,
                message: Some(CauseMessage::try_new("bye")?),
            }),
            close_socket: Some(true),
        };
        assert_eq!(
            request.encode_parameters(TransportKind::WebSocket)?,
            "LS_reqId=6&LS_op=destroy&LS_cause_code=-7&LS_cause_message=bye\
             &LS_close_socket=true"
        );

        Ok(())
    }

    #[test]
    fn test_destroy_cause_code_without_message_is_legal() -> Result<(), ProtocolError> {
        // "If LS_cause_code is present and LS_cause_message is not specified,
        // then the reported message will be null" [§11.1].
        let request = DestroySession {
            session: None,
            request_id: req_id(6),
            cause: Some(DestroyCause {
                code: CauseCode::try_new(0)?,
                message: None,
            }),
            close_socket: None,
        };
        assert_eq!(
            request.encode_parameters(TransportKind::WebSocket)?,
            "LS_reqId=6&LS_op=destroy&LS_cause_code=0"
        );

        Ok(())
    }

    #[test]
    fn test_cause_code_rejects_a_positive_code() {
        // "Only 0 or a negative code are supported; a positive code will be
        // collapsed to 0" [§11.1] — refused rather than silently rewritten.
        assert!(CauseCode::try_new(1).is_err());
        assert!(CauseCode::try_new(0).is_ok());
        assert!(CauseCode::try_new(-31).is_ok());
    }

    #[test]
    fn test_cause_message_rejects_multiline_text() {
        // "Multiline text is also not allowed" [§11.1].
        assert!(CauseMessage::try_new("one\r\ntwo").is_err());
        assert!(CauseMessage::try_new("one\ntwo").is_err());
        assert!(CauseMessage::try_new("one two").is_ok());
    }

    #[test]
    fn test_destroy_escapes_the_cause_message() -> Result<(), ProtocolError> {
        let request = DestroySession {
            session: None,
            request_id: req_id(6),
            cause: Some(DestroyCause {
                code: CauseCode::try_new(0)?,
                message: Some(CauseMessage::try_new("a=b&c")?),
            }),
            close_socket: None,
        };
        let line = request.encode_parameters(TransportKind::WebSocket)?;
        let message = required_param(&line, "LS_cause_message");
        assert!(!message.contains(RESERVED));
        assert!(has_escape(message, "%3D"));
        assert!(has_escape(message, "%26"));

        Ok(())
    }

    // -- msg ----------------------------------------------------------------

    fn spec_message(session: Option<&str>) -> MessageSend {
        MessageSend {
            session: session.map(str::to_owned),
            request_id: req_id(11),
            message: "Hello".to_owned(),
            sequence: Some(match SequenceName::try_new("CHAT") {
                Ok(name) => name,
                Err(error) => panic!("`CHAT` is a valid sequence name: {error}"),
            }),
            msg_prog: NonZeroU32::new(1),
            max_wait_millis: None,
            ack: None,
            outcome: None,
        }
    }

    #[test]
    fn test_message_send_http_body_matches_spec_example_p43() -> Result<(), ProtocolError> {
        assert_eq!(
            spec_message(Some(CONTROL_SESSION)).http_body()?,
            "LS_session=Sd9fce58fb5dbbebfT2255126&LS_reqId=11&LS_sequence=CHAT\
             &LS_msg_prog=1&LS_message=Hello"
        );

        Ok(())
    }

    #[test]
    fn test_message_send_ws_message_matches_spec_example_p43() -> Result<(), ProtocolError> {
        assert_eq!(
            spec_message(None).ws_message()?,
            "msg\r\nLS_reqId=11&LS_sequence=CHAT&LS_msg_prog=1&LS_message=Hello"
        );

        Ok(())
    }

    #[test]
    fn test_message_send_carries_no_op_parameter() -> Result<(), ProtocolError> {
        // "Makes use of a different request name and hence does not require
        // the LS_op parameter" [§12].
        assert!(!spec_message(None).ws_message()?.contains("LS_op="));
        assert_eq!(MessageSend::PATH, "/lightstreamer/msg.txt");

        Ok(())
    }

    #[test]
    fn test_message_send_requires_msg_prog_with_a_sequence() {
        // "Mandatory whenever LS_sequence is specified" [§12.1].
        let request = MessageSend {
            msg_prog: None,
            ..spec_message(None)
        };
        assert!(matches!(
            request.encode_parameters(TransportKind::WebSocket),
            Err(ProtocolError::Request { request: "msg", .. })
        ));
    }

    #[test]
    fn test_message_send_requires_msg_prog_when_outcome_is_requested() {
        // "…or LS_outcome is set to true", whose own default is true [§12.1,
        // §12.2]. The literal reading is enforced.
        let request = MessageSend {
            sequence: None,
            msg_prog: None,
            ..spec_message(None)
        };
        assert!(request.encode_parameters(TransportKind::WebSocket).is_err());

        let request = MessageSend {
            outcome: Some(true),
            ..request
        };
        assert!(request.encode_parameters(TransportKind::WebSocket).is_err());
    }

    #[test]
    fn test_message_send_fire_and_forget_needs_no_msg_prog() -> Result<(), ProtocolError> {
        // With both LS_ack and LS_outcome false there is no notification at
        // all, and LS_msg_prog is then genuinely optional [§12.1, §12.2].
        let request = MessageSend {
            sequence: None,
            msg_prog: None,
            ack: Some(false),
            outcome: Some(false),
            ..spec_message(None)
        };
        assert_eq!(
            request.encode_parameters(TransportKind::WebSocket)?,
            "LS_reqId=11&LS_ack=false&LS_outcome=false&LS_message=Hello"
        );

        Ok(())
    }

    #[test]
    fn test_message_send_max_wait_and_ack_and_outcome() -> Result<(), ProtocolError> {
        let request = MessageSend {
            max_wait_millis: Some(1500),
            ack: Some(true),
            outcome: Some(true),
            ..spec_message(None)
        };
        assert_eq!(
            request.encode_parameters(TransportKind::WebSocket)?,
            "LS_reqId=11&LS_sequence=CHAT&LS_msg_prog=1&LS_max_wait=1500\
             &LS_ack=true&LS_outcome=true&LS_message=Hello"
        );

        Ok(())
    }

    #[test]
    fn test_message_send_max_wait_without_a_sequence_is_error() {
        // "Ignored if LS_sequence is omitted" [§12.1] — refused rather than
        // emitted, under the module-level policy on ignored parameters.
        let request = MessageSend {
            sequence: None,
            max_wait_millis: Some(1500),
            outcome: Some(false),
            ack: Some(false),
            ..spec_message(None)
        };
        assert!(request.encode_parameters(TransportKind::WebSocket).is_err());
    }

    #[test]
    fn test_message_send_escapes_the_payload() -> Result<(), ProtocolError> {
        // "Any text string" [§12.1] — so the payload is the value most likely
        // to carry a reserved character.
        let request = MessageSend {
            message: "a=1&b=2\r\n100%".to_owned(),
            ..spec_message(None)
        };
        let line = request.encode_parameters(TransportKind::WebSocket)?;
        let message = required_param(&line, "LS_message");
        assert!(!message.contains(RESERVED));
        for escape in ["%3D", "%26", "%0D", "%0A", "%25"] {
            assert!(has_escape(message, escape), "missing {escape}");
        }
        // The escaped payload must not reintroduce a line separator, which
        // would split one request into two [`docs/spec/01-foundations.md`
        // §6.2.2].
        assert!(!line.contains('\r'));
        assert!(!line.contains('\n'));

        Ok(())
    }

    #[test]
    fn test_message_send_http_without_session_is_error() {
        assert!(spec_message(None).http_body().is_err());
    }

    #[test]
    fn test_sequence_name_rejects_the_reserved_identifier() {
        // "UNORDERED_MESSAGES … is now reserved and can't be used. If
        // specified it leads to an error" [§12.1].
        assert!(SequenceName::try_new("UNORDERED_MESSAGES").is_err());
        assert!(SequenceName::try_new("CHAT_1").is_ok());
    }

    #[test]
    fn test_sequence_name_rejects_invalid_characters_and_empty() {
        assert!(SequenceName::try_new("").is_err());
        assert!(SequenceName::try_new("chat room").is_err());
        assert!(SequenceName::try_new("chat-room").is_err());
    }

    // -- heartbeat ----------------------------------------------------------

    #[test]
    fn test_heartbeat_http_body_matches_spec_example_p46() -> Result<(), ProtocolError> {
        let request = Heartbeat {
            session: Some(CONTROL_SESSION.to_owned()),
        };
        assert_eq!(request.http_body()?, "LS_session=Sd9fce58fb5dbbebfT2255126");
        assert_eq!(
            Heartbeat::http_target(),
            "/lightstreamer/heartbeat.txt?LS_protocol=TLCP-2.5.0"
        );

        Ok(())
    }

    #[test]
    fn test_heartbeat_ws_message_matches_spec_example_p46() -> Result<(), ProtocolError> {
        let request = Heartbeat {
            session: Some(CONTROL_SESSION.to_owned()),
        };
        assert_eq!(
            request.ws_message()?,
            "heartbeat\r\nLS_session=Sd9fce58fb5dbbebfT2255126"
        );

        Ok(())
    }

    #[test]
    fn test_heartbeat_ws_without_parameters_keeps_the_empty_parameter_line()
    -> Result<(), ProtocolError> {
        // "Since the request here has no parameters, the final line
        // terminator must be included" [§14.2].
        assert_eq!(Heartbeat::default().ws_message()?, "heartbeat\r\n");

        Ok(())
    }

    #[test]
    fn test_heartbeat_carries_no_req_id_and_no_op() -> Result<(), ProtocolError> {
        // "heartbeat takes no LS_reqId and no LS_op" [§14.1].
        let message = Heartbeat {
            session: Some(CONTROL_SESSION.to_owned()),
        }
        .ws_message()?;
        assert!(!message.contains("LS_reqId"));
        assert!(!message.contains("LS_op"));

        Ok(())
    }

    #[test]
    fn test_heartbeat_http_without_session_is_error() {
        assert!(Heartbeat::default().http_body().is_err());
    }

    // -- wsok ---------------------------------------------------------------

    #[test]
    fn test_wsok_message_matches_spec_example_p47() {
        // "Not only does the request require no parameters, but also the
        // parameter line … is missing in this case" [§15.1].
        assert_eq!(WsOk::ws_message(), "wsok");
        assert!(!WsOk::ws_message().contains('\r'));
    }

    // -- shared value types -------------------------------------------------

    #[test]
    fn test_request_id_rejects_empty_and_non_alphanumeric() -> Result<(), ProtocolError> {
        // "Any combination of letters and numbers" [§5.1].
        assert!(RequestId::try_new("").is_err());
        assert!(RequestId::try_new("1-2").is_err());
        assert!(RequestId::try_new("req 1").is_err());
        assert_eq!(RequestId::try_new("abc123")?.as_str(), "abc123");
        assert_eq!(RequestId::from_progressive(42).as_str(), "42");

        Ok(())
    }

    #[test]
    fn test_decimal_number_accepts_the_spec_example_forms() -> Result<(), ProtocolError> {
        // The spec writes `2.0` [§8.3] and `50.0` [§9.3].
        assert_eq!(DecimalNumber::try_new("2.0")?.as_str(), "2.0");
        assert_eq!(DecimalNumber::try_new("50.0")?.as_str(), "50.0");
        assert!(DecimalNumber::try_new("50").is_ok());

        Ok(())
    }

    #[test]
    fn test_decimal_number_rejects_non_decimal_forms() {
        for bad in [
            "", ".", ".5", "5.", "1,5", "-1", "+1", "1e3", "1.2.3", "abc",
        ] {
            assert!(
                DecimalNumber::try_new(bad).is_err(),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn test_protocol_and_websocket_constants() {
        // [`docs/spec/01-foundations.md` §6.1.1, §6.2.1].
        assert_eq!(PROTOCOL_VERSION, "TLCP-2.5.0");
        assert_eq!(WS_SUBPROTOCOL, "TLCP-2.5.0.lightstreamer.com");
        assert_eq!(WS_PATH, "/lightstreamer");
        assert_eq!(CRLF, "\r\n");
    }

    #[test]
    fn test_every_http_path_matches_the_quick_index() {
        // [`docs/spec/03-requests.md` §16].
        assert_eq!(CreateSession::PATH, "/lightstreamer/create_session.txt");
        assert_eq!(BindSession::PATH, "/lightstreamer/bind_session.txt");
        assert_eq!(Subscribe::PATH, "/lightstreamer/control.txt");
        assert_eq!(Unsubscribe::PATH, "/lightstreamer/control.txt");
        assert_eq!(ReconfigureSubscription::PATH, "/lightstreamer/control.txt");
        assert_eq!(ConstrainSession::PATH, "/lightstreamer/control.txt");
        assert_eq!(ForceRebind::PATH, "/lightstreamer/control.txt");
        assert_eq!(DestroySession::PATH, "/lightstreamer/control.txt");
        assert_eq!(MessageSend::PATH, "/lightstreamer/msg.txt");
        assert_eq!(Heartbeat::PATH, "/lightstreamer/heartbeat.txt");
    }

    #[test]
    fn test_every_request_name_matches_the_quick_index() {
        assert_eq!(CreateSession::NAME, "create_session");
        assert_eq!(BindSession::NAME, "bind_session");
        assert_eq!(Subscribe::NAME, "control");
        assert_eq!(MessageSend::NAME, "msg");
        assert_eq!(Heartbeat::NAME, "heartbeat");
        assert_eq!(WsOk::NAME, "wsok");
    }

    #[test]
    fn test_control_operation_constants_per_request_type() {
        assert_eq!(Subscribe::OPERATION, ControlOperation::Add);
        assert_eq!(Unsubscribe::OPERATION, ControlOperation::Delete);
        assert_eq!(ReconfigureSubscription::OPERATION, ControlOperation::Reconf);
        assert_eq!(ConstrainSession::OPERATION, ControlOperation::Constrain);
        assert_eq!(ForceRebind::OPERATION, ControlOperation::ForceRebind);
        assert_eq!(DestroySession::OPERATION, ControlOperation::Destroy);
    }
}
