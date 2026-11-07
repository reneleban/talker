//! Messzeile je Dictation (ADR-0012, REQ-DIAG-012 … 018): alle Zeitpunkte auf der
//! gemeinsamen Uhr ([`HostInstant`]) relativ zum Tastendruck. `DictationTrace` hat
//! absichtlich kein Textfeld — Transcript-Text kann nie in die Zeile gelangen.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use crate::audio::{Captured, HostInstant, SourceInfo};
use crate::cleanup::CleanupMode;
use crate::config::InsertMode;
use crate::models::ModelId;

/// Wie die Dictation endete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DictationResult {
    Inserted,
    Discarded(&'static str),
    Blocked,
    Error(String),
}

/// Messpunkte einer Dictation; fehlende Punkte bleiben `None` und erscheinen als `—`.
#[derive(Debug, Clone)]
pub struct DictationTrace {
    pub nr: u64,
    pub pressed_at: HostInstant,
    pub mic_ready_at: Option<HostInstant>,
    pub first_block_at: Option<HostInstant>,
    pub first_signal_at: Option<HostInstant>,
    pub released_at: Option<HostInstant>,
    pub recorded: Option<Duration>,
    pub text_at: Option<HostInstant>,
    pub stt: Option<Duration>,
    pub cleanup: Option<Duration>,
    pub injected: Option<Duration>,
    pub first_segment_at: Option<HostInstant>,
    pub segments: Option<u32>,
    pub model_loads: Vec<(ModelId, Duration)>,
    pub device: Option<SourceInfo>,
    pub insert_mode: InsertMode,
    pub cleanup_mode: CleanupMode,
    pub cold_start: bool,
    pub result: DictationResult,
}

/// Meldet die fertige Messzeile (App: `log::info!(target: "talker::messung", …)`).
pub type TraceReport = Arc<dyn Fn(DictationTrace) + Send + Sync>;

/// Der Report der App: genau eine Zeile je Dictation auf `info`, Ziel `talker::messung`.
pub fn log_report() -> TraceReport {
    Arc::new(|trace| log::info!(target: "talker::messung", "{trace}"))
}

const MISSING: &str = "—";

/// Unter 1 s in ms, sonst Sekunden mit Komma und einer Nachkommastelle.
fn fmt_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1_000 {
        return format!("{ms} ms");
    }
    let tenths = (ms + 50) / 100;
    format!("{},{} s", tenths / 10, tenths % 10)
}

fn fmt_opt(d: Option<Duration>) -> String {
    d.map_or_else(|| MISSING.to_string(), fmt_duration)
}

fn fmt_khz(rate: u32) -> String {
    let hundreds = (rate + 50) / 100;
    if hundreds.is_multiple_of(10) {
        format!("{} kHz", hundreds / 10)
    } else {
        format!("{},{} kHz", hundreds / 10, hundreds % 10)
    }
}

fn cleanup_name(mode: CleanupMode) -> &'static str {
    match mode {
        CleanupMode::Raw => "Roh",
        CleanupMode::Business => "Geschäftlich",
        CleanupMode::Casual => "Natürlich",
        CleanupMode::LlmOptimized => "LLM-optimiert",
    }
}

fn model_name(id: ModelId) -> &'static str {
    match id {
        ModelId::Stt => "STT",
        ModelId::Gemma => "Cleanup",
    }
}

impl DictationTrace {
    /// Neue Trace beim Tastendruck. `result` steht auf „abgebrochen", bis ein Ende
    /// es setzt — jeder Pfad meldet seine Trace mit gesetztem Ergebnis.
    pub fn new(
        nr: u64,
        pressed_at: HostInstant,
        insert_mode: InsertMode,
        cleanup_mode: CleanupMode,
    ) -> Self {
        Self {
            nr,
            pressed_at,
            mic_ready_at: None,
            first_block_at: None,
            first_signal_at: None,
            released_at: None,
            recorded: None,
            text_at: None,
            stt: None,
            cleanup: None,
            injected: None,
            first_segment_at: None,
            segments: None,
            model_loads: Vec::new(),
            device: None,
            insert_mode,
            cleanup_mode,
            cold_start: nr == 1,
            result: DictationResult::Discarded("abgebrochen"),
        }
    }

    /// Übernimmt Loslass-Zeitpunkt, aufgenommene Dauer, Start-Messpunkte und Gerät.
    pub fn apply_capture(&mut self, released_at: HostInstant, captured: &Captured) {
        self.released_at = Some(released_at);
        self.recorded = Some(captured.recorded());
        self.first_block_at = captured.marks.first_block;
        self.first_signal_at = captured.marks.first_signal;
        self.device = Some(captured.info.clone());
    }

    /// Abstand eines Messpunkts zum Tastendruck.
    fn since_press(&self, at: Option<HostInstant>) -> Option<Duration> {
        at.map(|at| at.saturating_since(self.pressed_at))
    }
}

impl fmt::Display for DictationTrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let held = self
            .released_at
            .map(|at| at.saturating_since(self.pressed_at));
        let to_text = match (self.text_at, self.released_at) {
            (Some(text), Some(released)) => Some(text.saturating_since(released)),
            _ => None,
        };
        write!(
            f,
            "Diktat #{}: Taste→Mikro bereit {} · →erster Block {} · →erstes Signal {} · gehalten {} · aufgenommen {} · Loslassen→Text {} (STT {} · Cleanup {}) · →eingefügt {}",
            self.nr,
            fmt_opt(self.since_press(self.mic_ready_at)),
            fmt_opt(self.since_press(self.first_block_at)),
            fmt_opt(self.since_press(self.first_signal_at)),
            fmt_opt(held),
            fmt_opt(self.recorded),
            fmt_opt(to_text),
            fmt_opt(self.stt),
            fmt_opt(self.cleanup),
            fmt_opt(self.injected),
        )?;
        if self.insert_mode != InsertMode::AtEnd {
            write!(
                f,
                " · →erstes Segment {} · Segmente {}",
                fmt_opt(self.since_press(self.first_segment_at)),
                self.segments
                    .map_or_else(|| MISSING.to_string(), |n| n.to_string()),
            )?;
        }
        for (id, took) in &self.model_loads {
            write!(
                f,
                " · Modell-Laden {} {}",
                model_name(*id),
                fmt_duration(*took)
            )?;
        }
        let device = self.device.as_ref().map_or_else(
            || MISSING.to_string(),
            |d| {
                let channels = d.format.channels;
                format!(
                    "{} ({}, {} {})",
                    d.device,
                    fmt_khz(d.format.sample_rate),
                    channels,
                    if channels == 1 { "Kanal" } else { "Kanäle" }
                )
            },
        );
        let result = match &self.result {
            DictationResult::Inserted => "eingefügt".to_string(),
            DictationResult::Discarded(reason) => format!("verworfen ({reason})"),
            DictationResult::Blocked => "gesperrt".to_string(),
            DictationResult::Error(e) => format!("Fehler ({})", e.replace(['\n', '\r'], " ")),
        };
        write!(
            f,
            " · Gerät: {device} · Einfügen: {} · Modus: {} · Kaltstart: {} · Ergebnis: {result}",
            self.insert_mode.label(),
            cleanup_name(self.cleanup_mode),
            if self.cold_start { "ja" } else { "nein" },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::AudioFormat;

    const T0: u64 = 5_000_000_000;

    fn at(ms: u64) -> Option<HostInstant> {
        Some(HostInstant::from_nanos(T0 + ms * 1_000_000))
    }

    fn ms(n: u64) -> Option<Duration> {
        Some(Duration::from_millis(n))
    }

    /// Die vollständige Beispielzeile aus dem Ticket (Diktat #17).
    fn example() -> DictationTrace {
        DictationTrace {
            nr: 17,
            pressed_at: HostInstant::from_nanos(T0),
            mic_ready_at: at(120),
            first_block_at: at(380),
            first_signal_at: at(640),
            released_at: at(4_200),
            recorded: ms(3_600),
            text_at: at(4_980),
            stt: ms(520),
            cleanup: ms(240),
            injected: ms(35),
            first_segment_at: None,
            segments: None,
            model_loads: vec![],
            device: Some(SourceInfo {
                format: AudioFormat {
                    channels: 1,
                    sample_rate: 48_000,
                },
                device: "AirPods".into(),
            }),
            insert_mode: InsertMode::AtEnd,
            cleanup_mode: CleanupMode::Casual,
            cold_start: false,
            result: DictationResult::Inserted,
        }
    }

    /// REQ-DIAG-012/-013: Format wörtlich wie im Beispiel des Tickets.
    #[test]
    fn example_line_matches_the_ticket_verbatim() {
        assert_eq!(
            example().to_string(),
            "Diktat #17: Taste→Mikro bereit 120 ms · →erster Block 380 ms · →erstes Signal 640 ms · gehalten 4,2 s · aufgenommen 3,6 s · Loslassen→Text 780 ms (STT 520 ms · Cleanup 240 ms) · →eingefügt 35 ms · Gerät: AirPods (48 kHz, 1 Kanal) · Einfügen: Am Ende · Modus: Natürlich · Kaltstart: nein · Ergebnis: eingefügt"
        );
    }

    /// REQ-DIAG-015: Live zusätzlich →erstes Segment und Segmente.
    #[test]
    fn live_adds_first_segment_and_segment_count() {
        let mut t = example();
        t.insert_mode = InsertMode::PerSentence;
        t.first_segment_at = at(1_400);
        t.segments = Some(3);
        let line = t.to_string();
        assert!(line.contains("→eingefügt 35 ms · →erstes Segment 1,4 s · Segmente 3 · Gerät:"));
        assert!(line.contains("Einfügen: Satzweise"));
        assert!(!example().to_string().contains("Segment"));
        t.first_segment_at = None;
        t.segments = None;
        assert!(t.to_string().contains("→erstes Segment — · Segmente —"));
        t.insert_mode = InsertMode::PerPhrase;
        assert!(t.to_string().contains("Einfügen: Wortgruppenweise"));
    }

    /// REQ-DIAG-016: Ladedauer je Modell, nur wenn geladen wurde.
    #[test]
    fn model_loads_are_listed_per_model() {
        let mut t = example();
        assert!(!t.to_string().contains("Modell-Laden"));
        t.model_loads = vec![(ModelId::Stt, Duration::from_millis(1_900))];
        assert!(
            t.to_string()
                .contains("→eingefügt 35 ms · Modell-Laden STT 1,9 s · Gerät:")
        );
        t.model_loads
            .push((ModelId::Gemma, Duration::from_millis(3_200)));
        assert!(
            t.to_string()
                .contains("Modell-Laden STT 1,9 s · Modell-Laden Cleanup 3,2 s · Gerät:")
        );
    }

    #[test]
    fn cold_start_is_shown_as_ja() {
        let mut t = example();
        t.cold_start = true;
        assert!(t.to_string().contains("Kaltstart: ja"));
    }

    /// Zahlenformat: ms unter 1 s, sonst Sekunden mit Komma; Grenzen.
    #[test]
    fn durations_switch_from_ms_to_seconds_at_one_second() {
        let d = |n: u64| fmt_duration(Duration::from_millis(n));
        assert_eq!(d(0), "0 ms");
        assert_eq!(d(999), "999 ms");
        assert_eq!(d(1_000), "1,0 s");
        assert_eq!(d(1_949), "1,9 s");
        assert_eq!(d(1_950), "2,0 s");
        assert_eq!(d(60_000), "60,0 s");
        assert_eq!(fmt_khz(44_100), "44,1 kHz");
        assert_eq!(fmt_khz(48_000), "48 kHz");
        assert_eq!(fmt_khz(16_000), "16 kHz");
    }

    #[test]
    fn two_channels_use_the_plural() {
        let mut t = example();
        t.device = Some(SourceInfo {
            format: AudioFormat {
                channels: 2,
                sample_rate: 44_100,
            },
            device: "MacBook-Mikrofon".into(),
        });
        assert!(
            t.to_string()
                .contains("Gerät: MacBook-Mikrofon (44,1 kHz, 2 Kanäle)")
        );
    }

    /// REQ-DIAG-017: nichts erreicht → überall `—`, Zeile trotzdem vollständig.
    fn nothing_reached(result: DictationResult) -> DictationTrace {
        DictationTrace {
            mic_ready_at: None,
            first_block_at: None,
            first_signal_at: None,
            released_at: None,
            recorded: None,
            text_at: None,
            stt: None,
            cleanup: None,
            injected: None,
            device: None,
            result,
            ..example()
        }
    }

    #[test]
    fn missing_everything_prints_dashes_and_still_has_all_fields() {
        let line = nothing_reached(DictationResult::Error("Mikrofon fehlt".into())).to_string();
        assert_eq!(
            line,
            "Diktat #17: Taste→Mikro bereit — · →erster Block — · →erstes Signal — · gehalten — · aufgenommen — · Loslassen→Text — (STT — · Cleanup —) · →eingefügt — · Gerät: — · Einfügen: Am Ende · Modus: Natürlich · Kaltstart: nein · Ergebnis: Fehler (Mikrofon fehlt)"
        );
    }

    /// AK-13: kein Block bis Loslassen / nur Null-Blöcke / zu kurz / nichts erkannt.
    #[test]
    fn discard_reasons_and_partial_marks_are_rendered() {
        // Nur Null-Blöcke: erster Block da, erstes Signal fehlt.
        let mut t = example();
        t.first_signal_at = None;
        assert!(
            t.to_string()
                .contains("→erster Block 380 ms · →erstes Signal — ·")
        );
        // Kein Block bis Loslassen.
        t.first_block_at = None;
        assert!(
            t.to_string()
                .contains("→erster Block — · →erstes Signal — ·")
        );
        // Zu kurz / nichts erkannt / abgebrochen.
        for reason in ["zu kurz", "nichts erkannt", "abgebrochen"] {
            let line = nothing_reached(DictationResult::Discarded(reason)).to_string();
            assert!(
                line.ends_with(&format!("Ergebnis: verworfen ({reason})")),
                "{line}"
            );
        }
        // STT-Fehler nach Aufnahme: Aufnahme-Werte da, Text/Einfügen fehlen.
        let mut stt_failed = example();
        stt_failed.text_at = None;
        stt_failed.stt = None;
        stt_failed.cleanup = None;
        stt_failed.injected = None;
        stt_failed.result = DictationResult::Error("STT: kaputt".into());
        let line = stt_failed.to_string();
        assert!(line.contains("Loslassen→Text — (STT — · Cleanup —) · →eingefügt —"));
        assert!(line.ends_with("Ergebnis: Fehler (STT: kaputt)"));
    }

    /// Modus Roh: Cleanup `—`, Modus „Roh".
    #[test]
    fn raw_mode_has_no_cleanup_time() {
        let mut t = example();
        t.cleanup_mode = CleanupMode::Raw;
        t.cleanup = None;
        let line = t.to_string();
        assert!(line.contains("(STT 520 ms · Cleanup —)"));
        assert!(line.contains("Modus: Roh"));
    }

    #[test]
    fn blocked_result_is_gesperrt() {
        let line = nothing_reached(DictationResult::Blocked).to_string();
        assert!(line.ends_with("Kaltstart: nein · Ergebnis: gesperrt"));
    }

    /// Release vor dem Tastendruck-Stempel (Uhr-Rauschen) → 0 statt Unterlauf.
    #[test]
    fn points_before_the_press_saturate_to_zero() {
        let mut t = example();
        t.mic_ready_at = Some(HostInstant::from_nanos(T0 - 1));
        assert!(t.to_string().contains("Taste→Mikro bereit 0 ms"));
    }

    /// „Darf nicht passieren": kein Transcript-Text, immer einzeilig. `DictationTrace` hat
    /// kein Textfeld; selbst ein mehrzeiliger Fehlertext bleibt eine Zeile.
    #[test]
    fn line_is_single_line_even_with_multiline_error_text() {
        let t = nothing_reached(DictationResult::Error("zeile eins\nzeile zwei\r\n".into()));
        let line = t.to_string();
        assert!(!line.contains('\n') && !line.contains('\r'));
        assert!(line.contains("Fehler (zeile eins zeile zwei  )"));
        assert!(!example().to_string().contains('\n'));
    }

    /// `new`: Kaltstart nur bei Nr. 1, nichts gemessen, Ergebnis noch „abgebrochen".
    #[test]
    fn new_trace_has_no_marks_and_cold_start_only_for_the_first() {
        let first = DictationTrace::new(
            1,
            HostInstant::from_nanos(T0),
            InsertMode::AtEnd,
            CleanupMode::Raw,
        );
        assert!(first.cold_start);
        assert!(first.mic_ready_at.is_none() && first.device.is_none());
        assert_eq!(first.result, DictationResult::Discarded("abgebrochen"));
        let second = DictationTrace::new(
            2,
            HostInstant::from_nanos(T0),
            InsertMode::AtEnd,
            CleanupMode::Raw,
        );
        assert!(!second.cold_start);
    }

    #[test]
    fn apply_capture_takes_release_marks_and_device() {
        use crate::audio::StartMarks;
        let captured = Captured {
            raw: vec![0.0; 96_000],
            info: SourceInfo {
                format: AudioFormat {
                    channels: 2,
                    sample_rate: 48_000,
                },
                device: "Mikro".into(),
            },
            marks: StartMarks {
                first_block: at(380),
                first_signal: None,
            },
        };
        let mut t = DictationTrace::new(
            3,
            HostInstant::from_nanos(T0),
            InsertMode::AtEnd,
            CleanupMode::Raw,
        );
        t.apply_capture(at(2_000).unwrap(), &captured);
        assert_eq!(t.released_at, at(2_000));
        assert_eq!(t.recorded, ms(1_000));
        assert_eq!(t.first_block_at, at(380));
        assert_eq!(t.first_signal_at, None);
        assert_eq!(t.device.unwrap().device, "Mikro");
    }
}
