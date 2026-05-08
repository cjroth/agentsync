pub mod protocol;

#[cfg(not(target_arch = "wasm32"))]
pub mod client;
#[cfg(not(target_arch = "wasm32"))]
pub mod server;
#[cfg(not(target_arch = "wasm32"))]
pub mod transport;

pub use protocol::*;

#[cfg(not(target_arch = "wasm32"))]
pub use client::{discover_vault_id, ClientConn};
#[cfg(not(target_arch = "wasm32"))]
pub use server::{Server, ServerTls};
