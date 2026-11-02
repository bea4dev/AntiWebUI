//! Links all of AntiWebUI into a single Rust dynamic library to speed up incremental builds
//! during development. Enabled through the `dynamic_linking` feature of `anti-webui`.
//!
//! A Rust `dylib` has no stable ABI: the application and this library must be built by the same
//! compiler with the same flags. Do not ship it; use static linking for releases.

#![allow(unused_imports)]
pub use anti_webui_internal::*;
