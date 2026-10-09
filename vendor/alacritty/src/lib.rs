//! Alacritty's display, renderer, input and event processing hosted in embedder-owned native
//! views, one independent terminal session per view: AppKit `NSView`s on macOS, GTK input
//! widgets with Wayland subsurfaces on Linux.
//!
//! This crate is a modified copy of the `alacritty` binary crate at
//! def47a5dd511ed20607eb5d9e5e7140afd16b31d (see `vendor/PROVENANCE.md`). The upstream winit
//! `EventLoop`/`Window` are replaced by [`host`] types and the [`platform`] view; the
//! terminal core (`alacritty_terminal`), renderer, shaders, glyph cache, bindings and input
//! algorithms are the upstream ones.
//!
//! All functions in [`embed`] must be called on the main (AppKit or GTK) thread.

#![warn(rust_2018_idioms, future_incompatible)]
#![allow(unused_unsafe)]

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
compile_error!("alacritty_embed supports macOS and Linux (Wayland) only");

mod clipboard;
pub mod config;
mod daemon;
mod display;
pub mod embed;
mod event;
mod host;
mod input;
#[cfg(target_os = "macos")]
mod macos;
mod platform;
mod renderer;
mod scheduler;
mod string;
mod window_context;

mod gl {
    #![allow(clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/gl_bindings.rs"));
}

#[cfg(target_os = "macos")]
pub use crate::macos::locale;
