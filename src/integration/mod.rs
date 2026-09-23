//! Integrations: the things the taskbar widget can show.
//!
//! The widget itself knows nothing about Discord, or about any other source.
//! An integration owns its own state and background work, and describes what
//! it wants drawn as a list of generic [`crate::ui::block::Block`]s. Adding a
//! second source means adding a module here, not editing the widget.

pub mod discord;
