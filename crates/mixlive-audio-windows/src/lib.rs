//! Device path and bounded clock transport. Only `windows` opens devices.
//! No recording, default-device substitution, ASIO SDK or virtual driver installation.

pub mod bridge;
pub mod capture;
pub mod config;
pub mod control;
pub mod devices;
pub mod plugin;
pub mod scene;
#[cfg(windows)]
pub mod windows;
