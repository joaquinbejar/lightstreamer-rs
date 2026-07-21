//! Subscription state: what the client knows about the items it subscribed to.
//!
//! Like [`crate::protocol`], this module performs no I/O. It holds the item
//! state that a real-time update is applied to, the decoding of the
//! second-level field-value syntax, and the per-subscription bookkeeping that
//! turns server notifications into the events a caller consumes.
//!
//! - [`update`] — the normative decoding of a `U` value list onto an item's
//!   previous field values [`docs/spec/04-notifications.md` §2.2, §2.3].
//! - [`manager`] — one subscription's lifecycle and item state: activation,
//!   snapshot classification, end of snapshot, clearing, overflow,
//!   reconfiguration and unsubscription
//!   [`docs/spec/04-notifications.md` §2.4, §3].
//! - [`item_update`] — [`ItemUpdate`](item_update::ItemUpdate), the public type
//!   the caller receives for every update.
//!
//! Source: `docs/spec/04-notifications.md` §2, §3.

// Consumed by the session layer and the client façade, which do not consume it
// yet (see `docs/SPEC.md`, implementation order steps 4-5).
#![allow(dead_code)]

pub(crate) mod item_update;
pub(crate) mod manager;
pub(crate) mod update;
