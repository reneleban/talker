//! Konfiguration (TOML): ~/Library/Application Support/talker/config.toml.
//! Fehlende/kaputte Datei oder Teilfelder → sinnvolle Defaults, kein Crash.
//!
//! Das Format ist ein Kompatibilitäts-Vertrag (`docs/stability.md`): jedes
//! neue Feld berührt Config + Default — fehlende Felder füllt `#[serde(default)]`,
//! falscher Typ ist ein Parse-Fehler. Deprecation statt stillem Break:
//! Legacy-Felder migriert `parse()` (Referenz: cleanup_enabled → cleanup_mode).

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

/// Default der Sprechpause für *Satzweise*.
pub const DEFAULT_ENDPOINT_SILENCE_MS: u32 = 700;

/// Stufe 1 des Live-Diktats (ADR-0009): wann der Text ins Zielfeld kommt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InsertMode {
    /// Gesamter Text nach dem Loslassen (bisheriges Verhalten).
    #[default]
    #[serde(rename = "end")]
    AtEnd,
    /// Ein Satz je Sprechpause.
    #[serde(rename = "sentence")]
    PerSentence,
    /// Stabile Wortgruppen während des Sprechens.
    #[serde(rename = "phrase")]
    PerPhrase,
}

impl InsertMode {
    pub const ALL: [InsertMode; 3] = [
        InsertMode::AtEnd,
        InsertMode::PerSentence,
        InsertMode::PerPhrase,
    ];

    pub fn label(self) -> &'static str {
        match self {
            InsertMode::AtEnd => "Am Ende",
            InsertMode::PerSentence => "Satzweise",
            InsertMode::PerPhrase => "Wortgruppenweise",
        }
    }

    /// Stufe 2: erlaubte Cleanup-Modi (aufsteigend nach Eingriffstiefe).
    pub fn allowed_modes(self) -> &'static [CleanupMode] {
        match self {
            InsertMode::AtEnd => &CleanupMode::ALL,
            InsertMode::PerSentence => {
                &[CleanupMode::Raw, CleanupMode::Business, CleanupMode::Casual]
            }
            InsertMode::PerPhrase => &[CleanupMode::Raw],
        }
    }

    pub fn allows(self, mode: CleanupMode) -> bool {
        self.allowed_modes().contains(&mode)
    }

    /// Nicht erlaubter Modus → der höchste erlaubte.
    pub fn resolve(self, mode: CleanupMode) -> CleanupMode {
        if self.allows(mode) {
            mode
        } else {
            *self.allowed_modes().last().unwrap_or(&CleanupMode::Raw)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// PTT-Taste (macOS-Keycode eines Modifiers, Default Fn/🌐 = 63).
    pub hotkey_keycode: i64,
    /// Verzeichnis des Nemotron-Modells (encoder/decoder/joiner/tokens).
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
    /// Modell-Downloader (Ticket-0028): Nutzer hat den Gemma-Terms (und früher
    /// der Parakeet-Lizenz) zugestimmt. Zusammen mit `stt_license_consent` Voraussetzung
    /// für jeden Download, siehe `models_consent()`.
    pub model_download_consent: bool,
    /// Nutzer hat der Lizenz des STT-Modells (NVIDIA Nemotron, OpenMDW-1.1)
    /// zugestimmt. Eigenes Feld, damit Bestandsnutzer nach dem Modellwechsel
    /// erneut gefragt werden.
    pub stt_license_consent: bool,
    /// Live-Diktat Stufe 1: wann der Text eingefügt wird (ADR-0009).
    pub insert_mode: InsertMode,
    /// Sprechpause in ms, nach der *Satzweise* einfügt. Nur per TOML einstellbar.
    pub endpoint_silence_ms: u32,
}

/// Default-Farben der Wellen: Weiß, Cyan, Blau, Violett (kühle Siri-Palette).
pub const DEFAULT_OVERLAY_COLORS: [[u8; 3]; 4] = [
    [242, 247, 255],
    [89, 217, 255],
    [64, 115, 255],
    [158, 102, 255],
];

/// Gültige Wertebereiche der Overlay-Optik-Felder — die EINE Quelle für die
/// Slider in `ui.rs` und die Clamps in `overlay.rs` (Ticket-0035). Werte
/// außerhalb (z.B. von Hand editierte TOML) werden beim Rendern geclampt,
/// nicht abgelehnt.
pub mod overlay_limits {
    use std::ops::RangeInclusive;

    pub const WIDTH_PCT: RangeInclusive<u8> = 15..=80;
    pub const GAIN: RangeInclusive<f32> = 2.0..=30.0;
    pub const SPEED: RangeInclusive<f32> = 0.25..=3.0;
    pub const TRAIL_LEN: RangeInclusive<u8> = 1..=12;
    pub const TRAIL_DECAY: RangeInclusive<f32> = 0.2..=0.9;

    /// Spur-Obergrenze als usize — dimensioniert die Layer-Pools des Overlays.
    pub const TRAIL_LEN_MAX: usize = *TRAIL_LEN.end() as usize;

    /// Clamp auf einen der Bereiche oben.
    pub fn clamp<T: PartialOrd + Copy>(value: T, range: &RangeInclusive<T>) -> T {
        if value < *range.start() {
            *range.start()
        } else if value > *range.end() {
            *range.end()
        } else {
            value
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey_keycode: crate::hotkey::DEFAULT_PTT_KEYCODE,
            stt_model_dir: crate::stt::NemotronTranscriber::default_model_dir(),
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
            model_download_consent: false,
            stt_license_consent: false,
            insert_mode: InsertMode::default(),
            endpoint_silence_ms: DEFAULT_ENDPOINT_SILENCE_MS,
        }
    }
}

/// Verzeichnisname des früheren STT-Modells (Parakeet): nicht mit dem
/// Nemotron-Loader kompatibel, wird beim Laden auf den Default umgebogen.
const LEGACY_PARAKEET_DIR_NAME: &str = "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8";

impl Config {
    /// Beide Lizenz-Zustimmungen liegen vor — erst dann darf ein Modell geladen werden.
    pub fn models_consent(&self) -> bool {
        self.model_download_consent && self.stt_license_consent
    }

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

    /// Parst TOML tolerant: fehlende Felder → Default (`#[serde(default)]`),
    /// falscher Typ in einem bekannten Feld → Fehler, unbekannte Felder werden
    /// toleriert. Migriert vorab das Legacy-Feld `cleanup_enabled` (bool, bis
    /// Ticket-0010): false → Roh, true → Geschäftlich — ein explizit gesetztes
    /// `cleanup_mode` gewinnt. Ein `stt_model_dir`, das auf das frühere
    /// Parakeet-Verzeichnis zeigt, wird auf den Nemotron-Default gesetzt.
    fn parse(text: &str) -> std::result::Result<Self, toml::de::Error> {
        use serde::de::Error as _;
        let mut value: toml::Value = toml::from_str(text)?;
        if let Some(table) = value.as_table_mut() {
            match table.remove("cleanup_enabled") {
                None => {}
                Some(toml::Value::Boolean(enabled)) => {
                    let mode = if enabled {
                        CleanupMode::Business
                    } else {
                        CleanupMode::Raw
                    };
                    let mode = toml::Value::try_from(mode).map_err(toml::de::Error::custom)?;
                    table.entry("cleanup_mode").or_insert(mode);
                }
                // Kein stiller Fallback: falscher Typ ist ein Fehler.
                Some(other) => {
                    return Err(toml::de::Error::custom(format!(
                        "cleanup_enabled: bool erwartet, fand {}",
                        other.type_str()
                    )));
                }
            }
        }
        let mut cfg: Self = value.try_into()?;
        if cfg
            .stt_model_dir
            .file_name()
            .is_some_and(|name| name == LEGACY_PARAKEET_DIR_NAME)
        {
            cfg.stt_model_dir = Self::default().stt_model_dir;
        }
        Ok(cfg)
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

    /// AK2 (Ticket-0035): die Overlay-Defaults liegen im geteilten
    /// Wertebereichs-Vertrag, den ui-Slider und overlay-Clamps nutzen —
    /// und der Clamp-Helper schneidet an beiden Grenzen korrekt.
    #[test]
    fn overlay_defaults_lie_within_the_shared_limits() {
        use overlay_limits::*;
        let cfg = Config::default();
        assert!(WIDTH_PCT.contains(&cfg.overlay_width_pct));
        assert!(GAIN.contains(&cfg.overlay_gain));
        assert!(SPEED.contains(&cfg.overlay_speed));
        assert!(TRAIL_LEN.contains(&cfg.overlay_trail_len));
        assert!(TRAIL_DECAY.contains(&cfg.overlay_trail_decay));
        assert_eq!(TRAIL_LEN_MAX, usize::from(*TRAIL_LEN.end()));
        // Grenzwertanalyse: unter/über/innerhalb.
        assert_eq!(clamp(14u8, &WIDTH_PCT), 15);
        assert_eq!(clamp(81u8, &WIDTH_PCT), 80);
        assert_eq!(clamp(40u8, &WIDTH_PCT), 40);
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
    fn stt_license_consent_defaults_false_and_roundtrips() {
        // Alte Configs ohne das Feld → kein Consent für die STT-Lizenz.
        assert!(
            !Config::parse("cleanup_mode = \"raw\"")
                .unwrap()
                .stt_license_consent
        );
        assert!(!Config::default().stt_license_consent);
        let cfg = Config {
            stt_license_consent: true,
            ..Config::default()
        };
        assert!(
            Config::parse(&cfg.serialize().unwrap())
                .unwrap()
                .stt_license_consent
        );
        assert!(Config::parse("stt_license_consent = \"ja\"").is_err());
    }

    /// REQ-STREAM-001/002: drei Einfüge-Verhalten, Default Am Ende; alte
    /// Configs ohne Feld bleiben gültig, falscher Wert/Typ ist ein Parse-Fehler.
    #[test]
    fn insert_mode_defaults_to_at_end_and_roundtrips() {
        assert_eq!(InsertMode::ALL.len(), 3);
        assert_eq!(Config::default().insert_mode, InsertMode::AtEnd);
        assert_eq!(
            Config::parse("cleanup_mode = \"raw\"").unwrap().insert_mode,
            InsertMode::AtEnd
        );
        for mode in InsertMode::ALL {
            let cfg = Config {
                insert_mode: mode,
                ..Config::default()
            };
            assert_eq!(
                Config::parse(&cfg.serialize().unwrap())
                    .unwrap()
                    .insert_mode,
                mode
            );
        }
        assert!(Config::parse("insert_mode = \"sofort\"").is_err());
        assert!(Config::parse("insert_mode = 3").is_err());
    }

    /// REQ-STREAM-008: fehlende Pausenlänge → Default; übrige Werte werden gelesen.
    #[test]
    fn endpoint_silence_defaults_and_roundtrips() {
        assert_eq!(Config::default().endpoint_silence_ms, 700);
        assert_eq!(
            Config::parse("insert_mode = \"sentence\"")
                .unwrap()
                .endpoint_silence_ms,
            700
        );
        assert_eq!(
            Config::parse("endpoint_silence_ms = 1200")
                .unwrap()
                .endpoint_silence_ms,
            1200
        );
        assert!(Config::parse("endpoint_silence_ms = \"lang\"").is_err());
    }

    /// REQ-STREAM-009: Erlaubnismatrix je Einfüge-Verhalten.
    #[test]
    fn insert_mode_allows_exactly_the_agreed_cleanup_modes() {
        use CleanupMode::*;
        assert_eq!(
            InsertMode::AtEnd.allowed_modes(),
            &[Raw, Business, Casual, LlmOptimized]
        );
        assert_eq!(
            InsertMode::PerSentence.allowed_modes(),
            &[Raw, Business, Casual]
        );
        assert_eq!(InsertMode::PerPhrase.allowed_modes(), &[Raw]);
        assert!(InsertMode::AtEnd.allows(LlmOptimized));
        assert!(!InsertMode::PerSentence.allows(LlmOptimized));
        assert!(!InsertMode::PerPhrase.allows(Business));
    }

    /// REQ-STREAM-010/015: nicht erlaubter Modus → nächster erlaubter
    /// (der höchste erlaubte); erlaubte Modi bleiben unverändert.
    #[test]
    fn insert_mode_resolves_disallowed_cleanup_modes_to_the_nearest_allowed() {
        use CleanupMode::*;
        for mode in CleanupMode::ALL {
            assert_eq!(InsertMode::AtEnd.resolve(mode), mode);
        }
        assert_eq!(InsertMode::PerSentence.resolve(LlmOptimized), Casual);
        assert_eq!(InsertMode::PerSentence.resolve(Business), Business);
        assert_eq!(InsertMode::PerPhrase.resolve(LlmOptimized), Raw);
        assert_eq!(InsertMode::PerPhrase.resolve(Casual), Raw);
        assert_eq!(InsertMode::PerPhrase.resolve(Raw), Raw);
    }

    /// Bestandsnutzer haben nur den alten Lizenzen (Parakeet + Gemma)
    /// zugestimmt: ohne neue STT-Zustimmung gilt der Consent nicht.
    #[test]
    fn models_consent_needs_both_flags() {
        for (download, stt, expected) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (true, true, true),
        ] {
            let cfg = Config {
                model_download_consent: download,
                stt_license_consent: stt,
                ..Config::default()
            };
            assert_eq!(cfg.models_consent(), expected, "{download}/{stt}");
        }
        let legacy = Config::parse("model_download_consent = true").unwrap();
        assert!(!legacy.models_consent());
    }

    /// Das alte Parakeet-Verzeichnis passt nicht zum Nemotron-Loader: es wird
    /// beim Laden auf den neuen Default umgebogen (egal wo es liegt).
    #[test]
    fn legacy_parakeet_model_dir_migrates_to_the_default() {
        let default_dir = Config::default().stt_model_dir;
        for legacy in [
            "/Users/x/Library/Application Support/talker/models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
            "/Volumes/extern/modelle/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/",
        ] {
            let cfg = Config::parse(&format!("stt_model_dir = \"{legacy}\"")).unwrap();
            assert_eq!(cfg.stt_model_dir, default_dir, "{legacy}");
        }
    }

    #[test]
    fn custom_and_current_model_dirs_are_kept() {
        for dir in [
            "/tmp/eigenes/modell",
            "/tmp/parakeet-test",
            "/tmp/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8-alt",
        ] {
            let cfg = Config::parse(&format!("stt_model_dir = \"{dir}\"")).unwrap();
            assert_eq!(cfg.stt_model_dir, PathBuf::from(dir));
        }
        let current = Config::default().stt_model_dir;
        let text = Config::default().serialize().unwrap();
        assert_eq!(Config::parse(&text).unwrap().stt_model_dir, current);
    }

    #[test]
    fn model_download_consent_defaults_false_and_roundtrips() {
        // Abwärtskompatibilität: alte Configs ohne das Feld → kein Consent.
        assert!(
            !Config::parse("cleanup_mode = \"raw\"")
                .unwrap()
                .model_download_consent
        );
        assert!(!Config::default().model_download_consent);
        let cfg = Config {
            model_download_consent: true,
            ..Config::default()
        };
        let text = cfg.serialize().unwrap();
        assert!(Config::parse(&text).unwrap().model_download_consent);
        // Falscher Typ ist ein Parse-Fehler, kein stiller Fallback.
        assert!(Config::parse("model_download_consent = \"ja\"").is_err());
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
