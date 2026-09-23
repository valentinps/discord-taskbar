//! Discord voice status.
//!
//! The reference integration: it reads voice state over Discord's local RPC
//! pipe and renders it as blocks the widget draws.

pub mod icons;
pub mod view;

/// Where Discord serves avatars and server icons.
pub const CDN_HOST: &str = "cdn.discordapp.com";
