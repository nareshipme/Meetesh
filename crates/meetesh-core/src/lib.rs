//! Meetesh core: everything that must work on desktop and, later, mobile.
//!
//! Rule (NFR-22): no windowing, no OS-specific audio, no Tauri here.
//! Platform specifics live behind traits in the shell crates.

pub mod asr;
pub mod assist;
pub mod audio;
pub mod db;
pub mod error;
pub mod export;
pub mod models;
pub mod online;
pub mod permissions;

pub use asr::{Source, TranscriptSegment, Transcriber};
pub use db::{Db, Meeting};
pub use error::{Error, Result};
