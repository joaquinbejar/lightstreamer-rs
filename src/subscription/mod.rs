//! Subscription state: what the client knows about the items it subscribed to.
//!
//! Like [`crate::protocol`], this module performs no I/O. It holds the item
//! state that a real-time update is applied to, and the decoding of the
//! second-level field-value syntax.
//!
//! Source: `docs/spec/04-notifications.md` §2.

// Consumed by the session layer, which does not exist yet
// (see `docs/SPEC.md`, implementation order steps 4-5).
#![allow(dead_code)]

pub(crate) mod update;
