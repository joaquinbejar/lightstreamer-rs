# TLCP Foundations — Concepts, Transports, and Wire Syntax

Scope: this chapter distills chapter 1 (*Introduction*) of the **TLCP Specification, version 2.5.0** (Text Lightstreamer Client Protocol; target: Lightstreamer Server v. 7.4.0 or greater; last updated 19/3/2024), covering printed pages 5–14, with the exception of the *Session Life Cycle* and *Session Recovery* subsections (printed pages 6–8), which are covered elsewhere and are only referenced here. Every factual statement below carries a citation of the form `[§Section Name, p.NN]`, where `NN` is the **printed** page number of the specification. Wire tokens are reproduced verbatim in backticks; spec examples are quoted verbatim in fenced blocks.

---

## 1. Protocol Versions

`[§Protocol Versions, p.5]`

TLCP is versioned in the form:

```
major.minor.subminor
```

with the example `2.1.0` given by the spec `[§Protocol Versions, p.5]`.

Compatibility rules:

- If only the **subminor** version number changes, no upgrade to the Lightstreamer Server is required to support the new version of TLCP `[§Protocol Versions, p.5]`.
- Any new version of Lightstreamer Server is guaranteed to work with any previous version of TLCP `[§Protocol Versions, p.5]`.

The spec's worked example, verbatim `[§Protocol Versions, p.5]`:

> - Lightstreamer Server v. 6.1 is released, with support for TLCP v. 2.0.0.
> - Later on, Lightstreamer Server v. 6.2 is released, which, among other things, introduces explicit support for TLCP v. 2.0.3.
> - A client is developed against TLCP v. 2.0.3. Such client is guaranteed to work even if it connects to Lightstreamer Server v. 6.1, because that server supports TLCP v. 2.0.x.

This document describes version **2.5.0** of the protocol `[§1 Introduction, p.5]`. The version string appears on the wire as `TLCP-2.5.0` (HTTP query-string parameter `LS_protocol`) and as `TLCP-2.5.0.lightstreamer.com` (WebSocket subprotocol) — see [§5, Request Syntax](#5-request-syntax).

⚠️ **Spec unclear:** the *Protocol Versions* section states the compatibility rule for a changing **subminor**, and states forward compatibility of newer Servers with older protocol versions, but does not state what a client may assume when the **minor** number differs (e.g. a client built for `TLCP-2.5.0` talking to a Server that only supports `TLCP-2.1.0`), nor how such a mismatch is signalled on the wire `[§Protocol Versions, p.5]`.

---

## 2. Main Concepts

`[§Main Concepts, pp.5–6]` — the following is the spec's own enumeration.

| Concept | Definition (per spec) |
|---|---|
| **session** | Communication between client and Server always operates inside the context of a session `[p.5]`. |
| **session ID** | Each session has a specific session ID `[p.5]`. |
| **stream connection** | A session must be bound to a network connection in order to receive downstream **real-time notifications**, and in particular **real-time updates**. The connection to which a session is bound is called the *stream connection* of that session `[p.5]`. |
| **session creation request** | Provides both a session and its initial stream connection `[p.5]`. |
| **session binding request** | Binds an existing session to a different stream connection `[p.5]`. |
| **items** and **fields** | Real-time updates are organized into items and fields `[p.5]`. |
| **control requests** | Change the set of items and fields for which real-time updates are received `[p.5]`. Depending on the transport, they may be sent on separate network connections or on the session stream connection `[p.6]`. |
| **subscription** | The control request that adds new items to the set being received `[p.6]`. |
| **unsubscription** | The opposite control request, that removes items and fields from the set `[p.6]`. |
| **messages** | A specific subset of control requests is dedicated to sending **upstream** messages `[p.6]`. |

The spec states as a prerequisite that a general understanding of the main Lightstreamer concepts — **Adapter Set**, **Metadata Adapter**, **Data Adapter** — is required, and points to the *General Concepts* document for them `[§1 Introduction, p.5]`.

⚠️ **Spec unclear:** chapter 1 names *Adapter Set*, *Metadata Adapter* and *Data Adapter* as prerequisite knowledge but does not define them; it explicitly defers their definition to the external *General Concepts* document `[§1 Introduction, p.5]`. Their protocol-visible roles that *are* stated in chapter 1 are limited to: the Metadata Adapter maps group names to item sets and schema names to field sets, and defines item and field ordering `[§Subscription Data Model, pp.9–10]`; and the Data Adapter is the (optional) named implementer of the subscribed group `[§Subscription Data Model, p.9]`.

The session state machine, binding/unbinding, buffering while unbound, long polling, and session recovery are specified in *Session Life Cycle* `[pp.6–8]` and *Session Recovery* `[p.8]` — **not** reproduced here.

---

## 3. Transports

`[§Transports, p.6]`

TLCP supports two transports `[§Transports, p.6]`:

| Transport | Spec wording | Stream connection | Control requests |
|---|---|---|---|
| **HTTP** (and HTTPS) | "HTTP (and HTTPS)" | An HTTP request whose response is of (almost) **infinite length** | HTTP is **half-duplex**, so each control request requires a **separate connection**, parallel to the main stream connection |
| **WebSocket** / **WS** (and Secure WebSocket / **WSS**) | "WebSocket, or WS for short (and Secure WebSocket, or WSS for short)" | A simple WS connection | WS is **full-duplex**, so control requests may be sent **directly on the stream connection**, eliminating the connection overhead of HTTP |

All rows above: `[§Transports, p.6]`.

Edition note, verbatim `[§Transports, p.6]`:

> Edition Note: WSS/HTTPS is an optional feature, available depending on Edition and License Type. To know what features are enabled by your license, please see the License tab of the Monitoring Dashboard (by default, available at /dashboard).

Further rules `[§Transports, p.6]`:

- The protocol is designed to minimize differences between the two transports, while still leveraging transport-specific advantages.
- **Transports may be mixed**: as far as the correct session ID is specified in each control request, a stream connection on WS may be controlled by control requests on HTTP and vice-versa.
- Mixing of transports is exploited by official Lightstreamer client libraries to implement the **Stream-Sense** algorithm, which automatically discovers the best transport to use at any given moment.

Additional WS-specific constraint `[§Request Syntax → WS Transport, p.13]`:

- As long as a session is bound to a WS, **no other session can be bound**. After unbinding of a session, the same or a different session can be bound.
- After binding the second session, it is possible that **late responses** to control requests related with the first session arrive interspersed with notifications for the second session.

⚠️ **Spec unclear:** chapter 1 states that transports may be mixed provided the correct session ID is specified in each control request `[§Transports, p.6]`, but does not state, within the pages covered here, to which host/URL an out-of-band HTTP control request must be directed for a session whose stream connection lives elsewhere. (A `<control-link>` argument exists in the `CONOK` response signature quoted on p.13, but chapter 1 does not define its semantics.)

---

## 4. Subscription Data Model

`[§Subscription Data Model, pp.9–10]`

### 4.1 What a subscription request carries

| Element | Required? | Value | Meaning |
|---|---|---|---|
| subscription ID | yes | client-generated **progressive integer number starting with 1** | Must be **unique within the session** `[p.9]` |
| group name | yes | a single variable-length symbolic name | Represents the **set of items** to subscribe to `[p.9]` |
| schema name | yes | a single variable-length symbolic name | Represents the **set of fields** to subscribe to `[p.9]` |
| subscription mode | yes | see [§4.2](#42-subscription-modes) | Tells the Server how to consider real-time updates for this set of items `[p.9]` |
| Data Adapter name | **optional** | name | The name of the Data Adapter that implements the subscribed group `[p.9]` |

Group name rules `[§Subscription Data Model, p.9]`:

- The mapping between the group name and the corresponding item set is implemented in the **Metadata Adapter**.
- A typical example of group is the name of a stock list, e.g. `NASDAQ-100`, which the Metadata Adapter maps to the list of 100 items composing the NASDAQ 100 list.
- Nothing forbids using a **second-level syntax** to express a literal list of items, e.g. `AAPL MSFT GOOGL`. It's up to the Metadata Adapter to interpret the group name.
- The **same item may be included in multiple groups**.

Schema name rules `[§Subscription Data Model, p.9]`:

- As with the group, the mapping between the schema name and the corresponding field set is implemented in the **Metadata Adapter**.
- As with the group, nothing forbids using a second-level syntax to express a literal list of fields, e.g. `NAME LAST OPEN MIN MAX`.

### 4.2 Subscription modes

`[§Subscription Data Model, p.9]`

| Mode | Table shape | Effect of each real-time update | Common example |
|---|---|---|---|
| `MERGE` | a **fixed table** where items are rows and fields are columns | **changes the content of a specific row** | a stock quote table |
| `COMMAND` | a **dynamic table**, where items are rows and fields are columns | may **add a new row**, **delete an existing row** or **change the content of a specific row** | a stock portfolio |
| `DISTINCT` | a **set of growing tables**, where each item is a separate table and fields are columns | **appends a new row** at the end of a specific table | a news feed, with a different news topic for each item |

The spec defers an in-depth discussion of subscription modes to the *General Concepts* document `[§Subscription Data Model, p.9]`.

⚠️ **Spec unclear:** *Subscription Data Model* `[p.9]` enumerates exactly three modes (`MERGE`, `COMMAND`, `DISTINCT`), whereas the `LS_mode` parameter description states "Possible values are: RAW, MERGE, DISTINCT and COMMAND" `[§Control Requests → Subscription Request, p.30]`. The chapter under description here does not define `RAW` or its data model.

### 4.3 What a real-time update carries

Once a subscription is active, real-time updates include `[§Subscription Data Model, pp.9–10]`:

- The **subscription ID** `[p.9]`.
- The **progressive number of the item** updated. Item numbers are progressive integers starting with 1 `[p.10]`.
  - The order of items of a certain group is defined by the **Metadata Adapter** `[p.10]`.
  - The same item may appear in different groups with a **different number**. The client is supposed to know *a priori* this ordering `[p.10]`.
- A list with **values of updated fields** `[p.10]`.
  - The order of fields of a certain schema is defined by the **Metadata Adapter** `[p.10]`.

### 4.4 What an unsubscription request carries

Just the **subscription ID** `[§Subscription Data Model, p.10]`.

---

## 5. Requests, Responses, and Notifications

`[§Requests, Responses, and Notifications, p.10]` — general rules on request and response handling.

### 5.1 Request IDs

- Control requests are always identified by a **client-generated request ID**. It may be **any combination of letters and numbers**, but typically a progressive number will do, and must be **unique within the connection** `[p.10]`.
- It needs to be unique only within a reasonable amount of time, e.g. a few minutes. The Server will never respond to a request with a delay of several minutes. If a progressive integer is used, the counter may be periodically reset to avoid sending long integer strings `[p.10]`.

⚠️ **Spec unclear:** the request-ID uniqueness scope is stated as "unique within the connection" `[p.10]`, but with HTTP transport each control request is sent on its own separate connection `[§Transports, p.6]`, which makes that scope trivially satisfiable; the spec does not restate the scope in per-transport terms, nor reconcile "within the connection" with the "reasonable amount of time" qualifier.

### 5.2 Response routing and identification

- Responses (including errors) are always identified by the **corresponding request ID**, *unless the request syntax was so misleading that a request ID could not be parsed* `[p.10]`.
- Responses (including errors) are always sent on the **same connection** the request was sent to `[p.10]`:
  - If the control request was sent on a **separate connection**, such as with HTTP transport, the response is sent on that same connection `[p.10]`.
  - If the control request was sent on the **stream connection**, such as with WS transport, the response is sent on the stream connection `[p.10]`.

⚠️ **Spec unclear:** the spec states that when a request ID cannot be parsed the response is not identified by one `[p.10]`, but does not, within these pages, specify the shape of such an unidentified response (which tag is used, or which argument position replaces the request ID).

### 5.3 Concurrency and ordering

- Requests are handled **in parallel and independently of one another**, also when the transport supports a sequence of requests (as in the WebSocket case). As a consequence, **the responses to multiple requests can come in any order** `[p.10]`.
- The spec's own illustration, verbatim `[p.10]`:

> For instance, if an unsubscription request immediately follows the related subscription request, it is possible that the unsubscription request will be processed first. In this case, it will fail and only the subscription will take place. The failed unsubscription request will be notified with a proper error response.

- There are only a few exceptions to the parallel processing of subsequent requests, that are specified within the individual request descriptions `[p.10]`.
- The same policy applies to **batched** requests: requests of the same batch are executed concurrently and responses may arrive out of order `[§Request Syntax → HTTP Transport, p.11]` `[§Request Syntax → WS Transport, p.13]`.
- With WS, responses may always arrive **interspersed with notifications**, when control requests are sent on the stream connection `[§Request Syntax → WS Transport, p.13]`.

### 5.4 Notifications and protocol-breaking requests

- If the request was successful, a **notification** may be sent on the **stream connection** (if appropriate for the request) `[p.10]`.
- Requests so wrong as to break the protocol (or involved in severe processing issues) may cause the **interruption of the connection** they were sent to `[p.10]`.
- Such interruption may either be **abrupt** or provide an error code leveraging the transport syntax (e.g. HTTP Response Status Code or WebSocket Close Status Code). However, **the interruption details are not in the scope of the TLCP protocol** `[p.10]`.

---

## 6. Request Syntax

`[§Request Syntax, p.11]`

> Requests follow general rules for query string composition in an HTTP URL, with minor differences for the HTTP and WS cases. `[p.11]`

### 6.1 HTTP Transport

`[§Request Syntax → HTTP Transport, pp.11–12]`

#### 6.1.1 Single request — general form

Verbatim from the spec `[p.11]`:

```
POST /lightstreamer/<request-name>.txt?LS_protocol=TLCP-2.5.0 <HTTP-version>
<HTTP-request-headers>
<empty-line>
<param1>=<value1>&...&<paramN>=<valueN>
```

Where `[p.11]`:

| Element | Required? | Value | Meaning |
|---|---|---|---|
| `<request-name>` | yes | e.g. `create_session`, `control` | Determines the kind of request; `create_session` for session creation request, `control` for control requests, etc. |
| `.txt` extension | yes (HTTP) | literal | Part of the path in the HTTP case; explicitly **missing** in the WS case `[p.12]` |
| `LS_protocol` | **required** | `TLCP-2.5.0` | "a required parameter to specify the request protocol" |
| `<param>=<value>` pairs | per request | e.g. `LS_reqId=123`, `LS_session=S1aa6c792585db57aT1726545` | Set the parameters of the request; `LS_reqId` specifies the request ID, `LS_session` specifies the session ID |
| `<HTTP-version>`, `<HTTP-request-headers>`, `<empty-line>` | yes | as per HTTP 1.x | Part of the HTTP 1.x request specifications as usual; see *Appendix C: HTTP Request Headers* `[p.95]` for accepted HTTP headers |

- The **line separator is CR-LF**, as per HTTP 1.x specifications `[p.11]`.
- The response will be received as the content of the **HTTP response body** `[p.12]`.

#### 6.1.2 Batched control requests

> In the specific case of **control requests**, multiple requests with the same request name may be sent in a single **batch** with the following syntax `[p.11]`:

```
POST /lightstreamer/<req-name>.txt?LS_protocol=TLCP-2.5.0[&LS_session=<session-ID>] <HTTP-version>
<HTTP-request-headers>
<empty-line>
<param1>=<value1>&...&<paramN>=<valueN>
...
<param1>=<value1>&...&<paramN>=<valueN>
```

Where `[p.11]`:

- The `<LS_session>=<session-ID>` pair in the **query string**, if present, acts as a **default value** for subsequent requests specified in the body; e.g. `LS_session=S1aa6c792585db57aT1726545` if all requests act on session ID `S1aa6c792585db57aT1726545`.
- Each `<param1>=<value1>&...&<paramN>=<valueN>` line in the request body specifies a **separate control request**.
- The **line separator is CR-LF, also within the request body**. The separator is **optional after the last line, unless the line is empty** (which is possible, in principle).

⚠️ **Spec unclear:** the batch form is introduced as applying to "multiple requests **with the same request name**" `[p.11]`, but the spec does not state what happens if the body lines would semantically belong to different request names, nor whether a batch of size 1 differs in any way from a single request.

⚠️ **Spec unclear:** the rule "the separator is optional after the last line, unless the line is empty (which is possible, in principle)" `[p.11]` implies a control request may be encoded as an empty line, but the spec does not describe what an empty request line means or when it legitimately occurs.

#### 6.1.3 Charset and percent-encoding (HTTP)

- The request **body** can be expressed in any charset supported by the Java installation which runs the Server, but **UTF-8 is recommended** and is guaranteed to be supported `[p.11]`.
- Reserved characters in the **values of the parameters specified in the body must be percent-encoded** `[p.12]`. Such reserved characters are **limited to** `[p.12]`:

| Character(s) | Reason given by the spec |
|---|---|
| Ascii **CR** and **LF** | used for line separation |
| `&` and `=` | used for parameter separation |
| `%` and `+` | used for percent-encoding / URL Encoding |

- However, **any further percent-encoding is supported**, provided that it is based on the **UTF-8** character set (*regardless of the charset used for the request body*). This allows use of existing encoding libraries which include the handling of the above characters, although this will introduce inefficiencies `[p.12]`.
- On the other hand, **the parameters on the request query string don't make use of any reserved character**. However, should a user-agent perform any extra percent-encoding on the request line, **it would be supported as well** `[p.12]`.

⚠️ **Spec unclear:** the reserved set is stated as *limited to* CR, LF, `&`, `=`, `%`, `+` `[p.12]`, whereas the chapter 2 preamble states "Reserved characters (as per section 2.2 of RFC 3896) in the value of a parameter must be percent-encoded" `[§2 Request/Response Reference, p.22]` — a strictly larger set, and citing an RFC number (`3896`) that does not correspond to the URI generic-syntax RFC. Which of the two statements is normative for an encoder is not resolved by the document.

⚠️ **Spec unclear:** `+` is listed among the reserved characters "used for percent-encoding / URL Encoding" `[p.12]`, but the spec never states whether the Server decodes a literal `+` in a parameter value as a space (`application/x-www-form-urlencoded` behaviour), nor whether a client may encode a space as `+`. Only the requirement to percent-encode `+` itself is stated.

⚠️ **Spec unclear:** the encoding rules are stated for the **values** of parameters `[p.12]`; the spec does not say whether parameter **names** may or must be percent-encoded, nor whether a decoder must percent-decode names before matching them.

⚠️ **Spec unclear:** the statement that query-string parameters "don't make use of any reserved character" `[p.12]` is asserted rather than constrained; the batch form does place `LS_session` on the query string `[p.11]`, and the special-case section permits **all** parameters on the query string when HTTP GET is used `[§Use of HTTP GET in Place of HTTP POST, p.68]`, in which case "the parameter values must be encoded as URI components, according to the requirements of the HTTP specifications" `[p.68]`. The two encoding regimes are not unified in the text.

⚠️ **Spec unclear:** the spec does not state which HTTP `Content-Type` a client must set on the request body; chapter 1 says only that the body may be in any Server-supported charset with UTF-8 recommended `[p.11]`, and defers headers to *Appendix C* `[p.95]`.

#### 6.1.4 HTTP method

All of chapter 1 specifies **POST** `[p.11]`. The Server also accepts **GET**, with parameters on the query string, subject to query-string length limits, credential exposure, and non-nullipotence; its use is **strongly discouraged** `[§Use of HTTP GET in Place of HTTP POST, p.68]`.

### 6.2 WS Transport

`[§Request Syntax → WS Transport, pp.12–13]`

#### 6.2.1 Connection establishment

Open a WebSocket to the Lightstreamer Server, using the URI and subprotocol below `[p.12]`:

| Item | Value |
|---|---|
| URI | `/lightstreamer` |
| `Sec-WebSocket-Protocol` | `TLCP-2.5.0.lightstreamer.com` |

#### 6.2.2 Single request — general form

Verbatim from the spec `[p.12]`:

```
<request-name>
<param1>=<value1>&...&<paramN>=<valueN>
```

Where `[p.12]`:

| Element | Required? | Value | Meaning |
|---|---|---|---|
| `<request-name>` | yes | e.g. `create_session`, `control` | Determines the kind of request; `create_session` for session creation request, `control` for control requests, etc. |
| `<param>=<value>` pairs | per request | e.g. `LS_reqId=123`, `LS_session=S1aa6c792585db57aT1726545` | Set the parameters of the request |

- As in the HTTP case, the **line separator is CR-LF** — note that **name and parameters are on separate lines**. The separator is **optional after the last line, unless the line is empty** (which is possible, in principle) `[p.12]`.
- **Each request must be sent in a single WS message** `[p.12]`.
- With WS the name extension (e.g. `.txt`) **is missing** `[p.12]`.

#### 6.2.3 Batched control requests

Verbatim from the spec `[p.12]`:

```
<request-name>
<param1>=<value1>&...&<paramN>=<valueN>
...
<param1>=<value1>&...&<paramN>=<valueN>
```

- Since WS is a full-duplex transport, the advantage of sending a batch of requests is not as evident as in the HTTP case, where the overhead of multiple HTTP connections is greatly reduced `[p.12]`.

#### 6.2.4 Charset and percent-encoding (WS)

- The WS messages used to carry requests must be of **text** (as opposed to binary) type, hence they must be based on the **UTF-8** character set `[p.13]`.
- Reserved characters in the values of the parameters specified in the message must be **percent-encoded**. Such reserved characters are **limited to** `[p.13]`:

| Character(s) | Reason given by the spec |
|---|---|
| Ascii **CR** and **LF** | used for line separation |
| `&` and `=` | used for parameter separation |
| `%` and `+` | used for percent-encoding / URL Encoding |

- However, any further percent-encoding is supported, provided that it is **also based on the UTF-8 character set** `[p.13]`.

#### 6.2.5 Responses on WS

The response will be received as a WS **text message**, or a **sequence** of them in case of a session creation / binding request `[p.13]`.

⚠️ **Spec unclear:** the WS request form `[p.12]` shows only `<request-name>` and the parameter line; unlike the HTTP form, it does not include `LS_protocol`, and the spec does not state whether `LS_protocol` may or must be sent as a parameter on WS given that the version is already carried by the `Sec-WebSocket-Protocol` subprotocol value.

⚠️ **Spec unclear:** for WS the spec does not state whether a single WS text message may carry more than one *non-control* request, nor whether the batch form's `LS_session` default-value mechanism described for the HTTP query string `[p.11]` has any WS equivalent (the WS batch form has no query string).

---

## 7. Common Response and Notification Syntax

`[§Common Response and Notification Syntax, pp.13–14]`

> All of requests use a simple syntax for their response, either failed or successful. The same base syntax is also adopted by notifications on the stream connection. `[p.13]`

### 7.1 Line format

Verbatim `[p.13]`:

```
<tag>,<argument1>,...,<argumentN>
```

| Element | Definition |
|---|---|
| `<tag>` | A **short (up to 8 characters) uppercase string** identifying the **kind** of response or notification `[p.13]` |
| `<argument1>,...,<argumentN>` | The arguments of the specific response or notification `[p.13]` |

- The **line terminator is CR-LF** `[p.13]`.
- The character set is **UTF-8**, for **both encoded and unencoded content** `[p.13]`.
- To simplify parsing, **each different tag has a fixed number of arguments**. Once the tag has been extracted by finding the first comma, the number of remaining commas is known, and thus how to extract the following arguments `[p.13]`.
- **An argument may be empty**, although most arguments, because of their meaning in the context of the response, will never be empty `[p.13]`.
- The **first 4 characters of a tag are unique** `[p.14, note *]`.

The spec also warns, for all syntax examples in the document `[§2 Request/Response Reference, p.22]`:

> Please note that in the above syntax examples and in all the examples that follow we don't make the CR-LF parts explicit and just use separate text lines. As a consequence, forging sample requests by starting from a copy-and-paste from our examples may not yield correct requests, depending on your editor end-of-line settings. Please always ensure the presence of the full CR-LF instead of only CR or only LF.

⚠️ **Spec unclear:** CR-LF is mandated as the terminator both for requests and for responses/notifications `[pp.11–13]`, but the spec does not state whether a receiver must **reject**, or may **tolerate**, a bare LF or bare CR; the warning above only tells the reader to emit full CR-LF.

⚠️ **Spec unclear:** the tag is described as "up to 8 characters" `[p.13]`, while the illustrative C code declares `char tag[8]` for a "zero-padded tag read from network" `[p.14]` — which leaves no room for a terminator at the maximum tag length. Whether 8 is an inclusive maximum for the tag itself is not restated elsewhere in these pages.

### 7.2 Escaping — first level

`[§Common Response and Notification Syntax, p.13]`

> If an argument contains meta characters, such as the comma, or some other special characters, they are **percent-encoded**, with the notable **exception of the last argument of a real-time update** (see the parsing algorithm below). The character set is **UTF-8**, for both encoded and unencoded content. `[p.13]`

Consequences for a decoder `[pp.13–14]`:

- Percent-encoding is the **only** escaping mechanism defined for responses and notifications in these pages; no backslash-based or other escape syntax is defined anywhere in chapter 1.
- The **last argument of a real-time update** (tag `U`) is exempt from first-pass percent-decoding and has its own second-level syntax — see [§7.5](#75-second-level-syntax-cross-reference-only).
- Because the last argument of a line **may contain additional commas**, splitting must stop after the tag's known argument count `[p.14]`.

⚠️ **Spec unclear:** the set of characters that a Server percent-encodes in a response/notification argument is given only by example — "meta characters, such as the comma, or some other special characters" `[p.13]` — and is never enumerated. An encoder-side implementation of the Server direction, or a strict validator, cannot be written from this text; a decoder is unaffected, since generic percent-decoding covers any set.

⚠️ **Spec unclear:** the first-level syntax provides **no distinction between a null argument and an empty argument**: the spec states only that "an argument may be empty" `[p.13]`. The `#` (null) / `$` (empty) distinction exists exclusively in the **second-level** field-value syntax of a real-time update `[§Decoding the Pipe-Separated List of Values, pp.49–50]`.

### 7.3 Example tag signatures

Verbatim `[pp.13–14]`:

```
CONOK,<session-ID>,<request-limit>,<keep-alive>,<control-link>

REQERR,<request-ID>,<error-code>,<error-message>

SUBOK,<subscription-ID>,<num-items>,<num-fields>

MSGFAIL,<sequence>,<prog>,<error-code>,<error-message>

END,<cause-code>,<cause-message>
```

These are given by the spec as *examples of tags and arguments* `[p.13]`; the complete tag reference is in chapters 2 and 3.

### 7.4 The normative parsing algorithm

Verbatim structure `[p.14]`:

1. Read the HTTP response or the WS message **line by line** (line terminator is **CR-LF**).
2. For each line, **trim the trailing line terminator** and look for the **first comma from left to right** (it may not be there).
3. The text until the first comma (**or the full line, if it's not there**) is the **tag**.
4. **Switch on the tag**:
   - Each tag has a fixed **number of arguments** (e.g. for a `REQERR` tag it's 3).
   - **Split** the remaining part of the string **comma by comma, from left to right**, to obtain each argument. Note that **the last argument may contain additional commas**.
   - **Percent-decode non-numeric arguments**, **except** for the last argument of a real-time update (tag `U`).
5. Interpret the response or notification accordingly.

Note `[*]` verbatim `[p.14]`:

> The **first 4 characters** of a tag are **unique**. If parsing with a programming language that does not support switching on string literals, the first four characters of a tag (which are all simple Ascii characters) may be easily mapped to a 32 bit integer and then switch on its corresponding numeral.

The spec's C illustration, verbatim `[p.14]`:

```c
#define STR_SWITCH(a,b,c,d) (((a & 0xff) << 24) | \
                             ((b & 0xff) << 16) | \
                             ((c & 0xff) << 8)  | \
                              (d & 0xff))

char tag[8] = /* Zero-padded tag read from network */;
unsigned int iTag= STR_SWITCH(tag[0], tag[1], tag[2], tag[3]);
switch (iTag) {
case STR_SWITCH('R','E','Q','O'):
        // Handle "REQOK"
        break;

case STR_SWITCH('R','E','Q','E'):
        // Handle "REQERR"
        break;

// ...
}
```

Note `[**]` verbatim `[p.14]`:

> The last argument of a **real-time update** is itself a variable-length set of fields, with its own **second-level syntax**. To avoid unnecessary encoding of frequently used characters, this argument follows specific encoding rules and should not be percent-decoded during the first pass.

⚠️ **Spec unclear:** step 4 says to "percent-decode **non-numeric** arguments" `[p.14]`, but the algorithm as written provides no way to know which argument positions of a given tag are numeric — that classification lives in the per-tag reference of chapters 2 and 3, not in the parsing algorithm. The spec also does not say whether percent-decoding a numeric argument would be harmful or merely unnecessary.

⚠️ **Spec unclear:** the algorithm says the tag is "the text until the first comma (**or the full line, if it's not there**)" `[p.14]`, i.e. zero-argument lines exist, but chapter 1 does not state whether an unrecognised tag must be ignored, must abort parsing, or must terminate the connection.

### 7.5 Second-level syntax (cross-reference only)

The exemption in §7.2/§7.4 concerns the **third argument of a real-time update** (tag `U`), which is a pipe-separated list of field values with its own encoding. That syntax is specified in *Real-Time Update → Decoding the Pipe-Separated List of Values* `[pp.49–50]`, i.e. **outside** the pages this chapter covers. Its encoding rules, quoted for completeness because they define the null/empty/unchanged representations that the first-level syntax deliberately does not provide `[§Decoding the Pipe-Separated List of Values, pp.49–50]`:

| Value on the wire | Meaning |
|---|---|
| *(empty)* | the field is **unchanged** compared to the previous update of the same field |
| `#` | the field is **null** |
| `$` | the field is **empty** |
| `^` followed by a **number**, e.g. `^3` | the following number of fields are **unchanged** |
| `^` followed by a **letter**, e.g. `^P` | the remaining part of the value is a **"diff"** against the previous value of that field; the letter is the tag of the diff format (`P` = JSON Patch, RFC 6902; `T` = TLCP-diff, Appendix D) |
| anything else | actual content |

- Meta characters, such as the pipe `|`, CR-LF, etc., are **percent-encoded** `[p.49]`.
- `#`, `$` and `^` characters are percent-encoded **if occurring at the beginning of an actual content (and only in this case)** `[p.50]`.
- Field separator is the pipe `|`; `#` is UTF-8 code `0x23`, `$` is `0x24`, `^` is `0x5E` `[pp.49–50]`.

The full second-level decoding algorithm, the diff formats, and the snapshot-vs-real-time classification belong to the *Real-Time Update* chapter and are not reproduced here.
