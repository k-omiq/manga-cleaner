//! The clean pipeline: ingest, detect, gate, mask fitting, engines, the
//! decline metric, composite.
//!
//! Pure Rust with no Tauri dependency, so every stage is testable without a
//! webview. The Tauri adapter in `src-tauri` is the only thing that knows about
//! commands, channels and the `tile://` protocol.

pub mod accel;
pub mod balloon;
pub mod composite;
pub mod constants;
pub mod detect;
pub mod engines;
pub mod export;
pub mod fit;
pub mod gate;
pub mod image;
pub mod ingest;
pub mod mask;
pub mod memory;
pub mod paint;
pub mod patch;
pub mod project;
pub mod quality;
pub mod registry;
pub mod residency;
pub mod runtime;
pub mod sidecar;
pub mod strip;
