/******************************************************************************
   Author: Joaquín Béjar García
   Email: jb@taunais.com
   Date: 16/5/25
******************************************************************************/

/// Module containing custom error types used throughout the library.
///
/// This module provides specialized error types for handling different error scenarios,
/// such as illegal arguments and illegal states.
pub mod error;
mod proxy;
mod util;

mod logger;

pub use error::{IllegalArgumentException, IllegalStateException};
pub use logger::{setup_logger, setup_logger_with_level};
pub use proxy::Proxy;
pub(crate) use util::{clean_message, parse_arguments};
