//! bekdesk core: identity, pairing, discovery, secure channel and engine orchestration.
//! The Tauri shell (main.rs) is a thin UI layer over this crate so the core can be tested headless.

pub mod app;
pub mod clip;
pub mod discovery;
pub mod display;
pub mod engines;
pub mod identity;
pub mod modes;
pub mod net;

pub use app::App;
