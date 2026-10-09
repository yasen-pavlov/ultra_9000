//! Platform integration replacing winit's window backend.
//!
//! Each platform provides a terminal view and an `embedder` module with the native pieces of
//! the platform independent session registry in [`crate::embed`].

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "linux")]
pub use linux::embedder;
#[cfg(target_os = "macos")]
pub use macos::embedder;
