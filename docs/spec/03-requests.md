# Request/Response Reference

This chapter is the complete encoder reference for the client-to-server half of TLCP. It distils chapter 2 *Request/Response Reference* of the **Text Lightstreamer Client Protocol (TLCP) Specification, version 2.5.0**, printed pages 22-48, plus the general framing rules from *Request Syntax* on printed pages 11-13 that every request depends on. It documents `create_session`, `bind_session`, the six non-MPN `control` operations, `msg`, `heartbeat` and `wsok`, together with the exact success and failure responses for each. The **MPN Control Requests** of printed pages 36-42 (`LS_op` values `activate`, `deactivate`, `subscribe`, and the related device-registration operations) are deliberately excluded and are covered in a separate chapter of this reference. Notification syntax (`U`, `SUBOK`, `CONF`, `MSGDONE`, …) belongs to chapter 3 of the spec and is likewise covered elsewhere. Every statement below carries a citation to the printed page of the spec it comes from.

---

## 1. General request framing

Two transports carry TLCP requests, and the framing differs between them.

### 1.1 HTTP transport

General form [§Request Syntax → HTTP Transport, p.11]:

```
POST /lightstreamer/<request-name>.txt?LS_protocol=TLCP-2.5.0 <HTTP-version>
<HTTP-request-headers>
<empty-line>
<param1>=<value1>&...&<paramN>=<valueN>
```

- `<request-name>` determines the kind of request; e.g. `create_session` for a session creation request, `control` for control requests [§HTTP Transport, p.11].
- `LS_protocol=TLCP-2.5.0` is a **required** parameter on the query string, specifying the request protocol [§HTTP Transport, p.11].
- `<param>=<value>` pairs set the parameters of the request [§HTTP Transport, p.11].
- The line separator is `CR-LF`, as per HTTP 1.x specifications [§HTTP Transport, p.11].
- The path extension is `.txt` [§HTTP Transport, p.11]. The full path is `/lightstreamer/<request-name>.txt`.

The chapter-2 restatement of the same form makes the optional session default explicit [§Request/Response Reference, p.22]:

```
POST /lightstreamer/<req-name>.txt?LS_protocol=TLCP-2.5.0[&LS_session=<session-ID>] <HTTP-version>
<HTTP-request-headers>
<empty-line>
<param1>=<value1>&...&<paramN>=<valueN>
...
<param1>=<value1>&...&<paramN>=<valueN>
```

**Batching (control requests only).** In the specific case of control requests, multiple requests **with the same request name** may be sent in a single batch [§HTTP Transport, p.11]:

- An `LS_session=<session-ID>` pair in the **query string**, if present, acts as a **default value** for the requests specified in the body [§HTTP Transport, p.11].
- Each `<param1>=<value1>&...&<paramN>=<valueN>` line in the request body specifies a **separate** control request [§HTTP Transport, p.11].
- The line separator is `CR-LF`, also within the request body. The separator is **optional after the last line**, unless the line is empty (which is possible, in principle) [§HTTP Transport, p.11].
- Requests of the same batch are executed **concurrently**, and responses may arrive **out of order** [§HTTP Transport, p.11].

**Body charset and percent-encoding.** The request body can be expressed in any charset supported by the Java installation which runs the Server, but UTF-8 is recommended and is guaranteed to be supported [§HTTP Transport, p.11]. Reserved characters in the values of parameters specified in the body must be percent-encoded; such reserved characters are **limited to** [§HTTP Transport, p.12]:

| Character | Why reserved |
|---|---|
| ASCII `CR` and `LF` | used for line separation |
| `&` and `=` | used for parameter separation |
| `%` and `+` | used for percent-encoding / URL encoding |

Any further percent-encoding is supported, provided it is based on the UTF-8 character set (regardless of the charset used for the request body); this allows use of existing encoding libraries, although it introduces inefficiencies [§HTTP Transport, p.12]. Parameters on the request query string don't make use of any reserved character; however, should a user-agent perform extra percent-encoding on the request line, it would be supported as well [§HTTP Transport, p.12].

The response is received as the content of the HTTP response body [§HTTP Transport, p.12].

### 1.2 WS transport

Open a WebSocket to the Lightstreamer Server using [§WS Transport, p.12]:

| Element | Exact value |
|---|---|
| URI | `/lightstreamer` |
| Subprotocol header | `Sec-WebSocket-Protocol: TLCP-2.5.0.lightstreamer.com` |

Once the WS connection has been established, the general form of a request is [§WS Transport, p.12]:

```
<request-name>
<param1>=<value1>&...&<paramN>=<valueN>
```

- The line separator is `CR-LF`; note that name and parameters are on **separate lines** [§WS Transport, p.12].
- The separator is **optional after the last line**, unless the line is empty (which is possible, in principle) [§WS Transport, p.12].
- **Each request must be sent in a single WS message** [§WS Transport, p.12].
- With WS the name extension (e.g. `.txt`) is **missing** [§WS Transport, p.12].
- WS messages carrying requests must be of **text** (as opposed to binary) type, hence based on the UTF-8 character set [§WS Transport, p.12].
- The same reserved-character percent-encoding rules as HTTP apply to parameter values in the message [§WS Transport, p.12].

Batching on WS uses the same shape [§WS Transport, p.12]:

```
<request-name>
<param1>=<value1>&...&<paramN>=<valueN>
...
<param1>=<value1>&...&<paramN>=<valueN>
```

Requests of the same batch are executed concurrently and responses may arrive out of order; moreover, with WS, responses may always arrive **interspersed with notifications** when control requests are sent on the stream connection [§WS Transport, p.13]. As long as a session is bound to a WS, **no other session can be bound**; after unbinding, the same or a different session can be bound. After binding a second session, late responses to control requests related to the first session may arrive interspersed with notifications for the second session [§WS Transport, p.13].

### 1.3 Response framing

Responses and notifications share one general syntax; with HTTP transport the response is in the body of the HTTP response [§Request/Response Reference, p.22]:

```
<tag>,<argument1>,...,<argumentN>
```

Line separator is `CR-LF`. Encoding is UTF-8 [§Request/Response Reference, p.22].

> The spec warns that in its examples the `CR-LF` parts are not made explicit and separate text lines are used instead; copy-and-pasting examples may not yield correct requests depending on editor end-of-line settings, and the full `CR-LF` must always be present rather than only `CR` or only `LF` [§Request/Response Reference, p.22].

⚠️ **Spec unclear:** The `Content-Length` header values in several HTTP request examples do not match the byte length of the body shown. `create_session` states `Content-Length: 65` for a 90-byte body [p.24]; the subscription example states `Content-Length: 147` for a 148-byte body [p.31]; the `msg` example states `Content-Length: 95` for a 96-byte body [p.43]. Other examples (`bind_session` 36 [p.26], unsubscription 71 [p.32], reconf 102 [p.33], constrain 95 [p.34], force_rebind 66 [p.35], destroy 61 [p.36], heartbeat 36 [p.46]) are self-consistent. The example `Content-Length` values must therefore not be treated as normative fixtures.

---

## 2. Session Creation Request

**Request name:** `create_session` [§Session Creation Request, p.22]

**Purpose:** obtain an initial **session ID** and an initial **stream connection** to receive real-time notifications [§Session Creation Request, p.22].

**Targets:**

| Transport | Target |
|---|---|
| HTTP | `POST /lightstreamer/create_session.txt?LS_protocol=TLCP-2.5.0` [§Session Creation Request, p.24] |
| WS | first line `create_session` [§Session Creation Request, p.24] |

### 2.1 Parameters

| Parameter | Requirement | Value syntax / constraints | Default when omitted | Meaning |
|---|---|---|---|---|
| `LS_cid` | Required | Must be set with the special string `mgQkwtwdysogQz2BJ4Ji%20kOj2Bg` for all custom developed clients | — | Client identifier [§Session Creation Request, p.22] |
| `LS_user` | Optional | Free string, interpreted and verified by the Metadata Adapter | A null user name is specified to the Metadata Adapter | User name (used for authentication). In simplified scenarios the argument can be omitted, but authentication is still requested to the Metadata Adapter [§Session Creation Request, p.22] |
| `LS_password` | Optional | Free string, interpreted and verified by the Metadata Adapter | — | User password (used for authentication) [§Session Creation Request, p.23] |
| `LS_adapter_set` | Optional | Logical name of the Adapter Set | An adapter set named `DEFAULT` is assumed | Identifies the Adapter Set (the Metadata Adapter and related Data Adapters) that will serve and provide data for this stream connection [§Session Creation Request, p.23] |
| `LS_requested_max_bandwidth` | Optional | Expressed in **kbps**; it can be a decimal number, with a **dot** as decimal separator | — | Maximum bandwidth requested by the client. *Edition Note: Bandwidth Control is an optional feature, available depending on Edition and License Type* [§Session Creation Request, p.23] |
| `LS_content_length` | Optional | Expressed in **bytes** | Assigned by Lightstreamer Server, based on its own configuration | Content-Length to be used for the connection content. Also assigned by the Server if the supplied value is too low [§Session Creation Request, p.23] |
| `LS_supported_diffs` | Optional | A (possibly empty) **comma-separated list of format tags**, as specified in *Real-Time Update* | Server free to choose among its "diff" algorithms | Text "diff" formats accepted for the indication of update values; restricts the set the Server may choose from [§Session Creation Request, p.23] |
| `LS_polling` | Optional | `true` requests a polling connection | Not a polling connection (streaming) | If `true`, the Server will send only the notifications that are ready at connection time and will exit immediately, keeping the session active for subsequent rebind requests [§Session Creation Request, p.23] |
| `LS_polling_millis` | **Only if `LS_polling` is `true`** | Expressed in milliseconds; if too high, the Server may apply a configured maximum time | — (required by the Server in the polling case) | Expected time between the closing of the connection and the next polling connection. Required by the Server in order to ensure that the underlying session is kept active across polling connections. The timeout used is notified to the client in the response [§Session Creation Request, p.23] |
| `LS_idle_millis` | **Only if `LS_polling` is `true`**; Optional | Expressed in milliseconds; if too high, the Server may apply a configured maximum time | Not specified ⇒ Server response is synchronous and might be empty | Time the Server is allowed to wait for a notification, if none is present at request time. If zero or not specified, the response is synchronous and might be empty; if positive, the response is asynchronous and, if the timeout expires, might be empty [§Session Creation Request, p.23] |
| `LS_inactivity_millis` | **Only if `LS_polling` is not `true`**; Optional | Expressed in milliseconds | — | Maximum time the Client is committed to wait before issuing a request to the Server while this connection is open. If no request is needed, a `heartbeat` pseudo-request can be sent. If no request is received for more than this time, the Server can assume that the client is stuck and act accordingly [§Session Creation Request, p.23] |
| `LS_keepalive_millis` | **Only if `LS_polling` is not `true`**; Optional | Expressed in milliseconds; if too low the Server may apply a configured minimum, if too high a configured maximum | Decided by the Server, based on its own configuration | Longest inactivity time allowed for the connection; on such inactivity the Server sends a keepalive message (a `PROBE` notification). The keepalive time used is notified to the client in the response [§Session Creation Request, pp.23-24] |
| `LS_send_sync` | **Only if `LS_polling` is not `true`**; Optional | `false` suppresses `SYNC` | `true` | If set to `false`, instructs the Server not to send the `SYNC` notifications on this connection [§Session Creation Request, p.24] |
| `LS_reduce_head` | Optional | `true` / `false` | `false` | If `true`, instructs the Server not to send the `SERVNAME` and `CLIENTIP` notifications on this connection; moreover, instructs the Server not to send the `CONS` notifications **throughout all the session** [§Session Creation Request, p.24] |
| `LS_ttl_millis` | Optional | Expressed in milliseconds, or the special strings `unknown` / `unlimited` | `unknown` behaviour (request kept until completion) | Maximum time the client is committed to wait for an answer to this request before giving up. Allows the Server to interrupt processing and discard the request if the time limit cannot be obeyed [§Session Creation Request, p.24] |

`LS_ttl_millis` notes, verbatim in substance [§Session Creation Request, p.24]:

- The special value `unknown` is also supported. In this case, the request is kept until completion. This is also the **default behavior**.
- A request can also be discarded before completion when the connection is closed.
- A request can also be discarded before completion (with **error code 5**) when the Server is overloaded and it determines that a significant delay is still going to be accumulated.
- The special value `unlimited` is also supported. It is similar to `unknown`, but, in this case, the request is kept also if the Server is overloaded.

### 2.2 Conditionality and exclusivity

- `LS_polling=true` **enables** `LS_polling_millis` and `LS_idle_millis`, and **disables** `LS_inactivity_millis`, `LS_keepalive_millis` and `LS_send_sync`; when `LS_polling` is not `true` the converse holds [§Session Creation Request, p.23]. The two groups are therefore mutually exclusive.
- `LS_polling_millis` is stated as *required by the Server* in the polling case, i.e. `LS_polling=true` **implies** `LS_polling_millis` [§Session Creation Request, p.23].
- The spec states **no ordering requirement** among parameters within the parameter line.

### 2.3 Examples

Request example with HTTP transport [p.24]:

```
POST /lightstreamer/create_session.txt?LS_protocol=TLCP-2.5.0 HTTP/1.1
Host: push.lightstreamer.com
Accept: */*
Content-Length: 65
Content-Type: text/plain

LS_user=user&LS_password=password&LS_adapter_set=DEMO&LS_cid=mgQkwtwdysogQz2BJ4Ji%20kOj2Bg
```

Request example with WS transport [p.24]:

```
create_session
LS_user=user&LS_password=password&LS_adapter_set=DEMO&LS_cid=mgQkwtwdysogQz2BJ4Ji%20kOj2Bg
```

(In the printed spec the `LS_cid` value wraps across two typeset lines — `…BJ4Ji` / `%20kOj2Bg` — this is page layout, not a `CR-LF`.)

**Responses:** see §4 below; session creation and binding requests share the same responses [§Session Creation and Binding Responses, p.26].

---

## 3. Session Binding Request

**Request name:** `bind_session` [§Session Binding Request, p.24]

**Purpose:** rebind an existing session ID to a **new stream connection** and restart the flow of real-time updates and notifications [§Session Binding Request, p.24].

**Targets:**

| Transport | Target |
|---|---|
| HTTP | `POST /lightstreamer/bind_session.txt?LS_protocol=TLCP-2.5.0` [§Session Binding Request, p.26] |
| WS | first line `bind_session` [§Session Binding Request, p.26] |

### 3.1 Parameters

| Parameter | Requirement | Value syntax / constraints | Default when omitted | Meaning |
|---|---|---|---|---|
| `LS_session` | **Required with HTTP**; optional with WS | The Server internal string representing the session | With WS: the last session that was bound to the WS connection | The session the client wants to bind to [§Session Binding Request, p.25] |
| `LS_recovery_from` | Optional | Progressive number of the last data notification received in the session (equivalently, total number of data notifications received from the beginning of the session) | Server assumes all updates sent in the previous connection (up to the `LOOP` notification) were received and restarts from the next update | Determines the starting point for the response flow; related to the Session Recovery mechanism. Specifying a number lets the client recover from an interruption of the previous stream connection, provided the Server is still keeping the missed notifications. **Causes a `PROG` notification to be issued on the response before any data notification** [§Session Binding Request, p.25] |
| `LS_content_length` | Optional | — | Assigned by Lightstreamer Server, based on its own configuration | Content-Length to be used for the connection content. Also assigned by the Server if too low [§Session Binding Request, p.25] |
| `LS_polling` | Optional | `true` requests a polling connection | Not a polling connection | If `true`, the Server sends only the notifications ready at connection time and exits immediately, keeping the session active for subsequent rebind requests [§Session Binding Request, p.25] |
| `LS_polling_millis` | **Only if `LS_polling` is `true`** | If too high, the Server may apply a configured maximum time | — (required by the Server in the polling case) | Expected time between the closing of the connection and the next polling connection. The timeout used is notified to the client in the response [§Session Binding Request, p.25] |
| `LS_idle_millis` | **Only if `LS_polling` is `true`**; Optional | If too high, the Server may apply a configured maximum time | Not specified ⇒ synchronous, possibly empty response | Time the Server is allowed to wait for a notification, if none is present at request time. Zero or absent ⇒ synchronous response; positive ⇒ asynchronous response, possibly empty on timeout [§Session Binding Request, p.25] |
| `LS_inactivity_millis` | **Only if `LS_polling` is not `true`**; Optional | — | — | Maximum time the Client is committed to wait before issuing a request while this connection is open. A `heartbeat` pseudo-request may be sent if no request is needed. Exceeding it lets the Server assume the client is stuck [§Session Binding Request, p.25] |
| `LS_keepalive_millis` | **Only if `LS_polling` is not `true`**; Optional | If too low, configured minimum applies; if too high, configured maximum applies | Decided by the Server, based on its own configuration | Longest inactivity time allowed for the connection; on such inactivity the Server sends a keepalive message (a `PROBE` notification). The keepalive time used is notified to the client in the response [§Session Binding Request, p.26] |
| `LS_send_sync` | **Only if `LS_polling` is not `true`**; Optional | `false` suppresses `SYNC` | `true` | If `false`, instructs the Server not to send `SYNC` notifications on this connection [§Session Binding Request, p.26] |
| `LS_reduce_head` | Optional | `true` / `false` | `false` | If `true`, instructs the Server not to send the `CONOK`, `SERVNAME`, and `CLIENTIP` notifications on this connection [§Session Binding Request, p.26] |

`LS_session` notes, verbatim in substance [§Session Binding Request, p.25]:

- When omitted with WS transport, the rebind will apply to the **last session that was bound to the WS connection**. In other words, the request can be made to rebind on the same stream connection the session was previously bound to. Hence it is **required that a preceding session creation or binding request took place and was fulfilled successfully**.
- It is **always required with HTTP transport**.

### 3.2 Conditionality and exclusivity

- The `LS_polling=true` group (`LS_polling_millis`, `LS_idle_millis`) and the streaming group (`LS_inactivity_millis`, `LS_keepalive_millis`, `LS_send_sync`) are mutually exclusive, exactly as for `create_session` [§Session Binding Request, pp.25-26].
- `bind_session` accepts **no** `LS_ttl_millis`, `LS_cid`, `LS_user`, `LS_password`, `LS_adapter_set`, `LS_requested_max_bandwidth` or `LS_supported_diffs`; those exist only on `create_session` [§Session Creation Request, pp.22-24; §Session Binding Request, pp.25-26].

⚠️ **Spec unclear:** `LS_reduce_head` has **different** stated effects on the two requests. On `create_session` it suppresses `SERVNAME` and `CLIENTIP` plus `CONS` for the whole session [p.24]; on `bind_session` it suppresses `CONOK`, `SERVNAME` and `CLIENTIP`, with no mention of `CONS` [p.26]. The spec does not explain whether `CONOK` suppression is bind-only by design, nor whether a `create_session`-set `LS_reduce_head` persists in effect for later binds that omit it.

⚠️ **Spec unclear:** On `create_session` the units are stated explicitly (`LS_content_length` "in bytes", `LS_polling_millis`/`LS_idle_millis`/`LS_keepalive_millis` "in milliseconds") [pp.23-24]; on `bind_session` those unit statements are omitted from the corresponding entries [pp.25-26]. Identity of units across the two requests is implied but not stated.

### 3.3 Examples

Request example with HTTP transport [p.26]:

```
POST /lightstreamer/bind_session.txt?LS_protocol=TLCP-2.5.0 HTTP/1.1
Host: push.lightstreamer.com
Accept: */*
Content-Length: 36
Content-Type: text/plain

LS_session=S73d162c183916f0dT2729905
```

Request example with WS transport [p.26]:

```
bind_session
LS_session=S73d162c183916f0dT2729905
```

---

## 4. Session Creation and Binding Responses

Session creation and binding requests **share the same responses** [§Session Creation and Binding Responses, p.26].

### 4.1 Successful Session Creation or Binding Response

| Property | Value |
|---|---|
| Tag | `CONOK` (short of CONnection OK) |
| Format | `CONOK,<session-ID>,<request-limit>,<keep-alive>,<control-link>` |
| Arguments | 4 |

[§Successful Session Creation or Binding Response, p.26]

| Argument | Meaning |
|---|---|
| `<session-ID>` | The Server internal string representing the session ID [p.26] |
| `<request-limit>` | The maximum length allowed by the Server for a client request, **expressed in bytes**. Configured through the `<request_limit>` element in the Server configuration file. The system must be configured in advance to prevent any single request from exceeding this limit; the information is useful when **batching** multiple control requests [p.26] |
| `<keep-alive>` | The longest inactivity time guaranteed throughout connection life time, **expressed in milliseconds**. For a **stream** connection, when no notifications have been sent for this time, a `PROBE` notification is sent. For a **polling** connection, if the response has not been supplied for this time, an empty response is issued — in practice this value corresponds to the requested `LS_idle_millis` (but for a possible Server upper limit). Not receiving any message for longer than this time may signal a problem [pp.26-27] |
| `<control-link>` | If a control link is specified in the Server configuration file, the address (IP address, or hostname, and port) to which every following **session rebind and control request** must be sent, in case this requires the establishment of a new socket (possibly a websocket), to ensure session affinity in case of clustering. If no control link is configured, this parameter has the special value asterisk `*` (UTF-8 code `0x2a`), meaning the client should send all session rebind and control requests to the same address to which it opened this stream connection [p.27] |

Further `<control-link>` notes [§Successful Session Creation or Binding Response, p.27]:

- With **WS** transport, issuing requests directly on the stream connection is **always allowed**. Yet, the establishment of a new socket may still be needed, for instance when attempting to recover the session while the current stream connection is unresponsive.
- Upon the first time the "control link" address is used to open a new stream connection to rebind to a session, the streaming activity, that was flowing to the initial address, **will switch to the "control link"**.
- *Edition Note: Server Clustering is an optional feature, available depending on Edition and License Type.*

Response example with HTTP transport [p.27]:

```
HTTP/1.1 200 OK
Server: Lightstreamer-Server/7.0.0 build 1972
Content-Type: text/enriched; charset=iso-8859-1
Cache-Control: no-store
Cache-Control: no-transform
Cache-Control: no-cache
Pragma: no-cache
Expires: Thu, 1 Jan 1970 00:00:00 GMT
Date: Fri, 1 Jul 2016 14:01:19 GMT
Transfer-Encoding: chunked

2E
CONOK,S73d162c183916f0dT2729905,50000,5000,*
```

Response example with WS transport [p.27]:

```
CONOK,S73d162c183916f0dT2729905,50000,5000,*
```

**Notifications after `CONOK`.** Whatever content follows is a real-time update or any other kind of notification; with WS transport they can be interspersed with responses to control requests sent on the stream connection. After a `CONOK` is sent the following notifications are also immediately sent, **in any order** [§Successful Session Creation or Binding Response, pp.27-28]:

| Notification | Condition |
|---|---|
| `SERVNAME` | can be omitted on a Binding Response, if unchanged |
| `CLIENTIP` | if available — can also be omitted on a Binding Response, if unchanged |
| `CONS` | — |
| `PROG` | if requested via `LS_recovery_from` on `bind_session` |

### 4.2 Other Session Creation or Binding Responses

| Property | Value |
|---|---|
| Tag | `CONERR` (short of CONnection ERRor) |
| Format | `CONERR,<error-code>,<error-message>` |
| Used when | With both session creation and binding, when the session could not be created or bound |
| Arguments | 2 |

[§Other Session Creation or Binding Responses, p.28]

| Argument | Meaning |
|---|---|
| `<error-code>` | Error code sent by the Server kernel or by the Metadata Adapter. For a list of supported error codes see **Appendix A** [p.28] |
| `<error-message>` | Error message sent by Server kernel or by the Metadata Adapter. **A message sent by the Metadata Adapter may be empty** [p.28] |

Error response example [p.28]:

```
CONERR,2,Requested Adapter Set not available
```

| Property | Value |
|---|---|
| Tag | `END` (short of session END) |
| Format | `END,<cause-code>,<cause-message>` |
| Used when | **Only with session rebinding**, when the requested session has just been forcibly closed on the Server side |
| Arguments | 1 (as printed) |

[§Other Session Creation or Binding Responses, p.28]

| Argument | Meaning |
|---|---|
| `<cause-code>` | Cause code sent by the Server kernel. For a list of supported cause codes see **Appendix A** [p.28] |
| `<cause-message>` | Short cause description sent by the Server kernel [p.28] |

End response example [p.28]:

```
END,8,Session count limit reached
```

⚠️ **Spec unclear:** The `END` response is declared with "**Arguments: 1**" but its `Format` line and its argument list both show **two** arguments (`<cause-code>` and `<cause-message>`), and the example `END,8,Session count limit reached` carries two [p.28]. The stated count appears to be an error; the format line and example are self-consistent with two arguments.

---

## 5. Control Requests — common structure

Control requests provide means to change the set of items and fields sent with real-time updates, send messages, and change parameters of an active subscription or session [§Control Requests, p.28].

All control requests **except message send requests** share a common request name and some mandatory parameters; one of them, `LS_op`, indicates the specific control operation [§Common Parameters, p.29].

**Request name:** `control` [§Common Parameters, p.29]

**Targets:**

| Transport | Target |
|---|---|
| HTTP | `POST /lightstreamer/control.txt?LS_protocol=TLCP-2.5.0[&LS_session=<session-ID>]` [§Common Parameters, p.29; §Request Syntax, p.11] |
| WS | first line `control` [§Common Parameters, p.29] |

### 5.1 Common parameters

| Parameter | Requirement | Value syntax / constraints | Default when omitted | Meaning |
|---|---|---|---|---|
| `LS_session` | **Required with HTTP**; optional with WS | The session ID received from the Server at the beginning of the stream connection | With WS: the ID of the last session that was bound to the WS connection | Identifies the session the control request acts on [§Common Parameters, p.29] |
| `LS_reqId` | Required | **Any combination of letters and numbers**; typically a progressive number. Must be **unique within the connection** | — | ID of the request [§Common Parameters, p.29] |
| `LS_op` | Required | Op-code of the specific control operation; see each request below | — | Selects the control operation [§Common Parameters, p.29] |

`LS_session` notes, verbatim in substance [§Common Parameters, p.29]:

- When omitted with WS transport, its default value is the ID of the last session that was bound to the WS connection (it may be currently still bound or **temporarily unbound due to a polling cycle**). Hence this requires that a preceding session creation or binding request was issued.
- When omitted with WS transport to leverage the default provided by a preceding session creation or binding request, the control request is **accepted immediately, regardless that the preceding request has started**. In any case, the response will always **follow** the initial response of the preceding request. Obviously, a failure of such preceding request would cause this control request to fail as well.
- With HTTP transport, it may be specified **on the query string**, instead of the request body, to address the same session with a multiple request.

### 5.2 `LS_op` values covered in this chapter

| `LS_op` | Operation | Section |
|---|---|---|
| `add` | Subscription Request | §6 |
| `delete` | Unsubscription Request | §7 |
| `reconf` | Subscription Reconfiguration Request | §8 |
| `constrain` | Session Constrain Request | §9 |
| `force_rebind` | Force Session Rebind Request | §10 |
| `destroy` | Session Destroy Request | §11 |

(MPN op-codes are defined on pp.36-42 and are documented in the MPN chapter of this reference.)

The `msg` request is still a control request but uses a **different request name** and hence **does not require the `LS_op` parameter** [§Message Send Request, p.42].

⚠️ **Spec unclear:** `LS_ack` is documented as a parameter of `add` [p.31], `delete` [p.32] and `msg` [p.43] only. The spec does not state whether `LS_ack` is accepted, ignored, or rejected on `reconf`, `constrain`, `force_rebind` and `destroy`.

---

## 6. Subscription Request (`LS_op=add`)

**Request name:** `control` [§Common Parameters, p.29] · **Transports:** HTTP and WS [§Subscription Request, pp.31-32]

### 6.1 Parameters

| Parameter | Requirement | Value syntax / constraints | Default when omitted | Meaning |
|---|---|---|---|---|
| `LS_session` | See §5.1 | See §5.1 | See §5.1 | See the Common Parameters section [§Subscription Request, p.29] |
| `LS_reqId` | See §5.1 | See §5.1 | — | See the Common Parameters section [§Subscription Request, p.29] |
| `LS_op` | Required | Literal `add` | — | Specifies that a **new subscription** must be added to the session [§Subscription Request, p.29] |
| `LS_subId` | Required | A **progressive integer number starting with 1**, **unique within the session** | — | ID of the subscription [§Subscription Request, p.29] |
| `LS_data_adapter` | Optional | Logical name of a Data Adapter available in the Adapter Set; this Data Adapter must supply **all** the requested items | A Data Adapter named `DEFAULT` within the Adapter Set is assumed | Data Adapter that will provide data for this subscription [§Subscription Request, p.29] |
| `LS_group` | Required | Interpreted by the Metadata Adapter | — | Identification name of the item group being subscribed [§Subscription Request, p.29] |
| `LS_schema` | Required | Interpreted by the Metadata Adapter | — | Identification name of the field schema for which the subscription should provide real-time updates [§Subscription Request, p.30] |
| `LS_selector` | Optional | Interpreted by the Metadata Adapter | — | Identification name of a selector related to the items in the subscription [§Subscription Request, p.30] |
| `LS_mode` | Required | One of `RAW`, `MERGE`, `DISTINCT`, `COMMAND` | — | Subscription mode of all the items in the subscription [§Subscription Request, p.30] |
| `LS_requested_buffer_size` | Optional | Number of update events, or the literal `unlimited` | `1` if `LS_mode` is `MERGE`; `unlimited` if `LS_mode` is `DISTINCT` | Dimension of the buffers related to the items in the subscription. The Server may limit the buffer size. `unlimited` requests no limit. **Considered only if `LS_mode` is `MERGE` or `DISTINCT` and `LS_requested_max_frequency` is not set to `unfiltered`** [§Subscription Request, p.30] |
| `LS_requested_max_frequency` | Optional | A decimal number (with a **dot** as decimal separator) of updates/sec, or the literal `unlimited`, or the literal `unfiltered` | `unlimited` | Maximum update frequency for the items in the subscription. The Server may limit the frequency [§Subscription Request, p.30] |
| `LS_snapshot` | Optional | `true`, `false`, or an integer number of updates | `false` | Requested snapshot length for the items in the subscription, expressed in number of events or as a boolean [§Subscription Request, pp.30-31] |
| `LS_ack` | Optional; **only if the stream connection is on WS transport** | `false` suppresses the `REQOK` response | `true` | If `false` the `REQOK` response is not sent [§Subscription Request, p.31] |

`LS_requested_max_frequency` notes, verbatim in substance [§Subscription Request, p.30]:

- If set to `unlimited`, the Server forwards each update as soon as possible, with no frequency limit.
- If set to `unfiltered`, the Server forwards each update as soon as possible, with no frequency limit, but **also without losses**. More precisely, any loss will be signaled through an overflow (`OV`) notification. Considered only if `LS_mode` is `MERGE`, `DISTINCT` or `COMMAND`.
- If set to a decimal number (with a dot as decimal separator), the maximum frequency is set to the corresponding number of updates/sec. Considered only if `LS_mode` is `MERGE`, `DISTINCT` or `COMMAND` (with `COMMAND` the maximum frequency applies to the `UPDATE` commands sent for each key).
- Default value: `unlimited`.
- *Edition Note: A further global frequency limit could also be imposed by the Server, depending on Edition and License Type; this specific limit also applies to `RAW` mode and to `unfiltered` dispatching.*

`LS_snapshot` notes, verbatim in substance [§Subscription Request, pp.30-31]:

- If set to `true`, the Server sends the snapshot (possibly empty) for the items contained in the subscription. Considered only if `LS_mode` is `MERGE`, `DISTINCT` or `COMMAND`. In the `DISTINCT` case, the length of the snapshot is decided by the Server.
- If set to `false`, the Server does not send the snapshot for the items contained in the subscription.
- If set to an integer number, the length of the snapshot is set to the corresponding number of updates. **Admitted only if `LS_mode` is `DISTINCT`.** The Server sends the snapshot for the items contained in the subscription, limiting the length to the requested value. The number of updates received may be lower if the Server imposes a stricter limit or fewer updates are available.
- Default value: `false`.

`LS_ack` notes, verbatim in substance [§Subscription Request, p.31]:

- Skipping the `REQOK` response may be useful when issuing many subscriptions at once. A successful subscription is also notified with `SUBOK` on the stream connection.
- **If the request fails, the consequent `REQERR` or `ERROR` response is always sent.**
- **Ignored if used with HTTP transport**, as an HTTP response is always sent.
- Default value is `true`.

### 6.2 Conditionality and exclusivity

- `LS_requested_max_frequency=unfiltered` **suppresses** `LS_requested_buffer_size` (which is considered only when the frequency is not `unfiltered`) [§Subscription Request, p.30].
- `LS_requested_buffer_size` is considered only for `LS_mode` `MERGE` or `DISTINCT`; the spec states **no** default and no effect for `RAW` and `COMMAND` [§Subscription Request, p.30].
- `LS_snapshot` set to an integer is admitted **only** for `LS_mode=DISTINCT`; `true` is considered only for `MERGE`, `DISTINCT`, `COMMAND` [§Subscription Request, pp.30-31].
- `LS_ack` is meaningful only on WS [§Subscription Request, p.31].

### 6.3 Notifications on success

If successful, a `SUBOK` or `SUBCMD` notification is sent on the stream connection, **followed by a `CONF` notification**. After that, real-time updates of the subscribed items start to be delivered, beginning with the snapshot part, if requested [§Subscription Request, p.31].

### 6.4 Examples

Request example with HTTP transport [p.31]:

```
POST /lightstreamer/control.txt?LS_protocol=TLCP-2.5.0 HTTP/1.1
Host: push.lightstreamer.com
Accept: */*
Content-Length: 147
Content-Type: text/plain

LS_session=Sd9fce58fb5dbbebfT2255126&LS_reqId=1&LS_op=add&LS_subId=1&LS_group=item1&LS_schema=last_price&LS_data_adapter=QUOTE_ADAPTER&LS_mode=MERGE
```

Request example with WS transport [p.31]:

```
control
LS_reqId=1&LS_op=add&LS_subId=1&LS_group=item1&LS_schema=last_price&LS_data_adapter=QUOTE_ADAPTER&LS_mode=MERGE
```

Multiple request example with HTTP transport [p.31] — reproduced verbatim, including the missing `&` after `LS_reqId=1` / `LS_reqId=2`:

```
POST /lightstreamer/control.txt?LS_protocol=TLCP-2.5.0&LS_session=Sd9fce58fb5dbbebfT2255126 HTTP/1.1
Host: push.lightstreamer.com
Accept: */*
Content-Length: 223
Content-Type: text/plain

LS_reqId=1LS_op=add&LS_subId=1&LS_group=item1&LS_schema=last_price&LS_data_adapter=QUOTE_ADAPTER&LS_mode=MERGE
LS_reqId=2LS_op=add&LS_subId=2&LS_group=item2&LS_schema=last_price&LS_data_adapter=QUOTE_ADAPTER&LS_mode=MERGE
```

Multiple request example with WS transport [pp.31-32]:

```
control
LS_reqId=1&LS_op=add&LS_subId=1&LS_group=item1&LS_schema=last_price&LS_data_adapter=QUOTE_ADAPTER&LS_mode=MERGE
LS_reqId=2&LS_op=add&LS_subId=2&LS_group=item2&LS_schema=last_price&LS_data_adapter=QUOTE_ADAPTER&LS_mode=MERGE
```

⚠️ **Spec unclear:** In the printed *Multiple request example with HTTP transport* [p.31], both body lines read `LS_reqId=1LS_op=add&…` and `LS_reqId=2LS_op=add&…`, with **no `&` separating `LS_reqId` from `LS_op`**, whereas the WS multiple-request example on the same and following page correctly shows `LS_reqId=1&LS_op=add&…`. Verified against the PDF page image; it is not a text-extraction artifact. The general syntax rule on p.11 requires `&` between pairs, so the HTTP example appears to be a typographical error, but the spec does not correct it.

**Responses:** see §13.

---

## 7. Unsubscription Request (`LS_op=delete`)

**Request name:** `control` · **Transports:** HTTP and WS [§Unsubscription Request, p.32]

### 7.1 Parameters

| Parameter | Requirement | Value syntax / constraints | Default when omitted | Meaning |
|---|---|---|---|---|
| `LS_session` | See §5.1 | See §5.1 | See §5.1 | See the Common Parameters section [§Unsubscription Request, p.32] |
| `LS_reqId` | See §5.1 | See §5.1 | — | See the Common Parameters section [§Unsubscription Request, p.32] |
| `LS_op` | Required | Literal `delete` | — | Specifies that an **existing subscription** must be deleted from the session [§Unsubscription Request, p.32] |
| `LS_subId` | Required | **The same progressive integer number used during subscription** | — | ID of the existing subscription [§Unsubscription Request, p.32] |
| `LS_ack` | Optional; **only if the stream connection is on WS transport** | `false` suppresses the `REQOK` response | `true` | If `false` the `REQOK` response is not sent [§Unsubscription Request, p.32] |

`LS_ack` notes, verbatim in substance [§Unsubscription Request, p.32]:

- Skipping the `REQOK` response may be useful when issuing many unsubscriptions at once. A successful unsubscription is also notified with `UNSUB` on the stream connection.
- If the request fails, the consequent `REQERR` or `ERROR` response is always sent.
- Ignored if used with HTTP transport, as an HTTP response is always sent.
- Default value is `true`.

### 7.2 Notifications on success

If successful, an `UNSUB` notification is sent on the stream connection, **after the last real-time update of subscribed items has been delivered** [§Unsubscription Request, p.32].

### 7.3 Examples

Request example with HTTP transport [p.32]:

```
POST /lightstreamer/control.txt?LS_protocol=TLCP-2.5.0 HTTP/1.1
Host: push.lightstreamer.com
Accept: */*
Content-Length: 71
Content-Type: text/plain

LS_session=Sd9fce58fb5dbbebfT2255126&LS_reqId=2&LS_op=delete&LS_subId=1
```

Request example with WS transport [p.32]:

```
control
LS_reqId=2&LS_op=delete&LS_subId=1
```

---

## 8. Subscription Reconfiguration Request (`LS_op=reconf`)

**Request name:** `control` · **Transports:** HTTP and WS [§Subscription Reconfiguration Request, p.33]

### 8.1 Parameters

| Parameter | Requirement | Value syntax / constraints | Default when omitted | Meaning |
|---|---|---|---|---|
| `LS_session` | See §5.1 | See §5.1 | See §5.1 | See the Common Parameters section [§Subscription Reconfiguration Request, pp.32-33] |
| `LS_reqId` | See §5.1 | See §5.1 | — | See the Common Parameters section [§Subscription Reconfiguration Request, pp.32-33] |
| `LS_op` | Required | Literal `reconf` | — | Specifies that an **existing subscription** must be reconfigured [§Subscription Reconfiguration Request, p.33] |
| `LS_subId` | Required | **The same progressive integer number used during subscription** | — | ID of the existing subscription [§Subscription Reconfiguration Request, p.33] |
| `LS_requested_max_frequency` | Optional | A decimal number (with **dot** as decimal separator) of updates/sec, or the literal `unlimited` | No modification is requested | New maximum update frequency for the items in the subscription [§Subscription Reconfiguration Request, p.33] |

`LS_requested_max_frequency` notes, verbatim in substance [§Subscription Reconfiguration Request, p.33]:

- May be a decimal number (with dot as a decimal separator).
- If the value is the string `unlimited`, the request is to **relieve any existing frequency limit**.
- **Admitted only if the subscription is not subscribed to with `unfiltered` max frequency.**
- Considered only if the subscription mode of the subscription is `MERGE`, `DISTINCT` or `COMMAND` (in `COMMAND` mode the maximum frequency applies to the `UPDATE` commands sent for each key).
- *Edition Note: A further global frequency limit could also be imposed by the Server, depending on Edition and License Type; this specific limit also applies to `RAW` mode and to `unfiltered` dispatching.*

**Note from the spec:** this control request is designed to change different subscription configuration parameters, in the future. **As of TLCP-2.5.0, the only changeable parameter is the subscription maximum frequency** [§Subscription Reconfiguration Request, p.33].

### 8.2 Notifications on success

If successful, a `CONF` notification is sent on the stream connection, **before** real-time updates of the subscribed items start to be delivered with the new frequency [§Subscription Reconfiguration Request, p.33].

### 8.3 Examples

Request example with HTTP transport [p.33]:

```
POST /lightstreamer/control.txt?LS_protocol=TLCP-2.5.0 HTTP/1.1
Host: push.lightstreamer.com
Accept: */*
Content-Length: 102
Content-Type: text/plain

LS_session=Sd9fce58fb5dbbebfT2255126&LS_reqId=3&LS_op=reconf&LS_subId=1&LS_requested_max_frequency=2.0
```

Request example with WS transport [p.33]:

```
control
LS_reqId=3&LS_op=reconf&LS_subId=1&LS_requested_max_frequency=2.0
```

---

## 9. Session Constrain Request (`LS_op=constrain`)

**Request name:** `control` · **Transports:** HTTP and WS [§Session Constrain Request, p.34]

### 9.1 Parameters

| Parameter | Requirement | Value syntax / constraints | Default when omitted | Meaning |
|---|---|---|---|---|
| `LS_session` | See §5.1 | See §5.1 | See §5.1 | See the Common Parameters section [§Session Constrain Request, pp.33-34] |
| `LS_reqId` | See §5.1 | See §5.1 | — | See the Common Parameters section [§Session Constrain Request, pp.33-34] |
| `LS_op` | Required | Literal `constrain` | — | Specifies that an **existing session** must change its constraints [§Session Constrain Request, p.34] |
| `LS_requested_max_bandwidth` | Optional | A decimal number, with a **dot** as decimal separator, expressed in **kbps**, or the literal `unlimited` | No modification is requested | New maximum bandwidth requested by the client [§Session Constrain Request, p.34] |

`LS_requested_max_bandwidth` notes, verbatim in substance [§Session Constrain Request, p.34]:

- May be a decimal number, with a dot as a decimal separator.
- If the value is the string `unlimited`, the request is to **relieve any existing bandwidth limit**.
- *Edition Note: Bandwidth Control is an optional feature, available depending on Edition and License Type.*

**Note from the spec:** this control request is designed to change different session constraints, in the future. **As of TLCP-2.5.0, the only changeable constraint is the session maximum bandwidth** [§Session Constrain Request, p.34].

### 9.2 Notifications on success

If successful, a `CONS` notification is sent on the stream connection, **before** real-time updates start to be delivered with the new bandwidth [§Session Constrain Request, p.34].

### 9.3 Examples

Request example with HTTP transport [p.34]:

```
POST /lightstreamer/control.txt?LS_protocol=TLCP-2.5.0 HTTP/1.1
Host: push.lightstreamer.com
Accept: */*
Content-Length: 95
Content-Type: text/plain

LS_session=Sd9fce58fb5dbbebfT2255126&LS_reqId=4&LS_op=constrain&LS_requested_max_bandwidth=50.0
```

Request example with WS transport [p.34]:

```
control
LS_reqId=4&LS_op=constrain&LS_requested_max_bandwidth=50.0
```

---

## 10. Force Session Rebind Request (`LS_op=force_rebind`)

**Request name:** `control` · **Transports:** HTTP and WS [§Force Session Rebind Request, p.35]

### 10.1 Parameters

| Parameter | Requirement | Value syntax / constraints | Default when omitted | Meaning |
|---|---|---|---|---|
| `LS_session` | See §5.1 | See §5.1 | See §5.1 | See the Common Parameters section [§Force Session Rebind Request, p.34] |
| `LS_reqId` | See §5.1 | See §5.1 | — | See the Common Parameters section [§Force Session Rebind Request, p.34] |
| `LS_op` | Required | Literal `force_rebind` | — | Specifies that an **existing session** must be forced to rebind [§Force Session Rebind Request, p.34] |
| `LS_polling_millis` | Optional | Expressed in **milliseconds** | — | Expected time between the closing of the connection and the **next binding request** [§Force Session Rebind Request, p.34] |
| `LS_close_socket` | Optional | `true` forces the Server to close the stream connection | The stream connection **may** be left open and the client may reuse it with both HTTP (by application-wide or system-wide connection pooling) and WS | If `true` the corresponding stream connection is forcibly closed by the Server [§Force Session Rebind Request, p.35] |

### 10.2 Notifications on success

If successful, a `LOOP` notification is sent on the stream connection. After that, real-time updates **stop** being delivered and the session is **unbound** from the connection [§Force Session Rebind Request, p.35].

### 10.3 Examples

Request example with HTTP transport [p.35]:

```
POST /lightstreamer/control.txt?LS_protocol=TLCP-2.5.0 HTTP/1.1
Host: push.lightstreamer.com
Accept: */*
Content-Length: 66
Content-Type: text/plain

LS_session=Sd9fce58fb5dbbebfT2255126&LS_reqId=5&LS_op=force_rebind
```

Request example with WS transport [p.35]:

```
control
LS_reqId=5&LS_op=force_rebind
```

---

## 11. Session Destroy Request (`LS_op=destroy`)

**Request name:** `control` · **Transports:** HTTP and WS [§Session Destroy Request, p.36]

### 11.1 Parameters

| Parameter | Requirement | Value syntax / constraints | Default when omitted | Meaning |
|---|---|---|---|---|
| `LS_session` | See §5.1 | See §5.1 | See §5.1 | See the Common Parameters section [§Session Destroy Request, p.35] |
| `LS_reqId` | See §5.1 | See §5.1 | — | See the Common Parameters section [§Session Destroy Request, p.35] |
| `LS_op` | Required | Literal `destroy` | — | Specifies that an **existing session** must be terminated [§Session Destroy Request, p.35] |
| `LS_cause_code` | Optional | A numeric code; **only `0` or a negative code are supported; a positive code will be collapsed to `0`** | The `END` message reports the default code `31` | A numeric code to be reported in the consequent `END` message received on the stream connection, in place of the default code `31`. Useful if the request is issued by some server-side process [§Session Destroy Request, p.35] |
| `LS_cause_message` | Optional; **considered only if `LS_cause_code` is also supplied** | Should be simple ASCII; **multiline text is not allowed**; should be short | If `LS_cause_code` is present and `LS_cause_message` is not specified, the reported message will be `null` | A text to be reported in the consequent `END` message received on the stream connection [§Session Destroy Request, p.35] |
| `LS_close_socket` | Optional | `true` forces the Server to close the stream connection | The stream connection **is** left open and the client may reuse it with both HTTP (by application-wide or system-wide connection pooling) and WS | If `true` the corresponding stream connection is forcibly closed by the Server [§Session Destroy Request, p.36] |

`LS_cause_message` notes, verbatim in substance [§Session Destroy Request, p.35]:

- If `LS_cause_code` is not present, the cause in the `END` message will be provided by the Server, **regardless of this setting**.
- If `LS_cause_code` is present and `LS_cause_message` is not specified, then the reported message will be `null`.
- The supplied message should be in simple ASCII, otherwise it might be altered in order to be sent to the client; **multiline text is also not allowed**.
- The supplied message should be short. **If longer than 35 characters** and if a content-length limit on the connection content is in force, it **may be replaced with `[Custom message skipped]`**.

### 11.2 Conditionality

- `LS_cause_message` **implies** `LS_cause_code`: it is considered only if `LS_cause_code` is also supplied [§Session Destroy Request, p.35].

⚠️ **Spec unclear:** The default behaviour of `LS_close_socket` is worded differently between the two requests that carry it: on `force_rebind`, "the stream connection **may** be left open" [p.35]; on `destroy`, "the stream connection **is** left open" [p.36]. Whether the `force_rebind` wording denotes a genuinely unspecified behaviour or is just a looser phrasing of the same guarantee is not stated.

### 11.3 Notifications on success

If successful, an `END` notification is sent on the stream connection. After that, real-time updates stop being delivered and the session is terminated [§Session Destroy Request, p.36].

### 11.4 Examples

Request example with HTTP transport [p.36]:

```
POST /lightstreamer/control.txt?LS_protocol=TLCP-2.5.0 HTTP/1.1
Host: push.lightstreamer.com
Accept: */*
Content-Length: 61
Content-Type: text/plain

LS_session=Sd9fce58fb5dbbebfT2255126&LS_reqId=6&LS_op=destroy
```

Request example with WS transport [p.36]:

```
control
LS_reqId=6&LS_op=destroy
```

---

## 12. Message Send Request

Sending a message is still a control request, but makes use of a **different request name** and hence **does not require the `LS_op` parameter** [§Message Send Request, p.42].

**Request name:** `msg` [§Message Send Request, p.42]

**Targets:**

| Transport | Target |
|---|---|
| HTTP | `POST /lightstreamer/msg.txt?LS_protocol=TLCP-2.5.0` [§Message Send Request, p.43] |
| WS | first line `msg` [§Message Send Request, p.43] |

### 12.1 Parameters

| Parameter | Requirement | Value syntax / constraints | Default when omitted | Meaning |
|---|---|---|---|---|
| `LS_session` | See §5.1 | See §5.1 | See §5.1 | See the Common Parameters section [§Message Send Request, pp.42-43] |
| `LS_reqId` | See §5.1 | See §5.1 | — | See the Common Parameters section [§Message Send Request, pp.42-43] |
| `LS_message` | Required | **Any text string**, interpreted and verified by the Metadata Adapter | — | The message payload; the developer is free to decide its meaning [§Message Send Request, p.43] |
| `LS_sequence` | Optional | An **alphanumeric identifier** (the **underscore** character is also allowed). The special identifier `UNORDERED_MESSAGES` is **reserved and can't be used**; if specified it leads to an error | The message is processed immediately, possibly concurrently with others | Identifies a subset of messages to be managed **in sequence**, based on the assigned progressive numbers [§Message Send Request, p.43] |
| `LS_msg_prog` | Optional; **mandatory whenever `LS_sequence` is specified or `LS_outcome` is set to `true`** | Progressive number within the specified sequence, **starting from 1** | — | Progressive number of the message within the sequence. Also used to identify the message in the `MSGDONE` and `MSGFAIL` notifications, **even if no sequence is specified** [§Message Send Request, p.43] |
| `LS_max_wait` | Optional | Expressed in **milliseconds**; if too high or not specified, the timeout is assigned by the Server based on its own configuration. **Ignored if `LS_sequence` is omitted** | Assigned by the Server | Maximum time the Server can wait before processing the message if one or more of the preceding messages for the same sequence have not been received [§Message Send Request, p.43] |
| `LS_ack` | Optional; **only if the stream connection is on WS transport** | `false` suppresses the `REQOK` response | `true` | If `false` the `REQOK` response is not sent [§Message Send Request, p.43] |
| `LS_outcome` | Optional | `false` suppresses `MSGDONE`/`MSGFAIL` | `true` | If `false` the `MSGDONE` or `MSGFAIL` notification is not sent [§Message Send Request, p.44] |

`LS_sequence` notes, verbatim in substance [§Message Send Request, p.43]:

- All messages associated with the same sequence name that have successfully been sent to the Server are guaranteed to be processed **sequentially**, according to the associated progressive numbers.
- In case a message is received and the messages for all the previous numbers expected haven't been received yet, the latter numbers **can be skipped** according to the timeout specified with the message.
- If omitted, the message is processed immediately, possibly concurrently with others, regardless of any progressive number assigned. Yet, if a progressive number is specified, previous numbers for which no unsequenced message has been received after a server-side timeout may be notified by the Server as **skipped**.
- The special identifier `UNORDERED_MESSAGES`, used in previous text protocols, is now **reserved** and can't be used. If specified it leads to an error.

`LS_msg_prog` notes, verbatim in substance [§Message Send Request, p.43]:

- It is **mandatory whenever `LS_sequence` is specified or `LS_outcome` is set to `true`**.
- Even if `LS_sequence` is omitted and `LS_outcome` is set to `false`, it **can** be specified to enforce checks for duplicates and missing messages (see `LS_sequence`).

`LS_ack` notes, verbatim in substance [§Message Send Request, pp.43-44]:

- Skipping the `REQOK` response may be useful when sending frequent loads of messages with no requirements for processing, e.g. when sending the status of a frequently updated object. A successful message submission is also notified with `MSGDONE` on the stream connection.
- If the request fails, the consequent `REQERR` or `ERROR` response is always sent.
- Ignored if used with HTTP transport, as an HTTP response is always sent.
- Default value is `true`.

`LS_outcome` notes, verbatim in substance [§Message Send Request, p.44]:

- As with the `LS_ack` argument, skipping the `MSGDONE` or `MSGFAIL` notification may be useful when sending frequent loads of messages with no requirements for processing.
- **If both `LS_ack` and `LS_outcome` are `false`, there will be no notification at all for a successful message submission (a "fire and forget" policy).**
- Default value is `true`.

### 12.2 Conditionality and exclusivity

- `LS_sequence` **implies** `LS_msg_prog` [§Message Send Request, p.43].
- `LS_outcome=true` **implies** `LS_msg_prog` [§Message Send Request, p.43].
- `LS_max_wait` is **ignored** unless `LS_sequence` is present [§Message Send Request, p.43].
- `LS_ack` is meaningful only on WS [§Message Send Request, p.43].

⚠️ **Spec unclear:** `LS_msg_prog` is labelled **Optional** yet is stated to be mandatory "whenever `LS_sequence` is specified **or `LS_outcome` is set to `true`**", while `LS_outcome`'s own default value is `true` [p.43-44]. Read literally, that makes `LS_msg_prog` mandatory in the default case, contradicting its "Optional" label — and the spec's own `msg` example does supply `LS_msg_prog`. It is not stated whether the mandatory-when-`LS_outcome`-is-`true` rule applies only when `LS_outcome=true` is written **explicitly**, or also when it holds by default.

### 12.3 Notifications on success

If `LS_outcome` is omitted or set to `true`, a `MSGDONE` or a `MSGFAIL` notification is sent on the stream connection, depending on whether the message is successfully or unsuccessfully delivered, respectively [§Message Send Request, p.44].

### 12.4 Examples

Request example with HTTP transport [p.43]:

```
POST /lightstreamer/msg.txt?LS_protocol=TLCP-2.5.0 HTTP/1.1
Host: push.lightstreamer.com
Accept: */*
Content-Length: 95
Content-Type: text/plain

LS_session=Sd9fce58fb5dbbebfT2255126&LS_reqId=11&LS_sequence=CHAT&LS_msg_prog=1&LS_message=Hello
```

Request example with WS transport [p.43]:

```
msg
LS_reqId=11&LS_sequence=CHAT&LS_msg_prog=1&LS_message=Hello
```

---

## 13. Control Responses

**All control requests, including message send requests, share the same responses** [§Control Responses, p.44].

### 13.1 Successful Control Response

| Property | Value |
|---|---|
| Tag | `REQOK` (short of REQuest OK) |
| Format | `REQOK,<request-ID>` |
| Arguments | 1 |

[§Successful Control Response, p.44]

| Argument | Meaning |
|---|---|
| `<request-ID>` | The ID of the request [p.44] |

Response example [p.44]:

```
REQOK,1
```

### 13.2 Other Control Responses

| Property | Value |
|---|---|
| Tag | `REQERR` (short of REQuest ERRor) |
| Format | `REQERR,<request-ID>,<error-code>,<error-message>` |
| Used when | An error occurred and the request could not be completed |
| Arguments | 3 |

[§Other Control Responses, p.45]

| Argument | Meaning |
|---|---|
| `<request-ID>` | The ID of the request [p.45] |
| `<error-code>` | Error code sent by the Server kernel or by the Metadata Adapter. For a list of supported error codes see **Appendix B** [p.45] |
| `<error-message>` | Error message sent by Server kernel or by the Metadata Adapter. **A message sent by the Metadata Adapter may be empty** [p.45] |

Error response example [p.45]:

```
REQERR,19,Specified subscription not found
```

| Property | Value |
|---|---|
| Tag | `ERROR` |
| Format | `ERROR,<error-code>,<error-message>` |
| Used when | **The request could not be interpreted or processed** |
| Arguments | 2 |

[§Other Control Responses, p.45]

| Argument | Meaning |
|---|---|
| `<error-code>` | Error code sent by the Server kernel. For a list of supported error codes see **Appendix B** [p.45] |
| `<error-message>` | Error message sent by Server kernel [p.45] |

Error response example [p.45]:

```
ERROR,68,Internal error during request processing
```

> Note the structural difference: `REQERR` carries the request ID and is used when the request was understood but failed; `ERROR` carries **no** request ID and is used when the request could not be interpreted or processed at all [§Other Control Responses, p.45].

---

## 14. Client Heartbeat

The Heartbeat is a **pseudo-request** whose sole purpose is keeping the client-server communication channel engaged [§Client Heartbeat, p.45]. Its stated purposes [§Client Heartbeat, pp.45-46]:

1. Preventing the communication infrastructure from closing an inactive socket that is ready for reuse for more HTTP control requests, to avoid connection reestablishment overhead. However it is **not guaranteed** that the connection will be kept open, as the underlying TCP implementation may open a new socket each time an HTTP request needs to be sent.
2. Detecting when a WebSocket connection has been interrupted but not explicitly closed.
3. Allowing the Server to detect cases in which the client is stuck but the socket of the stream connection is kept open by some intermediate node. **This must be done in combination with supplying the `LS_inactivity_millis` parameter to the `create_session` and `bind_session` requests.**

**Any Control Request has the same effect as a Heartbeat**, hence no Heartbeat is needed as long as Control Requests are sent [§Client Heartbeat, p.46]. This request, obviously, is not meant to receive a response [§Client Heartbeat, p.46].

**Request name:** `heartbeat` [§Client Heartbeat, p.46]

**Targets:**

| Transport | Target |
|---|---|
| HTTP | `POST /lightstreamer/heartbeat.txt?LS_protocol=TLCP-2.5.0` [§Client Heartbeat, p.46] |
| WS | first line `heartbeat` [§Client Heartbeat, p.46] |

### 14.1 Parameters

| Parameter | Requirement | Value syntax / constraints | Default when omitted | Meaning |
|---|---|---|---|---|
| `LS_session` | **Optional with WS transport** | The session ID received from the Server at the beginning of the stream connection | With WS: the last session that was bound to the WS connection (it may be currently still bound or temporarily unbound due to a polling cycle), if any | Only needed for purpose 3 above, whereby the client declares that it is still listening to the stream connection for the specified session [§Client Heartbeat, p.46] |

`LS_session` notes, verbatim in substance [§Client Heartbeat, p.46]:

- When omitted with WS transport, it obviously refers to the last session that was bound to the WS connection (it may be currently still bound or temporarily unbound due to a polling cycle), if any.
- With HTTP transport, it may be specified **on the query string**, instead of the request body.

`heartbeat` takes **no** `LS_reqId` and **no** `LS_op` [§Client Heartbeat, p.46].

**Notifications:** No notifications are involved [§Client Heartbeat, p.46].

### 14.2 Examples

Request example with HTTP transport [p.46]:

```
POST /lightstreamer/heartbeat.txt?LS_protocol=TLCP-2.5.0 HTTP/1.1
Host: push.lightstreamer.com
Accept: */*
Content-Length: 36
Content-Type: text/plain

LS_session=Sd9fce58fb5dbbebfT2255126
```

Request example with WS transport [p.46]:

```
heartbeat
LS_session=Sd9fce58fb5dbbebfT2255126
```

or:

```
heartbeat
<empty line>
```

The spec adds: *(note that, since the request here has no parameters, the final line terminator must be included)* [§Client Heartbeat, p.46]. That is, the WS message is `heartbeat` followed by `CR-LF` and an empty parameter line — the general "separator optional after the last line, **unless the line is empty**" rule of p.12 applied.

### 14.3 Responses

**The response is returned only with HTTP transport. With WS transport there is no response, but for the case of an error in the formulation of the request** [§Client Heartbeat, p.46].

**Successful Response** [§Client Heartbeat → Successful Response, pp.46-47]:

| Property | Value |
|---|---|
| Tag | `REQOK` (short of REQuest OK) |
| Format | `REQOK` |

Response example (only with **HTTP** transport) [p.47]:

```
REQOK
```

Note this is a **bare** `REQOK` with **no** `<request-ID>` argument, unlike the control-request `REQOK,<request-ID>` of §13.1 [§Client Heartbeat → Successful Response, p.47; §Successful Control Response, p.44].

**Other Responses** [§Client Heartbeat → Other Responses, p.47]:

| Property | Value |
|---|---|
| Tag | `ERROR` |
| Format | `ERROR,<error-code>,<error-message>` |
| Used when | The request could not be interpreted or processed. **Note that there is no error if the specified session is not found.** |
| Arguments | 2 |

| Argument | Meaning |
|---|---|
| `<error-code>` | Error code sent by the Server kernel. For a list of supported error codes see **Appendix B** [p.47] |
| `<error-message>` | Error message sent by Server kernel [p.47] |

Error response example [p.47]:

```
ERROR,68,Internal error during request processing
```

⚠️ **Spec unclear:** `heartbeat` carries no `LS_reqId`, and its error response is the request-ID-less `ERROR` tag. The spec does not state how a client is to correlate a WS `ERROR` with the specific `heartbeat` that provoked it when other requests are in flight on the same connection.

---

## 15. WebSocket establishment check

A **pseudo-request** whose sole purpose is testing a newly established WebSocket, to ensure that the communication channel is working with no interference from intermediate nodes. **It consists in the Server simply echoing the request** [§WebSocket establishment check, p.47].

- The request is **only allowed on a WebSocket** and it is typically expected to be the **first one issued on the WebSocket** [§WebSocket establishment check, p.47].
- The Server will send the response as soon as possible and, in particular, **the response is guaranteed to precede the responses of any other subsequent requests** [§WebSocket establishment check, p.47].

**Request name:** `wsok` [§WebSocket establishment check, p.47]

**Parameters:** **No parameters are involved** [§WebSocket establishment check, p.47].

**Notifications:** No notifications are involved [§WebSocket establishment check, p.47].

**Transports:** WS only [§WebSocket establishment check, p.47].

### 15.1 Example

Request example with WS transport [p.47]:

```
wsok
```

The spec adds: *Note that, not only does the request require no parameters, but also the parameter line, needed with other types of requests (and possibly empty), is missing in this case* [§WebSocket establishment check, p.47]. This is the one request whose framing deviates from the general WS two-line form of p.12: `wsok` has **no** parameter line at all, whereas `heartbeat` with no parameters still requires an empty one [§Client Heartbeat, p.46].

### 15.2 Successful Response

| Property | Value |
|---|---|
| Tag | `WSOK` (short of WebSocket OK) |
| Format | `WSOK` |

[§WebSocket establishment check → Successful Response, p.48]

Response example [p.48]:

```
WSOK
```

### 15.3 Other Responses

**There are no error responses** [§WebSocket establishment check → Other Responses, p.48].

---

## 16. Quick index of request names

| Request name | HTTP path | WS first line | Response tags (success / failure) | Section |
|---|---|---|---|---|
| `create_session` | `/lightstreamer/create_session.txt` | `create_session` | `CONOK` / `CONERR` | §2, §4 |
| `bind_session` | `/lightstreamer/bind_session.txt` | `bind_session` | `CONOK` / `CONERR`, `END` | §3, §4 |
| `control` (`LS_op=add`) | `/lightstreamer/control.txt` | `control` | `REQOK,<request-ID>` / `REQERR`, `ERROR` | §6, §13 |
| `control` (`LS_op=delete`) | `/lightstreamer/control.txt` | `control` | `REQOK,<request-ID>` / `REQERR`, `ERROR` | §7, §13 |
| `control` (`LS_op=reconf`) | `/lightstreamer/control.txt` | `control` | `REQOK,<request-ID>` / `REQERR`, `ERROR` | §8, §13 |
| `control` (`LS_op=constrain`) | `/lightstreamer/control.txt` | `control` | `REQOK,<request-ID>` / `REQERR`, `ERROR` | §9, §13 |
| `control` (`LS_op=force_rebind`) | `/lightstreamer/control.txt` | `control` | `REQOK,<request-ID>` / `REQERR`, `ERROR` | §10, §13 |
| `control` (`LS_op=destroy`) | `/lightstreamer/control.txt` | `control` | `REQOK,<request-ID>` / `REQERR`, `ERROR` | §11, §13 |
| `msg` | `/lightstreamer/msg.txt` | `msg` | `REQOK,<request-ID>` / `REQERR`, `ERROR` | §12, §13 |
| `heartbeat` | `/lightstreamer/heartbeat.txt` | `heartbeat` | bare `REQOK` (HTTP only) / `ERROR` | §14 |
| `wsok` | — (WS only) | `wsok` | `WSOK` / none | §15 |

All HTTP paths require the query-string parameter `LS_protocol=TLCP-2.5.0` and optionally accept `&LS_session=<session-ID>` [§Request Syntax, p.11; §Request/Response Reference, p.22].

---

## 17. Index of flagged ambiguities

| # | Location | Summary |
|---|---|---|
| 1 | §1.3 (pp.24, 31, 43) | `Content-Length` values in three HTTP examples do not match the shown body lengths |
| 2 | §3.2 (pp.24, 26) | `LS_reduce_head` suppresses a different notification set on `create_session` vs `bind_session`; persistence across binds unstated |
| 3 | §3.2 (pp.23-26) | Units stated for `create_session` timing/length parameters are omitted from the corresponding `bind_session` entries |
| 4 | §4.2 (p.28) | `END` declared with "Arguments: 1" but format and example carry two |
| 5 | §5.2 (pp.31-33, 34-36) | Whether `LS_ack` is accepted on `reconf`, `constrain`, `force_rebind`, `destroy` is unstated |
| 6 | §6.4 (p.31) | HTTP multiple-request example printed with a missing `&` between `LS_reqId` and `LS_op`, contradicting the WS counterpart and the p.11 syntax rule |
| 7 | §11.2 (pp.35-36) | `LS_close_socket` default worded as "may be left open" (`force_rebind`) vs "is left open" (`destroy`) |
| 8 | §12.2 (pp.43-44) | `LS_msg_prog` labelled Optional but mandatory when `LS_outcome` is `true`, whose default is `true` |
| 9 | §14.3 (pp.46-47) | No stated correlation mechanism between a WS `heartbeat` `ERROR` and the request that caused it |
