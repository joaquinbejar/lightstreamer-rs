/******************************************************************************
    Author: Joaquín Béjar García
    Email: jb@taunais.com 
    Date: 16/5/25
 ******************************************************************************/
 
pub mod error;
mod proxy;
mod util;

mod logger;

pub use proxy::Proxy;
pub(crate) use util::{clean_message,parse_arguments};
pub use error::{IllegalArgumentException, IllegalStateException};
pub use logger::{setup_logger, setup_logger_with_level};