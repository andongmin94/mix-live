//! S1 device path. The clock bridge is portable; only `windows` opens devices.
//! No recording, default-device substitution, ASIO SDK or virtual driver installation.

pub mod bridge;
pub mod capture;
pub mod config;
pub mod control;
pub mod scene;
#[cfg(windows)]
pub mod windows;
