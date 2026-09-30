//! A widget for the Windows taskbar.
//!
//! Windows, drawing, layout, menus, the tray icon, the settings window and
//! the diagnostics window. It knows nothing about what it is displaying:
//! everything on screen is described to it as [`ui::block::Block`]s by
//! something implementing [`ui::integration::Integration`].
//!
//! See `ui::integration` for the whole of that contract, and the
//! `discord-integration` crate for a worked example.

pub mod assets;
pub mod config;
pub mod http;
pub mod ui;
pub mod update;
