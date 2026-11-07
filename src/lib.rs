//! talker — lokaler macOS-Diktier-Assistent (Bibliotheks-Teil).
//! Architektur & Entscheidungen: siehe `docs/adr/` und `CONTEXT.md`.

pub mod audio;
pub mod cleanup;
pub mod clipboard;
pub mod config;
pub mod diagnostics;
pub mod error;
pub mod hotkey;
pub mod indicator;
pub mod injection;
pub mod logging;
pub mod login_item;
pub mod models;
pub mod overlay;
pub mod permissions;
pub mod pipeline;
pub mod stt;
pub mod tray;
pub mod ui;
pub mod vocab_match;

/// Hilfen der „erstes Wort"-Regression: die Unit-Tests laufen auch in der CI (`cargo test --lib`).
#[cfg(test)]
#[path = "../tests/support/first_word.rs"]
mod first_word_support;
