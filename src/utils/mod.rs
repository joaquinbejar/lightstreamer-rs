/******************************************************************************
    Author: Joaquín Béjar García
    Email: jb@taunais.com 
    Date: 16/5/25
 ******************************************************************************/
 
pub mod error;
mod proxy;
mod util;

pub use proxy::Proxy;
pub(crate) use util::{clean_message,parse_arguments};
pub use error::{IllegalArgumentException, IllegalStateException};