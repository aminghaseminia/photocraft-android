#![cfg(target_os = "android")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
//! PhotoCraft for Android.
//!
//! A thin shell, not a second editor: [`photocraft_ui_egui::PhotocraftApp`] and the engine are the
//! same code the desktop and web builds run. This crate supplies only what Android needs —
//! `android_main`, an eframe window on GameActivity, and a [`Services`](photocraft_ui_egui::Services)
//! implementation backed by the Storage Access Framework (see `services.rs`).
//!
//! Build and packaging live in `packaging/android`; see its README.
//!
//! Known gaps in this first cut (all documented in `docs/android-port.md`):
//! - autosaves are not flushed when Android pauses the app (they are written periodically);
//! - no image clipboard (egui still copies and pastes text within the app);
//! - folder pickers, printing, update checks and the control/MCP server are absent;
//! - files are read and written through `content://` URIs, not paths, so linked smart objects
//!   that name sibling files cannot be resolved.

mod entry;
mod services;
