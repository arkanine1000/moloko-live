//! Scene packs written by molokolive-pack (manifest.json, version 1), and their playback state.
//!
//! - manifest: the files on disk and how they load
//! - scene: the loaded layers and how they composite
//! - timeline: animation steps and drift, and when each is due

mod manifest;
mod scene;
mod timeline;

pub use manifest::{load, summary};
pub use scene::Scene;
