//! A widget for the Windows taskbar, and the things it can show.
//!
//! `ui` is the widget: windows, drawing, layout, menus, the tray icon and the
//! settings. It knows nothing about what it is displaying. `integration` holds
//! the sources that do — today just Discord.

pub mod assets;
pub mod config;
pub mod http;
pub mod integration;
pub mod ui;
