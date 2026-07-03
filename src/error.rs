use thiserror::Error;

#[derive(Debug, Error)]
pub enum TalkerError {
    #[error("Audio-Aufnahme fehlgeschlagen: {0}")]
    Audio(String),
    #[error("Clipboard-Zugriff fehlgeschlagen: {0}")]
    Clipboard(String),
    #[error("CGEventTap konnte nicht erstellt werden (fehlt die Accessibility-Permission?)")]
    EventTap,
    #[error("Tastatur-Event konnte nicht erzeugt werden: {0}")]
    Keystroke(String),
    #[error("Spracherkennung: {0}")]
    Stt(String),
    #[error("Cleanup: {0}")]
    Cleanup(String),
    #[error("Config: {0}")]
    Config(String),
    #[error("Modell-Download: {0}")]
    Download(String),
    #[error("Menüleiste: {0}")]
    Tray(String),
}

pub type Result<T> = std::result::Result<T, TalkerError>;
