# TLCP Error and Cause Codes — Complete Catalog

Scope: this chapter is a cross-cutting sweep of the **TLCP Specification, version 2.5.0** (Text Lightstreamer Client Protocol; target: Lightstreamer Server v. 7.4.0 or greater; last updated 19/3/2024) for every error and cause code the Server can emit, wherever the code appears in the 99-page document — session creation and binding failures, session-termination causes, control-request failures, message-send failures, heartbeat failures, WebSocket-check responses, and codes embedded in notifications. Every factual statement carries a citation of the form `[§Section Name, p.NN]`, where `NN` is the **printed** page number of the specification. Codes and wire tokens are reproduced verbatim in backticks, with the exact sign, digits, and casing used by the spec.

---

## 1. Where codes come from

The spec defines exactly **two** code catalogs, both in its appendices:

| Catalog | Title | Pages | Spec's own statement of applicability |
| --- | --- | --- | --- |
| **A** | *Appendix A: Session Error Codes* | pp. 88–90 | "The following error codes may be used with session error responses such as `CONERR`, or by session notifications such as `END`" `[§Appendix A: Session Error Codes, p.88]` |
| **B** | *Appendix B: Control Error Codes* | pp. 91–94 | "The following cause codes may be used with control error responses such as `REQERR` and `ERROR`" `[§Appendix B: Control Error Codes, p.91]` |

Cross-references in the body of the document bind each error-bearing construct to one of the two catalogs:

| Construct | Catalog it points to | Citation |
| --- | --- | --- |
| `CONERR` `<error-code>` | Appendix A | `[§Other Session Creation or Binding Responses, p.28]` |
| `END` `<cause-code>` (as a *response* to a binding request) | Appendix A | `[§Other Session Creation or Binding Responses, p.28]` |
| `END` `<cause-code>` (as a *stream notification*) | Appendix A | `[§Stream Connection End, p.65]` |
| `REQERR` `<error-code>` | Appendix B | `[§Other Control Responses, p.45]` |
| `ERROR` `<error-code>` (control request) | Appendix B | `[§Other Control Responses, p.45]` |
| `ERROR` `<error-code>` (heartbeat) | Appendix B | `[§Client Heartbeat → Other Responses, p.47]` |
| `MSGFAIL` `<error-code>` | Appendix B | `[§Message Send Failed, p.60]` |

⚠️ **Spec unclear:** both appendix preambles are phrased with "such as" — "session error responses **such as** `CONERR`, or by session notifications **such as** `END`" `[§Appendix A, p.88]` and "control error responses **such as** `REQERR` and `ERROR`" `[§Appendix B, p.91]`. Neither list is presented as exhaustive, so the spec does not close the set of tags that may carry a code from each catalog. In particular, the Appendix B preamble does not mention `MSGFAIL`, even though `[§Message Send Failed, p.60]` explicitly routes `MSGFAIL` to Appendix B.

⚠️ **Spec unclear:** the spec never maps an individual code to a specific carrying tag. Membership in Appendix A or B is the only mapping given; beyond that, the only per-code contextual information is whatever the code's own one-line description happens to name (e.g. "on a `bind_session` request"). The "Context per spec" column below reproduces only that wording and is left as `—` where the spec names no context.

Transport-level failure codes are **explicitly out of scope** of TLCP: requests "so wrong as to break the protocol (or involved in severe processing issues) may cause the interruption of the connection they were sent to. Such interruption may either be abrupt or provide an error code leveraging the transport syntax (e.g. HTTP Response Status Code or WebSocket Close Status Code). However, the interruption details are not in the scope of the TLCP protocol" `[§Requests, Responses, and Notifications, p.10]`.

### Note on the recoverability column

The spec has **no formal recoverable/lost taxonomy**. The "Recoverability per spec" column therefore records only explicit statements found in the code's own description or elsewhere in the document; every other entry reads *Not stated*. Section 5 collects the general statements the spec does make about what a client is left with after each class of failure.

---

## 2. Master table

Catalog **A** = *Appendix A: Session Error Codes*; catalog **B** = *Appendix B: Control Error Codes*. Where the same numeric value appears in both catalogs, it gets one row per catalog, because the two meanings differ.

| Code | Cat. | Carried by | Context per spec | Spec's own description | Recoverability per spec | Page |
| --- | --- | --- | --- | --- | --- | --- |
| `1` | A | `CONERR`, `END` | — | "User/password check failed." | Not stated | p.88 |
| `2` | A | `CONERR`, `END` | — | "Requested Adapter Set not available." | Not stated | p.88 |
| `3` | A | `CONERR`, `END` | `bind_session` request | "The session specified on a `bind_session` request was initiated with a different and incompatible communication protocol or protocol version." | Not stated | p.88 |
| `4` | A | `CONERR`, `END` | `bind_session` request with `LS_recovery_from` | "The progressive specified on a `bind_session` request to recovery from has not been kept by the Server, hence the recovery attempt failed." | Recovery attempt failed; the spec's general rule for a failed recovery is that "the Client can't but create a new stream connection as if it was the first time it connects" `[§Session Life Cycle, p.7]` | p.88 |
| `5` | A | `CONERR`, `END` | Session creation request | "The Server is temporarily overloaded and cannot accept session creation requests: retry later." | Spec says **retry later** | p.88 |
| `6` | A | `CONERR`, `END` | Session creation request | "The Metadata Adapter is temporarily unable to validate the user credentials upon a session creation request: retry later." | Spec says **retry later** | p.88 |
| `7` | A | `CONERR`, `END` | — | "Licensed maximum number of actively started sessions reached (this can only happen with some licenses)." | Not stated | p.88 |
| `8` | A | `CONERR`, `END` | — | "Configured maximum number of actively started sessions reached." | Not stated | p.88 |
| `9` | A | `CONERR`, `END` | `create_session` request | "Configured maximum server load reached on a `create_session` request." | Not stated | p.88 |
| `10` | A | `CONERR`, `END` | — | "New sessions temporarily blocked." | Not stated (the word "temporarily" is the spec's) | p.88 |
| `11` | A | `CONERR`, `END` | — | "Streaming is not available because of Server license restrictions (this can only happen with special licenses)." | Not stated | p.88 |
| `20` | A | `CONERR`, `END` | `bind_session` request | "Specified session not found on a `bind_session` request." | Not stated | p.88 |
| `21` | A | `CONERR`, `END` | `bind_session` request | "The session ID specified in a `bind_session` request is not compatible with this Server instance and it is not likely to pertain to an instance just closed; it might pertain to a different instance." | Not stated per-code; see the clustering note at `[§HTTP Loop and Failed Rebind, p.20]` reproduced in §5.3 | p.89 |
| `31` | A | `CONERR`, `END` | Client `destroy` request | "The session was closed (possibly by the administrator) through a client *destroy* request." | Session closed | p.89 |
| `32` | A | `CONERR`, `END` | Server-side closure | "The session was closed on the Server side, via software or by the administrator." | Session closed | p.89 |
| `33`, `34` | A | `CONERR`, `END` | Session in activity | "An unexpected error occurred on the Server while the session was in activity." (one entry covers both codes) | Not stated | p.89 |
| `35` | A | `CONERR`, `END` | New session opened for the same user | "The Metadata Adapter only allows a limited number of sessions for the current user and has requested the closure of the current session upon opening of a new session for the same user by some client." | Session closed | p.89 |
| `40` | A | `CONERR`, `END` | Manual rebind by another client | "A manual rebind to this session has been performed by some client." | Not stated | p.89 |
| `48` | A | `CONERR`, `END` | Maximum session duration | "The maximum session duration has been reached, according to Server configuration or as enforced by the administrator. This is only meant as a way to refresh the session (for instance, to force a different association in a clustering scenario), hence the client should recover by opening a new session immediately." | Spec says the client **should recover by opening a new session immediately** | p.89 |
| `60` | A | `CONERR`, `END` | — | "Client version not supported with the current Server." | Not stated | p.89 |
| `64` | A | `CONERR`, `END` | Session/control combo request | "Generic error in the control request part of a session/control combo request." | The combo operation is atomic: "if the control request fails, the session creation also fails" `[§Session Creation and Control Combo Request, p.66]` | p.89 |
| `65` | A | `CONERR`, `END` | — | "Syntax error or invalid values specified for one or more arguments." | Not stated | p.89 |
| `66` | A | `CONERR`, `END` | `create_session` request | "Unchecked exception thrown in the Metadata Adapter while managing a `create_session` request." | Not stated | p.89 |
| `67` | A | `CONERR`, `END` | — | "Malformed request." | Not stated | p.89 |
| `68` | A | `CONERR`, `END` | — | "An unexpected error occurred on the Server while managing the request." | Not stated | p.89 |
| `69` | A | `CONERR`, `END` | Streaming request on a WebSocket | "Streaming request on a WebSocket not allowed, as a streaming is already in place." | Not stated | p.89 |
| `70` | A | `CONERR`, `END` | Session creation request | "Session creation request not allowed on this Server port." | Not stated | p.90 |
| `71` | A | `CONERR`, `END` | — | "Client type not supported with the current Server." | Not stated | p.90 |
| `≤ 0` | A | `CONERR`, `END` | Metadata Adapter supplied | "If the code is `0` or negative, it has been supplied by the **Metadata Adapter** and the interpretation is application-specific. In particular if used with and `END` notification, means the session was closed by an Administrator with a *destroy* command and the code was supplied as a custom cause code." (verbatim, including the spec's "with and") | Application-specific | p.90 |
| `3` | B | `REQERR`, `ERROR`, `MSGFAIL` | — | "The session specified was initiated with a different communication protocol or protocol version, which is incompatible with this request." | Not stated | p.91 |
| `11` | B | `REQERR`, `ERROR`, `MSGFAIL` | — | "The specified session ID is not compatible with this Server instance and it is not likely to pertain to an instance just closed; it might pertain to a different instance." | Not stated | p.91 |
| `13` | B | `REQERR`, `ERROR`, `MSGFAIL` | Subscription reconfiguration | "Subscription frequency reconfiguration not allowed because the subscription is configured for unfiltered dispatching." | Not stated | p.91 |
| `15` | B | `REQERR`, `ERROR`, `MSGFAIL` | Subscription in `COMMAND` mode | "Subscription to an item in `COMMAND` mode with no \"key\" field in the schema." | Not stated | p.91 |
| `16` | B | `REQERR`, `ERROR`, `MSGFAIL` | Subscription in `COMMAND` mode | "Subscription to an item in `COMMAND` mode with no \"command\" field in the schema." | Not stated | p.91 |
| `17` | B | `REQERR`, `ERROR`, `MSGFAIL` | — | "Bad Data Adapter name or default Data Adapter not defined." | Not stated | p.91 |
| `19` | B | `REQERR`, `ERROR`, `MSGFAIL` | — | "Specified subscription not found." | Not stated | p.91 |
| `20` | B | `REQERR`, `ERROR`, `MSGFAIL` | — | "Specified (or implied) session not found." | Not stated | p.91 |
| `21` | B | `REQERR`, `ERROR`, `MSGFAIL` | — | "Bad Item Group name." | Not stated | p.91 |
| `22` | B | `REQERR`, `ERROR`, `MSGFAIL` | — | "Bad Item Group name for this Field schema." | Not stated | p.91 |
| `23` | B | `REQERR`, `ERROR`, `MSGFAIL` | — | "Bad Field schema name." | Not stated | p.91 |
| `24` | B | `REQERR`, `ERROR`, `MSGFAIL` | — | "Subscription mode not allowed for an Item." | Not stated | p.91 |
| `25` | B | `REQERR`, `ERROR`, `MSGFAIL` | — | "Bad Selector name." | Not stated | p.91 |
| `26` | B | `REQERR`, `ERROR`, `MSGFAIL` | — | "Unfiltered dispatching not allowed for an Item, because a frequency limit is associated to the Item." | Not stated | p.92 |
| `27` | B | `REQERR`, `ERROR`, `MSGFAIL` | — | "Unfiltered dispatching not supported for an Item, because a frequency prefiltering is applied for the Item." | Not stated | p.92 |
| `28` | B | `REQERR`, `ERROR`, `MSGFAIL` | License terms | "Unfiltered dispatching is not allowed by the current license terms." | Not stated | p.92 |
| `29` | B | `REQERR`, `ERROR`, `MSGFAIL` | License terms | "`RAW` mode is not allowed by the current license terms." | Not stated | p.92 |
| `30` | B | `REQERR`, `ERROR`, `MSGFAIL` | License terms | "Subscriptions are not allowed by the current license terms (for special licenses only)." | Not stated | p.92 |
| `32` | B | `REQERR`, `ERROR`, `MSGFAIL` | Message send | "The specified message progressive number is too low: either a message with this number has already been enqueued (and possibly processed) or the number has already been skipped by timeout (the exact case cannot be determined)." | Not stated | p.92 |
| `33` | B | `REQERR`, `ERROR`, `MSGFAIL` | Message send | "The specified message progressive number is too low: a message with this number has already been enqueued (and possibly processed)." | Not stated | p.92 |
| `34` | B | `REQERR`, `ERROR`, `MSGFAIL` | Message processing | "An unexpected error occurred on the Server while managing the message (for instance, a `NotificationException` was thrown by the Metadata Adapter)." | Not stated | p.92 |
| `35` | B | `REQERR`, `ERROR`, `MSGFAIL` | Message processing | "Unchecked exception thrown in the Metadata Adapter while managing the message." | Not stated | p.92 |
| `38` | B | `REQERR`, `ERROR`, `MSGFAIL` | Message send | "The specified progressive number has been skipped by timeout." | Not stated | p.92 |
| `39` | B | `REQERR`, `ERROR`, `MSGFAIL` | Message send (batch of skipped progressives) | "The specified progressive number and some consecutive preceding numbers have been skipped by timeout; only in this case, `err_code` is an integer and specifies how many progressive numbers have been skipped; the notification pertains to all those progressive numbers." | Not stated — but see the ⚠️ below | p.92 |
| `40` | B | `REQERR`, `ERROR`, `MSGFAIL` | MPN | "The MPN Module is disabled, either by configuration or by license restrictions." | Not stated | p.92 |
| `41` | B | `REQERR`, `ERROR`, `MSGFAIL` | MPN | "The MPN request failed because of some internal resource error (e.g. database connection, timeout etc.)." | Not stated | p.92 |
| `42` | B | `REQERR`, `ERROR`, `MSGFAIL` | MPN | "The specified MPN platform is invalid or not supported." | Not stated | p.93 |
| `43` | B | `REQERR`, `ERROR`, `MSGFAIL` | MPN | "The specified MPN application ID is invalid or unknown." | Not stated | p.93 |
| `44` | B | `REQERR`, `ERROR`, `MSGFAIL` | MPN | "The syntax used in the specified MPN trigger expression is invalid." | Not stated | p.93 |
| `45` | B | `REQERR`, `ERROR`, `MSGFAIL` | MPN | "The specified MPN device ID is invalid or unknown." | Not stated | p.93 |
| `46` | B | `REQERR`, `ERROR`, `MSGFAIL` | MPN | "The specified MPN subscription ID is invalid or unknown." | Not stated | p.93 |
| `47` | B | `REQERR`, `ERROR`, `MSGFAIL` | MPN | "The specified MPN trigger expression or notification format contains an invalid argument name." | Not stated | p.93 |
| `48` | B | `REQERR`, `ERROR`, `MSGFAIL` | MPN | "The MPN device corresponding to the specified ID has been suspended." | Not stated | p.93 |
| `49` | B | `REQERR`, `ERROR`, `MSGFAIL` | MPN | "One or more arguments of the MPN request exceed its maximum size." | Not stated | p.93 |
| `50` | B | `REQERR`, `ERROR`, `MSGFAIL` | MPN | "The specified MPN subscription contains no items or no fields." | Not stated | p.93 |
| `51` | B | `REQERR`, `ERROR`, `MSGFAIL` | MPN | "The payload of the MPN subscription exceeds the maximum size for the platform." | Not stated | p.93 |
| `52` | B | `REQERR`, `ERROR`, `MSGFAIL` | MPN | "The specified MPN notification format is not a valid JSON structure." | Not stated | p.93 |
| `53` | B | `REQERR`, `ERROR`, `MSGFAIL` | MPN | "The specified MPN notification format is empty." | Not stated | p.93 |
| `56` | B | `REQERR`, `ERROR`, `MSGFAIL` | MPN | "The subscription parameters specified don't match with the specified MPN subscription." | Not stated | p.93 |
| `65` | B | `REQERR`, `ERROR`, `MSGFAIL` | — | "Syntax error or invalid values specified for one or more arguments." | Not stated | p.93 |
| `66` | B | `REQERR`, `ERROR`, `MSGFAIL` | Subscription / reconfiguration request | "Unchecked exception thrown in the Metadata Adapter while managing a subscription or reconfiguration request." | Not stated | p.93 |
| `67` | B | `REQERR`, `ERROR`, `MSGFAIL` | — | "Malformed request." | Not stated | p.94 |
| `68` | B | `REQERR`, `ERROR`, `MSGFAIL` | — | "An unexpected error occurred on the Server while managing the request." | Not stated | p.94 |
| `≤ 0` | B | `REQERR`, `ERROR`, `MSGFAIL` | Metadata Adapter supplied | "If the code is `0` or negative, it has been supplied by the **Metadata Adapter** and the interpretation is application-specific. This can affect subscription requests and message processing." | Application-specific | p.94 |

**Totals.** Appendix A defines **29** numeric codes plus the `≤ 0` range entry (30 catalog entries, occupying 29 rows above because the spec gives `33` and `34` a single shared description). Appendix B defines **43** numeric codes plus the `≤ 0` range entry (44 catalog entries, 44 rows). That is **74 catalog entries** across the two appendices, laid out here in 73 table rows. Because 14 numeric values appear in **both** catalogs with different meanings (`3`, `11`, `20`, `21`, `32`, `33`, `34`, `35`, `40`, `48`, `65`, `66`, `67`, `68`), the union of distinct numeric values is **58**, plus the `≤ 0` open range.

⚠️ **Spec unclear (code `39`):** the description says "only in this case, `err_code` is an integer and specifies how many progressive numbers have been skipped" `[§Appendix B, p.92]`. The identifier `err_code` is not defined anywhere else in TLCP 2.5.0 — the `MSGFAIL` argument names are `<error-code>` and `<error-message>` `[§Message Send Failed, p.60]`. Since the error code in this case *is* `39`, the sentence cannot be read as describing the `<error-code>` field, and the spec does not say which field carries the skip count. Do not guess.

⚠️ **Spec unclear (code `33`, `34` in Appendix A):** the two codes share a single bullet and a single description; the spec does not say what distinguishes `33` from `34` `[§Appendix A, p.89]`.

⚠️ **Spec unclear (codes `10`, `5`, `6`):** the spec labels these conditions "temporarily blocked" / "retry later" but never states a retry interval, a backoff policy, or a bound on the number of retries `[§Appendix A, p.88]`.

⚠️ **Spec unclear (gaps in the numbering):** both catalogs have gaps (Appendix A skips `12`–`19`, `22`–`30`, `36`–`39`, `41`–`47`, `49`–`59`, `61`–`63`; Appendix B skips `1`, `2`, `4`–`10`, `12`, `14`, `18`, `31`, `36`, `37`, `54`, `55`, `57`–`64`). The spec neither reserves these values nor says what a client should do with a positive code it does not recognise.

---

## 3. Per-context sub-tables

### 3.1 Session creation and binding failures — `CONERR`

`CONERR` is "used when: with both session creation and binding, when the session could not be created or bound" `[§Other Session Creation or Binding Responses, p.28]`. Its `<error-code>` is drawn from **Appendix A** `[§Other Session Creation or Binding Responses, p.28]`.

Codes whose Appendix A description explicitly names a session-creation or session-binding context:

| Code | Named context | Description | Page |
| --- | --- | --- | --- |
| `3` | `bind_session` request | Session was initiated with a different and incompatible communication protocol or protocol version | p.88 |
| `4` | `bind_session` request, recovery | The progressive to recover from has not been kept by the Server; the recovery attempt failed | p.88 |
| `5` | Session creation request | Server temporarily overloaded: retry later | p.88 |
| `6` | Session creation request | Metadata Adapter temporarily unable to validate credentials: retry later | p.88 |
| `9` | `create_session` request | Configured maximum server load reached | p.88 |
| `20` | `bind_session` request | Specified session not found | p.88 |
| `21` | `bind_session` request | Session ID not compatible with this Server instance | p.89 |
| `64` | Session/control combo request | Generic error in the control-request part | p.89 |
| `66` | `create_session` request | Unchecked exception in the Metadata Adapter | p.89 |
| `69` | Streaming request on a WebSocket | Not allowed, streaming already in place | p.89 |
| `70` | Session creation request | Not allowed on this Server port | p.90 |

The remaining Appendix A codes (`1`, `2`, `7`, `8`, `10`, `11`, `31`, `32`, `33`, `34`, `35`, `40`, `48`, `60`, `65`, `67`, `68`, `71`, `≤ 0`) carry no context marker in their descriptions.

Additionally, code `5` is named outside the appendix: a request "can also be discarded before completion (with error code `5`) when the Server is overloaded and it determines that a significant delay is still going to be accumulated" — a note attached to the `LS_ttl_millis` parameter of the session creation request `[§Session Creation Request, p.24]`.

Spec examples, verbatim:

```
CONERR,2,Requested Adapter Set not available
```
`[§Other Session Creation or Binding Responses, p.28]`

```
CONERR,64,Unexpected error while initializing the session: Metadata Provider refusal:
Mode MERGE is not supported for item item1
```
`[§Session Creation and Control Combo Request, p.67]` — the failure case of a combo request, shown identically for HTTP and WS transport.

### 3.2 Session termination causes — `END`

`END` appears in **two** distinct roles, both drawing their code from Appendix A:

1. As a **response to a session binding request**: "only with session rebinding, when the requested session has just been forcibly closed on the Server side" `[§Other Session Creation or Binding Responses, p.28]`.
2. As a **stream notification**: "to signal that the session has been closed by the Server […] e.g. on request by an administrator or as a consequence of a Session Destroy request" `[§Stream Connection End, pp.64–65]`.

Appendix A codes whose descriptions name a session-closure cause:

| Code | Cause | Page |
| --- | --- | --- |
| `31` | "The session was closed (possibly by the administrator) through a client *destroy* request." | p.89 |
| `32` | "The session was closed on the Server side, via software or by the administrator." | p.89 |
| `33`, `34` | "An unexpected error occurred on the Server while the session was in activity." | p.89 |
| `35` | Metadata Adapter requested closure of the current session upon opening of a new session for the same user by some client | p.89 |
| `40` | "A manual rebind to this session has been performed by some client." | p.89 |
| `48` | Maximum session duration reached; "the client should recover by opening a new session immediately" | p.89 |
| `≤ 0` | "In particular if used with and `END` notification, means the session was closed by an Administrator with a *destroy* command and the code was supplied as a custom cause code." | p.90 |

**Client-controlled `END` cause codes.** A Session Destroy Request may set the cause reported in the subsequent `END`: `LS_cause_code` is "a numeric code to be reported in the consequent `END` message that will be received on the stream connection **in place of the default code `31`**" `[§Session Destroy Request, p.35]`. Constraints stated by the spec `[§Session Destroy Request, p.35]`:

- "Only `0` or a negative code are supported; a positive code will be collapsed to `0`."
- `LS_cause_message` is "considered only if `LS_cause_code` is also supplied".
- "If `LS_cause_code` is not present, the cause in the `END` message will be provided by the Server, regardless of this setting."
- "if `LS_cause_code` is present and `LS_cause_message` is not specified, then the reported message will be `null`."
- The supplied message "should be in simple ASCII, otherwise it might be altered in order to be sent to the client; multiline text is also not allowed."
- "The supplied message should be short. If longer than 35 characters and if a content-length limit on the connection content is in force, it may be replaced with `[Custom message skipped]`."

Spec examples, verbatim:

```
END,8,Session count limit reached
```
`[§Other Session Creation or Binding Responses, p.28]` and, identically, `[§Stream Connection End, p.65]`.

```
END,31,Destroy invoked by client
```
`[§Disconnection, p.73]` and `[§Disconnection and Reconnection, p.85]`.

### 3.3 Control-request failures — `REQERR` and `ERROR`

All control requests, "including message send requests, share the same responses" `[§Control Responses, p.44]`.

| Tag | Used when | Code source | Page |
| --- | --- | --- | --- |
| `REQERR` | "an error occurred and the request could not be completed" | Appendix B | p.45 |
| `ERROR` | "the request could not be interpreted or processed" | Appendix B | p.45 |

The spec does **not** partition Appendix B between `REQERR` and `ERROR`; the distinction it gives is the one quoted above, plus the structural fact that `REQERR` carries a `<request-ID>` and `ERROR` does not — consistent with the general rule that "responses (including errors) are always identified by the corresponding request ID, **unless the request syntax was so misleading that a request ID could not be parsed**" `[§Requests, Responses, and Notifications, p.10]`.

Appendix B codes whose descriptions name a subscription-related context: `13`, `15`, `16`, `17`, `19`, `21`, `22`, `23`, `24`, `25`, `26`, `27`, `28`, `29`, `30`, `66` `[§Appendix B, pp.91–93]`. Session-related: `3`, `11`, `20` `[§Appendix B, p.91]`. Generic: `65`, `67`, `68`, `≤ 0` `[§Appendix B, pp.93–94]`.

`LS_ack` (subscription, unsubscription, and message send requests, WS transport only) suppresses the `REQOK` response but **never** an error: "If the request fails, the consequent `REQERR` or `ERROR` response is always sent" `[§Subscription Request, p.31]`, `[§Unsubscription Request, p.32]`, `[§Message Send Request, p.43]`. `LS_ack` is "ignored if used with HTTP transport, as an HTTP response is always sent" `[§Subscription Request, p.31]`.

Spec examples, verbatim:

```
REQERR,19,Specified subscription not found
```
`[§Other Control Responses, p.45]`

```
ERROR,68,Internal error during request processing
```
`[§Other Control Responses, p.45]`

### 3.4 Message-send failures — `MSGFAIL`

`MSGFAIL` is "used when: to notify that a message has not been sent or its processing failed" `[§Message Send Failed, p.60]`, and is "sent when the corresponding message has not been received by the Server or its processing failed for some reason" `[§Message Send Failed, p.61]`. Its `<error-code>` is drawn from **Appendix B** `[§Message Send Failed, p.60]`.

Appendix B codes whose descriptions name a message context:

| Code | Description | Page |
| --- | --- | --- |
| `32` | Progressive number too low: already enqueued **or** already skipped by timeout (the exact case cannot be determined) | p.92 |
| `33` | Progressive number too low: a message with this number has already been enqueued (and possibly processed) | p.92 |
| `34` | Unexpected Server error while managing the message (e.g. a `NotificationException` thrown by the Metadata Adapter) | p.92 |
| `35` | Unchecked exception thrown in the Metadata Adapter while managing the message | p.92 |
| `38` | The specified progressive number has been skipped by timeout | p.92 |
| `39` | The specified progressive number **and some consecutive preceding numbers** have been skipped by timeout; the notification pertains to all those progressive numbers | p.92 |
| `≤ 0` | Supplied by the Metadata Adapter, application-specific; "This can affect subscription requests and message processing." | p.94 |

`MSGFAIL` is suppressed together with `MSGDONE` when `LS_outcome` is `false`: "If `false` the `MSGDONE` or `MSGFAIL` notification is not sent" `[§Message Send Request, p.44]`. With both `LS_ack` and `LS_outcome` set to `false` "there will be no notification at all for a successful message submission (a 'fire and forget' policy)" `[§Message Send Request, p.44]` — note the spec scopes that sentence to the *successful* case.

One further error condition is stated in prose without a code: the sequence identifier `UNORDERED_MESSAGES`, "used in previous text protocols, is now reserved and can't be used. If specified leads to an error" `[§Message Send Request, p.42]`. ⚠️ **Spec unclear:** the spec does not say which code is returned for a reserved-sequence-name violation, nor whether it arrives as `REQERR`/`ERROR` or as `MSGFAIL`.

Spec example, verbatim:

```
MSGFAIL,Orders_Sequence,4,38,The specified progressive number has been skipped by timeout
```
`[§Message Send Failed, p.61]`

### 3.5 Heartbeat failures — `ERROR`

The heartbeat "is not meant to receive a response" `[§Client Heartbeat, p.46]`, and "the response is returned only with HTTP transport. With WS transport there is no response, but for the case an of error in the formulation of the request" (verbatim, including the spec's typo) `[§Client Heartbeat, p.46]`.

| Tag | Format | Used when | Code source | Page |
| --- | --- | --- | --- | --- |
| `ERROR` | `ERROR,<error-code>,<error-message>` | "the request could not be interpreted or processed. **Note that there is no error if the specified session is not found.**" | Appendix B | p.47 |

The highlighted sentence is the only per-context carve-out in the document: an unknown `LS_session` on a heartbeat is explicitly *not* an error `[§Client Heartbeat → Other Responses, p.47]`. This differs from control requests, where Appendix B code `20` covers "Specified (or implied) session not found" `[§Appendix B, p.91]`.

Spec example, verbatim:

```
ERROR,68,Internal error during request processing
```
`[§Client Heartbeat → Other Responses, p.47]`

### 3.6 WebSocket establishment check — no error responses

| Request | Successful response | Error responses | Page |
| --- | --- | --- | --- |
| `wsok` | `WSOK` (no arguments) | "There are no error responses." | pp.47–48 |

The `wsok` pseudo-request takes no parameters at all — "not only does the request require no parameters, but also the parameter line, needed with other types of requests (and possibly empty), is missing in this case" `[§WebSocket establishment check, p.47]` — and involves no notifications `[§WebSocket establishment check, p.47]`. Its *Other Responses* subsection consists solely of the sentence "There are no error responses." `[§WebSocket establishment check → Other Responses, p.48]`.

⚠️ **Spec unclear:** the spec states that the `wsok` check exists "to ensure that the communication channel is working with no interference from intermediate nodes" `[§WebSocket establishment check, p.47]`, but since there is no error response, it does not say how a *failed* check manifests (absence of `WSOK`, a timeout, an altered echo, or a transport-level close). No timeout value is given.

### 3.7 MPN-specific codes

All MPN codes live in Appendix B and therefore travel on `REQERR` / `ERROR` (MPN requests are a subset of control requests `[§MPN Control Requests, p.36]`).

| Code | Description | Page |
| --- | --- | --- |
| `40` | MPN Module disabled, by configuration or by license restrictions | p.92 |
| `41` | MPN request failed because of some internal resource error (e.g. database connection, timeout etc.) | p.92 |
| `42` | Specified MPN platform invalid or not supported | p.93 |
| `43` | Specified MPN application ID invalid or unknown | p.93 |
| `44` | Syntax of the specified MPN trigger expression invalid | p.93 |
| `45` | Specified MPN device ID invalid or unknown | p.93 |
| `46` | Specified MPN subscription ID invalid or unknown | p.93 |
| `47` | Specified MPN trigger expression or notification format contains an invalid argument name | p.93 |
| `48` | MPN device corresponding to the specified ID has been suspended | p.93 |
| `49` | One or more arguments of the MPN request exceed its maximum size | p.93 |
| `50` | Specified MPN subscription contains no items or no fields | p.93 |
| `51` | Payload of the MPN subscription exceeds the maximum size for the platform | p.93 |
| `52` | Specified MPN notification format is not a valid JSON structure | p.93 |
| `53` | Specified MPN notification format is empty | p.93 |
| `56` | The subscription parameters specified don't match with the specified MPN subscription | p.93 |

Note the collision: `48` means "The MPN device […] has been suspended" in Appendix B `[p.93]` but "The maximum session duration has been reached" in Appendix A `[p.89]`. Likewise `40` means "The MPN Module is disabled" in Appendix B `[p.92]` versus "A manual rebind to this session has been performed by some client" in Appendix A `[p.89]`. **The catalog is selected by the carrying tag, not by the number.**

The MPN-related notifications (`MPNREG`, `MPNZERO`, `MPNOK`, `MPNCONF`, `MPNDEL`) carry **no** error codes; they are success notifications only `[§MPN-Related Notifications, pp.57–59]`. See [06-mpn.md](06-mpn.md).

### 3.8 Metadata-Adapter-supplied codes (`≤ 0`)

Both catalogs terminate with the same range entry, worded slightly differently:

| Catalog | Wording | Page |
| --- | --- | --- |
| A | "If the code is `0` or negative, it has been supplied by the **Metadata Adapter** and the interpretation is application-specific. In particular if used with and `END` notification, means the session was closed by an Administrator with a *destroy* command and the code was supplied as a custom cause code." | p.90 |
| B | "If the code is `0` or negative, it has been supplied by the **Metadata Adapter** and the interpretation is application-specific. This can affect subscription requests and message processing." | p.94 |

Corroborating statements elsewhere: `CONERR`'s `<error-code>` is "sent by the Server kernel **or by the Metadata Adapter**" `[§Other Session Creation or Binding Responses, p.28]`; the same for `REQERR` `[§Other Control Responses, p.45]` and `MSGFAIL` `[§Message Send Failed, p.60]`. By contrast, `ERROR`'s `<error-code>` is described as "sent by the Server kernel" only, with no mention of the Metadata Adapter `[§Other Control Responses, p.45]`, `[§Client Heartbeat → Other Responses, p.47]`; and `END`'s `<cause-code>` is likewise "sent by the Server kernel" `[§Other Session Creation or Binding Responses, p.28]`, `[§Stream Connection End, p.65]`.

⚠️ **Spec unclear:** the `≤ 0` entry in Appendix A says a negative code may arrive "with and `END` notification" as a custom cause code, but the `END` argument description attributes `<cause-code>` to "the Server kernel" alone `[§Stream Connection End, p.65]`. The two statements are not reconciled in the document.

⚠️ **Spec unclear:** the spec does not enumerate the application-specific `≤ 0` codes and cannot — they are defined by whatever Metadata Adapter is deployed. It also gives no lower bound on the value. Any client must treat the whole non-positive integer range as opaque, application-defined values.

---

## 4. Error-response shapes

Every error arrives as one line of the common response/notification syntax `<tag>,<argument1>,...,<argumentN>` `[§Common Response and Notification Syntax, p.13]`, where the tag is "a short (up to 8 characters) uppercase string" and "each different tag has a fixed number of arguments" `[§Common Response and Notification Syntax, p.13]`. The line terminator is CR-LF, the character set is UTF-8 for both encoded and unencoded content, and arguments containing meta characters such as the comma are percent-encoded `[§Common Response and Notification Syntax, p.13]`.

Parsing consequence for error lines: the **last argument may contain additional commas** and is not split further — "Split the remaining part of the string comma by comma, from left to right, to obtain each argument. Note that the last argument may contain additional commas" `[§Common Response and Notification Syntax, p.14]`. Since in every error shape the last argument is the human-readable message, the message must be taken as the remainder of the line after the fixed number of leading commas. Non-numeric arguments are percent-decoded (the exception — the last argument of a `U` real-time update — does not apply to any error shape) `[§Common Response and Notification Syntax, p.14]`.

### 4.1 The five shapes

| # | Shape | Field positions (after the tag) | Where the code sits | Where the message sits | Declared arg count | Page |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | `CONERR,<error-code>,<error-message>` | 1 = code, 2 = message | position 1 | position 2 (last; may contain commas) | 2 | p.28 |
| 2 | `END,<cause-code>,<cause-message>` | 1 = cause code, 2 = cause message | position 1 | position 2 (last) | spec prints **1** — see ⚠️ below | p.28, p.65 |
| 3 | `REQERR,<request-ID>,<error-code>,<error-message>` | 1 = request ID, 2 = code, 3 = message | position **2** | position 3 (last) | 3 | p.45 |
| 4 | `ERROR,<error-code>,<error-message>` | 1 = code, 2 = message | position 1 | position 2 (last) | 2 | p.45, p.47 |
| 5 | `MSGFAIL,<sequence>,<prog>,<error-code>,<error-message>` | 1 = sequence, 2 = prog, 3 = code, 4 = message | position **3** | position 4 (last) | 4 | p.60 |

`REQERR` is the **only** error shape that carries the request ID `[§Other Control Responses, p.45]`; the general rule is that responses including errors are identified by the corresponding request ID "unless the request syntax was so misleading that a request ID could not be parsed" `[§Requests, Responses, and Notifications, p.10]`, which is precisely when `ERROR` is used instead.

⚠️ **Spec unclear (`END` argument count):** both descriptions of `END` print "**Arguments: 1**" and then proceed to document **two** arguments, `<cause-code>` and `<cause-message>` `[§Other Session Creation or Binding Responses, p.28]`, `[§Stream Connection End, p.65]`. The format line and every worked example (`END,8,Session count limit reached`; `END,31,Destroy invoked by client`) show two arguments, so the count "1" appears to be an error in the document, but the spec is self-contradictory here and this note records it rather than resolving it.

### 4.2 Per-field notes

**The code field.**

- `CONERR`: "Error code sent by the Server kernel or by the Metadata Adapter. For a list of supported error codes see Appendix A." `[§Other Session Creation or Binding Responses, p.28]`
- `END`: "Cause code sent by the Server kernel. For a list of supported cause codes see Appendix A." `[§Other Session Creation or Binding Responses, p.28]`, `[§Stream Connection End, p.65]`
- `REQERR`: "Error code sent by the Server kernel or by the Metadata Adapter. For a list of supported error codes see Appendix B." `[§Other Control Responses, p.45]`
- `ERROR`: "Error code sent by the Server kernel. For a list of supported error codes see Appendix B." `[§Other Control Responses, p.45]`, `[§Client Heartbeat → Other Responses, p.47]`
- `MSGFAIL`: "Error code sent by the Server kernel or by the Metadata Adapter. For a list of supported error codes see Appendix B." `[§Message Send Failed, p.60]`

**The message field.**

- `CONERR`: "Error message sent by Server kernel or by the Metadata Adapter. **Note that a message sent by the Metadata Adapter may be empty.**" `[§Other Session Creation or Binding Responses, p.28]`
- `REQERR`: same wording, including the may-be-empty note `[§Other Control Responses, p.45]`
- `MSGFAIL`: same wording, including the may-be-empty note `[§Message Send Failed, p.60]`
- `ERROR`: "Error message sent by Server kernel." — **no** may-be-empty note `[§Other Control Responses, p.45]`, `[§Client Heartbeat → Other Responses, p.47]`
- `END`: "Short **cause description** sent by the Server kernel." `[§Other Session Creation or Binding Responses, p.28]`, `[§Stream Connection End, p.65]`

The general rule that "an argument may be empty, although most arguments, because of their meaning in the context of the response, will never be empty" `[§Common Response and Notification Syntax, p.13]` applies to all of them.

**`MSGFAIL`'s leading fields** `[§Message Send Failed, p.60]`:

- `<sequence>`: "Sequence identifier specified in the send request, or `*`, if a sequence had not been specified."
- `<prog>`: "Message progressive number specified in the send request."

### 4.3 Where each shape can appear

| Shape | HTTP transport | WS transport | Notes | Page |
| --- | --- | --- | --- | --- |
| `CONERR` | Body of the HTTP response to `create_session` / `bind_session` | WS text message | Terminates the session creation/binding attempt | p.28 |
| `END` (as a binding response) | Response to the binding request | Response to the binding request | Only with session rebinding | p.28 |
| `END` (as a notification) | On the stream connection | On the stream connection | "since this notification marks the end of the stream connection, with HTTP transport, any spurious character following the CR-LF of this notification may be safely ignored" | p.65 |
| `REQERR` / `ERROR` (control) | On the separate connection the control request was sent to | On the stream connection | "Responses (including errors) are always sent on the same connection the request was sent to" | p.10, p.45 |
| `ERROR` (heartbeat) | On the HTTP response | Only in case of an error in the formulation of the request | Session-not-found is not an error here | pp.46–47 |
| `MSGFAIL` | On the stream connection | On the stream connection | Suppressed by `LS_outcome=false` | p.44, p.60 |

Ordering caveat that applies to reading errors: "The requests are handled in parallel and independently of one another […] As a consequence, the responses to multiple requests can come in any order." The spec's own illustration is that an unsubscription immediately following its subscription may be processed first and "will fail […] The failed unsubscription request will be notified with a proper error response" `[§Requests, Responses, and Notifications, p.10]`.

---

## 5. What the spec says about recovery

The spec makes no per-code recoverability claim beyond those already tabulated. The general statements are:

### 5.1 After a failed session recovery

"The session recovery attempt may fail. In this case, the Client can't but create a new stream connection as if it was the first time it connects." `[§Session Life Cycle, p.7]` The corresponding code is `4` — "the recovery attempt failed" `[§Appendix A, p.88]`.

### 5.2 After a failed rebind

"In this situation, the client should create a new stream connection, as if it was the first time it connects to the Server, and re-execute all the subscriptions that were active at the moment the previous stream connection terminated." `[§HTTP Loop and Failed Rebind, p.19]` The spec lists the reasons a rebind may fail: the Server deleted the old session after too much time; the Server was shut down and a brand new instance now responds on the same port; a cluster of Servers behind a load balancer routed the new connection to a Server that does not recognise the old session `[§HTTP Loop and Failed Rebind, p.19]`.

### 5.3 The permanent (unrecoverable) clustering case

"By the way, in the last error case described above (i.e. a cluster of Servers with incorrect affinity), the issue is permanent and will not be overcome by a reconnection but only by fixing the configuration. The Server tries to recognize this case and use a different error code." `[§HTTP Loop and Failed Rebind, p.20]`

⚠️ **Spec unclear:** the spec says the Server "tries to recognize this case and use a different error code" but does not name that code in this passage `[§HTTP Loop and Failed Rebind, p.20]`. Appendix A code `21` and Appendix B code `11` both describe a session ID "not compatible with this Server instance […] it might pertain to a different instance" `[§Appendix A, p.89]`, `[§Appendix B, p.91]`, which matches the description, but the document never states the linkage. Do not assume it.

### 5.4 Session refresh

Code `48` (Appendix A) is the one case where the spec prescribes the client action outright: the maximum session duration "is only meant as a way to refresh the session (for instance, to force a different association in a clustering scenario), hence the client should recover by opening a new session immediately" `[§Appendix A, p.89]`.

### 5.5 Retry-later conditions

Codes `5` and `6` (Appendix A) both end with "retry later" `[§Appendix A, p.88]`; code `10` is "New sessions temporarily blocked" `[§Appendix A, p.88]`. No timing guidance is given (see the ⚠️ in §2).

### 5.6 Atomicity of the combo request

For a session creation and control combo request, "the whole operation is considered atomic: if the control request fails, the session creation also fails" `[§Session Creation and Control Combo Request, p.66]`. The failure surfaces as `CONERR` with code `64` — "Generic error in the control request part of a session/control combo request" `[§Appendix A, p.89]` — and not as a `REQERR`, because in a combo request "the `LS_reqId` parameter is also omitted. The control request, in fact, has no specific response" `[§Session Creation and Control Combo Request, p.66]`.

---

## 6. Collected ambiguities

Every `⚠️ Spec unclear:` raised in this chapter, in order of appearance:

1. **§1** — Both appendix preambles use "such as"; the set of tags that may carry codes from each catalog is not closed, and Appendix B's preamble omits `MSGFAIL` despite `[p.60]` routing `MSGFAIL` to it.
2. **§1** — No per-code → per-tag mapping exists anywhere in the document; catalog membership is the only mapping.
3. **§2** — Code `39`: the identifier `err_code` is undefined in TLCP 2.5.0, and the field carrying the count of skipped progressives is therefore unidentified `[p.92]`.
4. **§2** — Codes `33`, `34` share one description; nothing distinguishes them `[p.89]`.
5. **§2** — "retry later" / "temporarily blocked" codes (`5`, `6`, `10`) come with no retry interval, backoff, or retry bound `[p.88]`.
6. **§2** — Both catalogs have numbering gaps; unlisted positive codes are neither reserved nor given a fallback handling rule.
7. **§3.4** — The reserved sequence name `UNORDERED_MESSAGES` "leads to an error" `[p.42]`, but neither the code nor the carrying tag is stated.
8. **§3.6** — `wsok` has no error response; how a failed WebSocket-establishment check manifests, and within what timeout, is not stated `[pp.47–48]`.
9. **§3.8** — Appendix A's `≤ 0` entry attributes negative `END` cause codes to the Metadata Adapter `[p.90]`, while the `END` argument description attributes `<cause-code>` to the Server kernel alone `[p.65]`.
10. **§3.8** — The `≤ 0` application-specific codes are, by construction, not enumerable, and no lower bound is given.
11. **§4.1** — `END` is documented as "Arguments: 1" while two arguments are listed and every example shows two `[p.28]`, `[p.65]`.
12. **§5.3** — The "different error code" the Server uses for the permanent clustering-affinity failure is never named `[p.20]`.
