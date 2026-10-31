#![cfg(target_os = "android")]
//! The JNI bridge between PhotoCraft's Android shell and its `MainActivity`.
//!
//! The Android app crate (`apps/photocraft-android`) is safe Rust; all pointer work lives here.
//! This is the second isolated `unsafe` helper crate, after `photocraft-tablet` (see AGENTS.md,
//! Never crash): JNI gives the process raw `JavaVM` and `jobject` handles, and the `jni` crate's
//! constructors for them are `unsafe`. The private `bridge` module allows `unsafe` with a
//! `SAFETY:` comment on every block; the public functions below are safe and return
//! `Result<_, String>`.
//!
//! Every call goes through an existing method on the activity instance that `android-activity`
//! publishes through `ndk-context`. That avoids `FindClass` on a native thread (which uses the
//! bootstrap class loader and cannot see the app's classes) and avoids caching class references.
//!
//! Picker results are polled rather than pushed back through a native callback: the Java side
//! stores the result and [`take_result`] drains it once per frame. That keeps this crate free of
//! exported JNI functions and their name mangling.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod bridge;

pub use bridge::{create_document, document_name, files_dir, init, open_documents, open_url, read_all, take_result, write_all};
