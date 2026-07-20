# MPN (Mobile Push Notifications) — Scope Note and Survey

Scope: this chapter surveys the Mobile Push Notifications (MPN) portion of the **TLCP Specification, version 2.5.0** (Text Lightstreamer Client Protocol; target: Lightstreamer Server v. 7.4.0 or greater; last updated 19/3/2024) — the five MPN Control Requests on printed pages 36–42 and the five MPN-Related Notifications on printed pages 57–59. It is deliberately a *survey*, not a parameter reference: each request and notification gets its tag or target, a short summary, a page reference, and a note on what the rest of the protocol it would depend on. Every factual statement carries a citation of the form `[§Section Name, p.NN]` with the **printed** page number. Wire tokens are reproduced verbatim in backticks.

---

## 1. Out of scope for v1

**MPN is out of scope for the v1 implementation of this crate.** No MPN control request will be issuable and no MPN notification will be modelled beyond, at most, being recognised and ignored on the stream connection.

The spec itself flags MPN as conditional functionality, twice, in identical wording:

> Edition Note: MPN is an optional feature, available depending on Edition and License Type. To know what features are enabled by your license, please see the License tab of the Monitoring Dashboard (by default, available at `/dashboard`).

`[§MPN Control Requests, p.36]` and `[§MPN-Related Notifications, p.57]`.

The Server can also refuse MPN outright at runtime: Appendix B code `40` is "The MPN Module is disabled, either by configuration or by license restrictions" `[§Appendix B, p.92]`. See [05-error-codes.md](05-error-codes.md) §3.7 for the full set of MPN error codes (`40`–`53` and `56`).

MPN also carries a **server-side deployment prerequisite** unrelated to the wire protocol: the module must be enabled in `conf/lightstreamer_conf.xml` and "An SQL database must be setup and configured appropriately in the Server's Hibernate configuration file `conf/mpn/hibernate.cfg.xml`" `[§Additional Configuration, p.81]`. Additionally, "for security purposes, the Server only accepts MPN requests for applications it knows", so the target application must be registered in the Server's MPN provider configuration file `[§Additional Configuration, p.82]`.

---

## 2. What MPN is, per the spec

"Mobile Push Notifications (MPN) requests are a subset of control requests dedicated to the interaction with the MPN Module, a component of the Server that can route real-time updates to mobile devices via their native push notification service (e.g. APNs for Apple™ platforms and FCM for Google™ platforms)." `[§MPN Control Requests, p.36]`

Two platform values are defined: `Apple` "for Apple™ platforms, suchs as iOS™, macOS™ and tvOS™" and `Google` "for Google™ platforms, such as Android™", with the note "No other platforms are supported at time of writing" `[§MPN Device Registration Request, pp.36–37]`.

The spec repeatedly defers the conceptual model to an external document: "See Chapter 5 of the General Concepts document for an in-depth introduction to the MPN Module" `[§MPN Control Requests, p.36]`, `[§MPN-Related Notifications, p.57]`.

⚠️ **Spec unclear:** substantial parts of the MPN semantics are not in TLCP 2.5.0 at all. The spec defers to the *General Concepts* document for: application-ID configuration (§5.5), device-token change handling (§5.1.1), argument expansion in the notification format (§5.2.1), trigger-expression argument expansion (§5.2.2), coalescing semantics (§5.2.3), the internal MPN Data Adapter's items ("The Internal MPN Data Adapter"), and the end-to-end workflow diagram (§5.6) `[§MPN Device Registration Request, p.37]`, `[§MPN Subscription Activation, pp.39–40]`, `[§MPN Device Registered, p.58]`, `[§Mobile Push Notifications, p.81]`. A complete MPN implementation cannot be built from this specification alone.

---

## 3. MPN Control Requests (pp. 36–42)

All five are ordinary control requests: request name `control`, distinguished by `LS_op`, and sharing the common parameters `LS_session` and `LS_reqId` `[§MPN Control Requests, pp.36–42]`, `[§Common Parameters, p.29]`. They therefore return the standard `REQOK` / `REQERR` / `ERROR` responses described in `[§Control Responses, pp.44–45]`; each MPN request section ends with "See Control Responses section in this chapter for sample responses" or an equivalent pointer `[§MPN Device Registration Request, p.37]`.

| # | Request | `LS_op` | Success notification | Page |
| --- | --- | --- | --- | --- |
| 1 | MPN Device Registration Request | `register` | `MPNREG` | pp.36–37 |
| 2 | MPN Device Badge Reset | `reset_badge` | `MPNZERO` | pp.37–38 |
| 3 | MPN Subscription Activation | `activate` | `MPNOK` | pp.38–40 |
| 4 | MPN Subscription Reconfiguration | `pn_reconf` | `MPNCONF` | pp.40–42 |
| 5 | MPN Subscription Deactivation | `deactivate` | `MPNDEL` (one per deactivated subscription) | p.42 |

### 3.1 MPN Device Registration Request — `LS_op=register` `[pp.36–37]`

Registers a new or existing MPN device with the Server, identified by platform type (`PN_type`), application ID (`PN_appId`) and the platform's device token (`PN_deviceToken`, with `PN_newDeviceToken` for token rotation). "Registering an MPN device is a mandatory operation to enable any subsequent MPN request on the session. Registering the same device multiple times, on different sessions or even on the same one, poses no issues." `[§MPN Device Registration Request, p.36]`

*Later-support dependencies:* the control-request framing (`control` request name, `LS_session`, `LS_reqId`) and the `REQOK`/`REQERR`/`ERROR` response handling; plus stream-notification dispatch to receive `MPNREG`. Nothing else in the protocol is a prerequisite — this is the entry point for every other MPN operation.

### 3.2 MPN Device Badge Reset — `LS_op=reset_badge` `[pp.37–38]`

Resets the badge counter of a previously registered MPN device, addressed by `PN_deviceId` — which "Must be the same ID notified by `MPNREG` after a successful registration" `[§MPN Device Badge Reset, p.38]`. "For the MPN device ID to be accepted, the device must have been registered on the current session." `[§MPN Device Badge Reset, p.38]`

*Later-support dependencies:* §3.1 (a device ID obtained from `MPNREG` **on the current session**), plus stream-notification dispatch for `MPNZERO`.

### 3.3 MPN Subscription Activation — `LS_op=activate` `[pp.38–40]`

Activates a new MPN subscription or modifies an existing one, so that subscription updates are turned into native push notifications. It reuses the data-subscription parameter vocabulary — `LS_subId`, `LS_data_adapter`, `LS_group`, `LS_schema`, `LS_mode` (here restricted to "`MERGE` and `DISTINCT`"), `LS_requested_buffer_size`, `LS_requested_max_frequency` — and adds `PN_deviceId`, the optional `PN_subscriptionId`, the mandatory `PN_notificationFormat` (a JSON structure), the optional `PN_trigger` (a Java boolean expression), and the optional `PN_coalescing` flag `[§MPN Subscription Activation, pp.38–40]`.

The spec is emphatic that two distinct identifiers are in play: `LS_subId` is "a transient, client-assigned progressive integer […] its scope is limited to the current session", while `PN_subscriptionId` is "a persistent, Server-assigned 36-characters UUID […] Its scope is beyond the session life-cycle" `[§MPN Subscription Activation, pp.38–39]`.

*Later-support dependencies:* §3.1; the full subscription-request parameter model (this request is a superset of it); the `TRIGGERED` MPN-subscription state, which is referenced here but defined in *General Concepts*; and stream-notification dispatch for `MPNOK`.

### 3.4 MPN Subscription Reconfiguration — `LS_op=pn_reconf` `[pp.40–42]`

Changes an existing MPN subscription "but only in the notification part, without modifying the subscription part" `[§MPN Subscription Reconfiguration, p.40]`. The notification parameters (max frequency, notification format, trigger expression) may each be omitted to leave them unchanged; the subscription parameters (Data Adapter, Group, Schema, Mode) "have to be specified, because they have to be consistent with the notification parameters. In case they don't match the current MPN subscription, the request will be refused" `[§MPN Subscription Reconfiguration, p.41]`. Reconfiguring always reactivates a subscription in `TRIGGERED` state, "even if no notification parameter is specified" `[§MPN Subscription Reconfiguration, p.41]`. Supplying an empty `PN_trigger` "removes any trigger in use" `[§MPN Subscription Reconfiguration, p.42]`.

*Later-support dependencies:* §3.1 and §3.3 (a `PN_subscriptionId` obtained from `MPNOK`); the same subscription-parameter model as §3.3; stream-notification dispatch for `MPNCONF`. Mismatched subscription parameters surface as Appendix B code `56` — "The subscription parameters specified don't match with the specified MPN subscription" `[§Appendix B, p.93]`.

### 3.5 MPN Subscription Deactivation — `LS_op=deactivate` `[p.42]`

Deactivates one, several, or all MPN subscriptions of a device. Selection is by `PN_subscriptionId` (a single subscription), by `PN_subscriptionStatus` (one of `ACTIVE` or `TRIGGERED`, deactivating all matching subscriptions), or — "If omitted and `PN_subscriptionStatus` is also omitted, all MPN subscriptions of the MPN device are deactivated" `[§MPN Subscription Deactivation, p.42]`. On success "an `MPNDEL` notification is sent on the stream connection **for each** deactivated MPN subscription" `[§MPN Subscription Deactivation, p.42]`.

*Later-support dependencies:* §3.1, plus stream-notification dispatch able to correlate a **variable number** of `MPNDEL` notifications with a single request — unlike every other MPN request, which yields exactly one notification.

⚠️ **Spec unclear:** with both `PN_subscriptionId` and `PN_subscriptionStatus` omitted, the request may produce an unbounded number of `MPNDEL` notifications, and the spec gives no way to know how many to expect, nor any terminator marking the end of the batch `[§MPN Subscription Deactivation, p.42]`.

⚠️ **Spec unclear:** the spec does not state what happens if **both** `PN_subscriptionId` and `PN_subscriptionStatus` are supplied `[§MPN Subscription Deactivation, p.42]`.

---

## 4. MPN-Related Notifications (pp. 57–59)

"The following notifications provide identifiers and status of MPN Module entities, such as MPN devices and MPN subscriptions." `[§MPN-Related Notifications, p.57]`

All five are **success-only** notifications: none carries an error code, and failures of MPN requests are reported through the ordinary control-error channel (`REQERR` / `ERROR` with an Appendix B code) `[§MPN-Related Notifications, pp.57–59]`, `[§Appendix B, p.91]`.

| # | Tag | Format | Meaning | Page |
| --- | --- | --- | --- | --- |
| 1 | `MPNREG` | `MPNREG,<MPN-device-ID>,<MPN-data-adapter-name>` | An MPN device has been successfully registered | pp.57–58 |
| 2 | `MPNZERO` | `MPNZERO,<MPN-device-ID>` | An MPN device had its badge successfully reset | p.58 |
| 3 | `MPNOK` | `MPNOK,<subscription-ID>,<MPN-subscription-ID>` | An MPN subscription has been successfully activated | pp.58–59 |
| 4 | `MPNCONF` | `MPNCONF,<MPN-subscription-ID>` | A previous MPN-subscription reconfiguration request has been accomplished | p.59 |
| 5 | `MPNDEL` | `MPNDEL,<MPN-subscription-ID>` | An MPN subscription has been deleted | p.59 |

### 4.1 `MPNREG` — MPN Device Registered `[pp.57–58]`

Reports the Server-assigned MPN device ID ("formatted as a 36-characters UUID") **and** the name of an internal Data Adapter: "an internal Data Adapter always made available in the session's Adapter Set when the MPN Module is enabled. This Data Adapter supplies special items that provide information for monitoring the status of the MPN device and its MPN subscriptions." `[§MPN Device Registered, pp.57–58]` After this notification "the MPN device may be addressed for subsequent requests on the current session, such as MPN subscription activation and badge reset" `[§MPN Device Registered, p.58]`.

Spec example, verbatim `[§MPN Device Registered, p.58]`:

```
MPNREG,8c1eaa79-acd1-4cb4-b2a7-3b62bb449798,MPN_INTERNAL_DATA_ADAPTER
```

*Later-support dependencies:* this is the notification that unlocks everything else. Beyond parsing it, exploiting the second argument requires the ordinary **data-subscription** machinery, because monitoring MPN state means subscribing to items of that internal Data Adapter — the item names are defined only in *General Concepts* `[§MPN Device Registered, p.58]`.

### 4.2 `MPNZERO` — MPN Device Badge Reset `[p.58]`

Confirms the badge reset of the identified MPN device `[§MPN Device Badge Reset, p.58]`.

Spec example, verbatim `[§MPN Device Badge Reset, p.58]`:

```
MPNZERO,8c1eaa79-acd1-4cb4-b2a7-3b62bb449798
```

*Later-support dependencies:* §3.2 only.

### 4.3 `MPNOK` — MPN Subscription Activated `[pp.58–59]`

Correlates the client-assigned `<subscription-ID>` ("assigned by the client and is a progressive integer starting with 1") with the Server-assigned `<MPN-subscription-ID>` ("formatted as a 36-characters UUID") `[§MPN Subscription Activated, p.58]`. "After this notification, native push notifications start to be sent to the mobile device." `[§MPN Subscription Activated, p.58]`

Spec example, verbatim `[§MPN Subscription Activated, p.59]`:

```
MPNOK,3,af9fbcf1-7d03-4bb5-9712-59a28d12d22a
```

*Later-support dependencies:* §3.3; plus persistence of the returned UUID across sessions, since it is the only handle for later reconfiguration or deactivation.

### 4.4 `MPNCONF` — MPN Subscription Reconfigured `[p.59]`

Acknowledges a reconfiguration. "Differently from the `CONF` notification for data subscriptions, this notification does not specify the new parameters. For this kind of information you should rely on the dedicated internal Data Adapter, whose name was received upon registration" `[§MPN Subscription Reconfigured, p.59]`. If the MPN subscription was in `TRIGGERED` state, push notifications "will be resumed" `[§MPN Subscription Reconfigured, p.59]`.

The spec's own example `[§MPN Subscription Reconfigured, p.59]`:

```
MPNCONF,,af9fbcf1-7d03-4bb5-9712-59a28d12d22a
```

⚠️ **Spec unclear:** the declared format is `MPNCONF,<MPN-subscription-ID>` with "Arguments: 1", yet the printed example shows **two** commas — an empty argument before the UUID `[§MPN Subscription Reconfigured, p.59]`. The example and the format declaration contradict each other, and the spec does not say which is correct.

*Later-support dependencies:* §3.4; and, to learn the resulting parameters, the internal MPN Data Adapter of §4.1 — i.e. the full data-subscription path.

### 4.5 `MPNDEL` — MPN Subscription Deactivated `[p.59]`

"The `MPNDEL` notification reports that the MPN subscription has been deleted." `[§MPN Subscription Deactivated, p.59]` "After this notification, no more native push notifications will be sent to the mobile device." `[§MPN Subscription Deactivated, p.59]`

Spec example, verbatim `[§MPN Subscription Deactivated, p.59]`:

```
MPNDEL,af9fbcf1-7d03-4bb5-9712-59a28d12d22a
```

⚠️ **Spec unclear:** the "Used when" line for `MPNDEL` reads "to notify that an MPN subscription has been succesfully **activated**" `[§MPN Subscription Deactivated, p.59]` — evidently a copy-paste from `MPNOK`, since the surrounding prose, the tag expansion ("MPN subscription DELeted"), and the request that triggers it all describe deactivation.

---

## 5. What supporting MPN later would take

Collecting the dependencies noted above, adding MPN after v1 would require, in this order:

1. **Nothing new in the transport or framing layer.** MPN requests are plain control requests (`control` + `LS_op`) and MPN notifications are plain comma-separated notification lines under the common syntax `[§MPN Control Requests, p.36]`, `[§Common Response and Notification Syntax, p.13]`. Parsing them costs five more tags and five more `LS_op` values.
2. **The existing control-response path**, unchanged: `REQOK` / `REQERR` / `ERROR`, with the Appendix B codes `40`–`53` and `56` becoming reachable `[§Control Responses, pp.44–45]`, `[§Appendix B, pp.92–93]`.
3. **Session-scoped device registration state**, because every MPN request other than `register` requires a device ID that "must have been registered on the current session" `[§MPN Device Badge Reset, p.38]`, `[§MPN Subscription Activation, p.38]`, `[§MPN Subscription Reconfiguration, p.41]`, `[§MPN Subscription Deactivation, p.42]`. A new session means re-registering before any MPN request.
4. **Cross-session persistence of MPN subscription UUIDs**, since `PN_subscriptionId` has a scope "beyond the session life-cycle" `[§MPN Subscription Activation, p.39]` and is the only way to address an existing MPN subscription later.
5. **The full data-subscription path**, if MPN state is to be observable: status monitoring goes through the internal MPN Data Adapter named in `MPNREG`, i.e. through ordinary item subscriptions `[§MPN Device Registered, p.58]`, `[§MPN Subscription Reconfigured, p.59]`.
6. **Correlation logic tolerant of one-request-to-many-notifications**, for the deactivate-all form of §3.5 `[§MPN Subscription Deactivation, p.42]`.
7. **The external *General Concepts* document**, for everything the TLCP spec defers: notification-format and trigger argument expansion, coalescing rules, the `ACTIVE`/`TRIGGERED` state machine, device-token rotation, and the internal Data Adapter's item names (see the ⚠️ in §2).

---

## 6. Collected ambiguities

1. **§2** — Large parts of MPN semantics are not specified in TLCP 2.5.0 and are deferred to Chapter 5 of the *General Concepts* document; MPN cannot be implemented from this spec alone.
2. **§3.5** — The deactivate-all form yields an unbounded number of `MPNDEL` notifications with no stated count and no batch terminator `[p.42]`.
3. **§3.5** — Behaviour when both `PN_subscriptionId` and `PN_subscriptionStatus` are supplied is not stated `[p.42]`.
4. **§4.4** — The `MPNCONF` example (`MPNCONF,,<UUID>`) contradicts the declared single-argument format `[p.59]`.
5. **§4.5** — The `MPNDEL` "Used when" line says "activated" where it plainly means deactivated `[p.59]`.
