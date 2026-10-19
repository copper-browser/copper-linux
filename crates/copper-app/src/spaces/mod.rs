//! Spaces in the window: the header, chips, menu, editor and delete dialog
//! ([`ui`]). Colour, tint, marks and names are the shell's
//! ([`copper_shell_core::spaces`]), re-exported here so the widgets keep
//! one path for both.

pub mod ui;

pub use copper_shell_core::spaces::{colour, emoji, fresh_name, mark};
