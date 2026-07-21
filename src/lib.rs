//! A Rust client for **TLCP**, the Text Lightstreamer Client Protocol.
//!
//! This crate implements TLCP 2.5.0 as published by Lightstreamer. It is a
//! protocol client and nothing more: it opens a session, keeps it alive,
//! delivers real-time updates, and gets out of the way.
//!
//! # Provenance
//!
//! Version 1.0.0 onwards is an independently authored implementation licensed
//! under MIT. Versions up to and including 0.3.3 were GPL-3.0-only and are
//! unrelated to this code; they remain published, under that licence, as
//! historical versions.
//!
//! Every wire behavior in this crate is derived from the official
//! specification and cites the chapter of `docs/spec/` it comes from. See
//! `docs/adr/0001-mit-relicensing-clean-room.md`.
//!
//! # Status
//!
//! Under construction. The pure protocol layer lands first; the transports and
//! the public client façade follow. See `docs/SPEC.md` for the implementation
//! order.

#![forbid(unsafe_code)]

pub mod error;

mod protocol;
mod session;
mod subscription;
mod transport;

pub use error::{Error, Result};
pub use transport::TransportError;
