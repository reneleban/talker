//! Konfiguration (TOML): ~/Library/Application Support/talker/config.toml.
//! Fehlende/kaputte Datei oder Teilfelder → sinnvolle Defaults, kein Crash.
//!
//! Das Format ist ein Kompatibilitäts-Vertrag (`docs/stability.md`): jedes
//! neue Feld berührt Config, RawConfig, Default und From<RawConfig>;
//! Deprecation statt stillem Break (Referenz: cleanup_enabled → cleanup_mode).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::cleanup::CleanupMode;
use crate::error::{Result, TalkerError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OverlayPosition {
    Top,
    Bottom,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Config {
    /// PTT-Taste (macOS-Keycode eines Modifiers, Default Fn/🌐 = 63).
    pub hotkey_keycode: i64,
    /// Verzeichnis des Parakeet-Modells (encoder/decoder/joiner/tokens).
    pub stt_model_dir: PathBuf,
    /// Aktives Cleanup-Stil-Profil (ersetzt das frühere `cleanup_enabled`).
    pub cleanup_mode: CleanupMode,
    /// Mikrofon nach Name; None = System-Default.
    pub mic_device: Option<String>,
    /// Aufnahme-Indikator: oben oder unten am Bildschirm.
    pub overlay_position: OverlayPosition,
    /// Aufnahme-Indikator: Breite in % der Bildschirmbreite (15–80).
    pub overlay_width_pct: u8,
    /// Eigene Fachbegriffe/Eigennamen — der Cleanup korrigiert Verhörer
    /// der Spracherkennung auf diese exakte Schreibweise (Ticket-0012).
    pub vocabulary: Vec<String>,
    /// Deterministische phonetische Vokabular-Korrektur (Kölner Phonetik).
    pub phonetic_matching: bool,
    /// Overlay-Optik: Pegel-Empfindlichkeit (Verstärkung des Mikrofon-RMS).
    pub overlay_gain: f32,
    /// Overlay-Optik: Animations-Tempo (1.0 = normal).
    pub overlay_speed: f32,
    /// Overlay-Optik: Länge der Leuchtspur (Anzahl Nachbilder, 0–12).
    pub overlay_trail_len: u8,
    /// Overlay-Optik: Ausglüh-Faktor der Spur pro Nachbild (0.3–0.8).
    pub overlay_trail_decay: f32,
    /// Overlay-Optik: Farben der vier Wellen (sRGB).
    pub overlay_colors: Vec<[u8; 3]>,
    /// Kontext-Awareness: Cleanup-Modus je fokussierter App wählen (opt-in).
    pub context_aware_enabled: bool,
    /// Regeln (bundle-id → Modus); erste passende gewinnt, sonst `cleanup_mode`.
    pub context_rules: Vec<(String, CleanupMode)>,
}

/// Default-Farben der Wellen: Weiß, Cyan, Blau, Violett (kühle Siri-Palette).
pub const DEFAULT_OVERLAY_COLORS: [[u8; 3]; 4] = [
    [242, 247, 255],
    [89, 217, 255],
    [64, 115, 255],
    [158, 102, 255],
];

/// Roh-Form fürs Laden: kennt zusätzlich das Legacy-Feld `cleanup_enabled`
/// (bool, bis Ticket-0010) und migriert es beim Einlesen.
#[derive(Deserialize)]
#[serde(default)]
struct RawConfig {
    hotkey_keycode: i64,
    stt_model_dir: PathBuf,
    cleanup_mode: Option<CleanupMode>,
    cleanup_enabled: Option<bool>,
    mic_device: Option<String>,
    overlay_position: OverlayPosition,
    overlay_width_pct: u8,
    vocabulary: Vec<String>,
    phonetic_matching: bool,
    overlay_gain: f32,
    overlay_speed: f32,
    overlay_trail_len: u8,
    overlay_trail_decay: f32,
    overlay_colors: Vec<[u8; 3]>,
    context_aware_enabled: bool,
    context_rules: Vec<(String, CleanupMode)>,
}

impl Default for RawConfig {
    fn default() -> Self {
        let d = Config::default();
        Self {
            hotkey_keycode: d.hotkey_keycode,
            stt_model_dir: d.stt_model_dir,
            cleanup_mode: None,
            cleanup_enabled: None,
            mic_device: d.mic_device,
            overlay_position: d.overlay_position,
            overlay_width_pct: d.overlay_width_pct,
            vocabulary: d.vocabulary,
            phonetic_matching: d.phonetic_matching,
            overlay_gain: d.overlay_gain,
            overlay_speed: d.overlay_speed,
            overlay_trail_len: d.overlay_trail_len,
            overlay_trail_decay: d.overlay_trail_decay,
            overlay_colors: d.overlay_colors,
            context_aware_enabled: d.context_aware_enabled,
            context_rules: d.context_rules,
        }
    }
}

impl From<RawConfig> for Config {
    fn from(raw: RawConfig) -> Self {
        // Explizites cleanup_mode gewinnt; sonst Legacy-Bool migrieren:
        // false → Roh, true → Geschäftlich; beides fehlend → Default.
        let cleanup_mode = raw.cleanup_mode.unwrap_or(match raw.cleanup_enabled {
            Some(false) => CleanupMode::Raw,
            Some(true) => CleanupMode::Business,
            None => CleanupMode::default(),
        });
        Self {
            hotkey_keycode: raw.hotkey_keycode,
            stt_model_dir: raw.stt_model_dir,
            cleanup_mode,
            mic_device: raw.mic_device,
            overlay_position: raw.overlay_position,
            overlay_width_pct: raw.overlay_width_pct,
            vocabulary: raw.vocabulary,
            phonetic_matching: raw.phonetic_matching,
            overlay_gain: raw.overlay_gain,
            overlay_speed: raw.overlay_speed,
            overlay_trail_len: raw.overlay_trail_len,
            overlay_trail_decay: raw.overlay_trail_decay,
            overlay_colors: raw.overlay_colors,
            context_aware_enabled: raw.context_aware_enabled,
            context_rules: raw.context_rules,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey_keycode: crate::hotkey::DEFAULT_PTT_KEYCODE,
            stt_model_dir: crate::stt::ParakeetTranscriber::default_model_dir(),
            cleanup_mode: CleanupMode::default(),
            mic_device: None,
            overlay_position: OverlayPosition::Bottom,
            overlay_width_pct: 33,
            vocabulary: Vec::new(),
            phonetic_matching: true,
            overlay_gain: 10.0,
            overlay_speed: 1.0,
            overlay_trail_len: 6,
            overlay_trail_decay: 0.55,
            overlay_colors: DEFAULT_OVERLAY_COLORS.to_vec(),
            context_aware_enabled: false,
            context_rules: Vec::new(),
        }
    }
}

impl Config {
    pub fn default_path() -> PathBuf {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        home.join("Library/Application Support/talker/config.toml")
    }

    /// Lädt die Config vom default_path.
    pub fn load() -> Self {
        Self::load_from(&Self::default_path())
    }

    /// Lädt die Config; fehlende Datei → Defaults, kaputte Datei → Defaults + Hinweis.
    pub fn load_from(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        Self::parse(&text).unwrap_or_else(|e| {
            eprintln!(
                "talker: config.toml unlesbar ({e}) — nutze Defaults. Pfad: {}",
                path.display()
            );
            Self::default()
        })
    }

    /// Speichert nach default_path.
    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::default_path())
    }

    /// Speichert nach `path` (legt das Verzeichnis bei Bedarf an).
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let map = |e: &dyn std::fmt::Display| {
            TalkerError::Config(format!("{} nicht schreibbar: {e}", path.display()))
        };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| map(&e))?;
        }
        let text = self.serialize().map_err(|e| map(&e))?;
        std::fs::write(path, text).map_err(|e| map(&e))?;
        Ok(())
    }

    fn parse(text: &str) -> std::result::Result<Self, toml::de::Error> {
        toml::from_str::<RawConfig>(text).map(Config::from)
    }

    fn serialize(&self) -> std::result::Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sensible() {
        let cfg = Config::default();
        assert_eq!(cfg.cleanup_mode, CleanupMode::Business);
        assert_eq!(cfg.hotkey_keycode, 63);
        assert!(cfg.mic_device.is_none());
        assert!(
            cfg.stt_model_dir
                .to_string_lossy()
                .contains("talker/models")
        );
    }

    #[test]
    fn legacy_cleanup_enabled_bool_migrates() {
        // Alter Schalter aus → Roh; an → Geschäftlich.
        assert_eq!(
            Config::parse("cleanup_enabled = false")
                .unwrap()
                .cleanup_mode,
            CleanupMode::Raw
        );
        assert_eq!(
            Config::parse("cleanup_enabled = true")
                .unwrap()
                .cleanup_mode,
            CleanupMode::Business
        );
        // Explizites cleanup_mode gewinnt gegen Legacy-Bool.
        let both = "cleanup_enabled = false\ncleanup_mode = \"llm\"";
        assert_eq!(
            Config::parse(both).unwrap().cleanup_mode,
            CleanupMode::LlmOptimized
        );
        // Fehlend → Default; unbekannter Wert → Fehler (load fällt auf Defaults).
        assert_eq!(
            Config::parse("").unwrap().cleanup_mode,
            CleanupMode::Business
        );
        assert!(Config::parse("cleanup_mode = \"episch\"").is_err());
    }

    #[test]
    fn all_modes_roundtrip_through_toml() {
        for mode in CleanupMode::ALL {
            let cfg = Config {
                cleanup_mode: mode,
                ..Config::default()
            };
            let text = cfg.serialize().unwrap();
            assert_eq!(Config::parse(&text).unwrap().cleanup_mode, mode, "{mode:?}");
        }
    }

    #[test]
    fn empty_toml_gives_defaults() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn overlay_style_fields_parse_and_default() {
        let cfg = Config::parse("overlay_gain = 14.5\noverlay_trail_len = 9").unwrap();
        assert_eq!(cfg.overlay_gain, 14.5);
        assert_eq!(cfg.overlay_trail_len, 9);
        assert_eq!(cfg.overlay_speed, 1.0, "Default");
        assert_eq!(
            cfg.overlay_colors,
            DEFAULT_OVERLAY_COLORS.to_vec(),
            "Default"
        );
        assert!(cfg.phonetic_matching, "Phonetik default an");
        assert!(
            !Config::parse("phonetic_matching = false")
                .unwrap()
                .phonetic_matching
        );
    }

    #[test]
    fn partial_toml_fills_missing_fields_with_defaults() {
        let cfg = Config::parse("cleanup_mode = \"raw\"").unwrap();
        assert_eq!(cfg.cleanup_mode, CleanupMode::Raw);
        assert_eq!(cfg.hotkey_keycode, Config::default().hotkey_keycode);
        assert_eq!(cfg.stt_model_dir, Config::default().stt_model_dir);
    }

    #[test]
    fn broken_toml_is_an_error() {
        assert!(Config::parse("cleanup_mode = ").is_err());
        assert!(Config::parse("hotkey_keycode = \"fn\"").is_err());
    }

    #[test]
    fn context_awareness_defaults_off_and_empty() {
        // Abwärtskompatibilität: alte Configs ohne die Felder → Feature AUS.
        let cfg = Config::parse("cleanup_mode = \"raw\"").unwrap();
        assert!(!cfg.context_aware_enabled);
        assert!(cfg.context_rules.is_empty());
        assert!(!Config::default().context_aware_enabled);
        assert!(Config::default().context_rules.is_empty());
    }

    #[test]
    fn context_rules_roundtrip_through_toml() {
        let cfg = Config {
            context_aware_enabled: true,
            context_rules: vec![
                ("com.apple.Terminal".into(), CleanupMode::LlmOptimized),
                ("com.tinyspeck.slackmacgap".into(), CleanupMode::Casual),
            ],
            ..Config::default()
        };
        let text = cfg.serialize().unwrap();
        let parsed = Config::parse(&text).unwrap();
        assert!(parsed.context_aware_enabled);
        assert_eq!(parsed.context_rules, cfg.context_rules);
    }

    #[test]
    fn context_rules_with_unknown_mode_are_a_parse_error() {
        assert!(Config::parse("context_rules = [[\"com.foo\", \"episch\"]]").is_err());
    }

    #[test]
    fn vocabulary_roundtrips_and_defaults_empty() {
        assert!(Config::parse("").unwrap().vocabulary.is_empty());
        let cfg = Config {
            vocabulary: vec!["Claude CLI".into(), "egui".into()],
            ..Config::default()
        };
        let text = cfg.serialize().unwrap();
        assert_eq!(Config::parse(&text).unwrap().vocabulary, cfg.vocabulary);
    }

    #[test]
    fn serialize_parse_roundtrip() {
        let cfg = Config {
            hotkey_keycode: 61,
            stt_model_dir: PathBuf::from("/tmp/modelle/parakeet"),
            cleanup_mode: CleanupMode::Casual,
            mic_device: Some("MacBook Pro Mikrofon".into()),
            overlay_position: OverlayPosition::Top,
            overlay_width_pct: 50,
            vocabulary: vec!["TOML".into()],
            ..Config::default()
        };
        let text = cfg.serialize().unwrap();
        assert_eq!(Config::parse(&text).unwrap(), cfg);
    }

    #[test]
    fn overlay_defaults_and_partial_parse() {
        let cfg = Config::parse("overlay_position = \"top\"").unwrap();
        assert_eq!(cfg.overlay_position, OverlayPosition::Top);
        assert_eq!(cfg.overlay_width_pct, 33, "Default-Breite");
        assert!(Config::parse("overlay_position = \"mitte\"").is_err());
    }

    /// Frisches, test-eigenes Verzeichnis unterm System-Tempdir (kein tempfile-Dep).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(test: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("talker-config-test-{}-{test}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn load_from_missing_file_gives_defaults() {
        let dir = TempDir::new("missing");
        assert_eq!(
            Config::load_from(&dir.path("gibts-nicht.toml")),
            Config::default()
        );
    }

    #[test]
    fn load_from_broken_file_gives_defaults() {
        let dir = TempDir::new("broken");
        let path = dir.path("config.toml");
        std::fs::write(&path, "cleanup_mode = ").unwrap();
        assert_eq!(Config::load_from(&path), Config::default());
    }

    #[test]
    fn save_load_roundtrip_via_file() {
        let dir = TempDir::new("roundtrip");
        let path = dir.path("config.toml");
        let cfg = Config {
            cleanup_mode: CleanupMode::Casual,
            vocabulary: vec!["Kölner Phonetik".into()],
            overlay_width_pct: 42,
            ..Config::default()
        };

        cfg.save_to(&path).unwrap();

        assert_eq!(Config::load_from(&path), cfg);
    }

    #[test]
    fn save_creates_missing_directories() {
        let dir = TempDir::new("mkdir");
        let path = dir.path("tief/verschachtelt/config.toml");

        Config::default().save_to(&path).unwrap();

        assert!(path.is_file());
    }

    #[test]
    fn save_write_error_maps_to_config_error() {
        let dir = TempDir::new("write-error");
        // Elternpfad ist eine DATEI → create_dir_all muss scheitern.
        let blocker = dir.path("blocker");
        std::fs::write(&blocker, "x").unwrap();
        let path = blocker.join("config.toml");

        let err = Config::default().save_to(&path).unwrap_err();

        assert!(
            matches!(err, TalkerError::Config(ref msg) if msg.contains("nicht schreibbar")),
            "erwartet Config-Fehler, bekam: {err}"
        );
    }

    #[test]
    fn legacy_cleanup_enabled_with_wrong_type_is_a_parse_error() {
        // Kein stiller Fallback: falscher Typ ist ein Fehler (load → Defaults + Hinweis).
        assert!(Config::parse("cleanup_enabled = \"ja\"").is_err());
        assert!(Config::parse("cleanup_enabled = 1").is_err());
    }

    #[test]
    fn overlay_colors_with_wrong_length_parse_unchanged() {
        // Länge ≠ 4 ist gültige Config; den Pro-Index-Fallback macht overlay.rs
        // (wave_color), hier bleibt die Liste erhalten.
        let short = Config::parse("overlay_colors = [[1, 2, 3]]").unwrap();
        assert_eq!(short.overlay_colors, vec![[1, 2, 3]]);
        let long =
            Config::parse("overlay_colors = [[0,0,0],[1,1,1],[2,2,2],[3,3,3],[4,4,4]]").unwrap();
        assert_eq!(long.overlay_colors.len(), 5);
        // Innere Länge ≠ 3 ist dagegen ein Typfehler.
        assert!(Config::parse("overlay_colors = [[1, 2]]").is_err());
    }

    /// Fuzz: beliebiger Input darf `parse` nie panicken lassen — immer Result.
    /// Deterministisches xorshift statt Zufalls-Seed (reproduzierbar).
    #[test]
    fn parse_never_panics_on_arbitrary_input() {
        let mut state: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let valid = Config::default().serialize().unwrap();
        let charset: Vec<char> = "abcdefghijklmnopqrstuvwxyz0123456789=[]{}\"'#.,-_\n\t ä→\u{0}"
            .chars()
            .collect();

        for _ in 0..2_000 {
            // Reiner Zufalls-String …
            let len = (next() % 200) as usize;
            let random: String = (0..len)
                .map(|_| charset[(next() as usize) % charset.len()])
                .collect();
            let _ = Config::parse(&random);

            // … und Mutation einer gültigen Config (Zeichen ersetzen/abschneiden).
            let mut chars: Vec<char> = valid.chars().collect();
            let cut = (next() as usize) % (chars.len() + 1);
            chars.truncate(cut);
            if !chars.is_empty() {
                let pos = (next() as usize) % chars.len();
                chars[pos] = charset[(next() as usize) % charset.len()];
            }
            let mutated: String = chars.into_iter().collect();
            let _ = Config::parse(&mutated);
        }
    }
}
