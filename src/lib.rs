//! talker — lokaler macOS-Diktier-Assistent (Bibliotheks-Teil).
//! Architektur & Entscheidungen: siehe `docs/adr/` und `CONTEXT.md`.

pub mod audio;
pub mod cleanup;
pub mod clipboard;
pub mod config;
pub mod error;
pub mod hotkey;
pub mod indicator;
pub mod injection;
pub mod login_item;
pub mod models;
pub mod overlay;
pub mod permissions;
pub mod pipeline;
pub mod stt;
pub mod tray;
pub mod ui;
pub mod vocab_match;
