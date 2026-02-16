//! D-Bus interface for IBus integration on Linux/GNOME
//!
//! This module provides a D-Bus server that allows external applications
//! (like the handy-ibus IBus engine) to control Handy's transcription
//! functionality.

mod server;

pub use server::{start_dbus_server, stop_dbus_server, HandyDbusState, HandyState};
