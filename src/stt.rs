//! stt: 16 kHz mono PCM → Raw Transcript.
//!
//! Engine hinter dem `Transcriber`-Trait (ADR-0001). Primäre Impl:
//! NVIDIA Nemotron 3.5 ASR Streaming 0.6b (multilingual, de/en) via sherpa-onnx
//! (Crate `sherpa-onnx`), on-device.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use sherpa_onnx::{
    OnlineModelConfig, OnlineRecognizer, OnlineRecognizerConfig, OnlineStream,
    OnlineTransducerModelConfig,
};

use crate::config::Language;
use crate::error::{Result, TalkerError};

/// PCM-Buffer (16 kHz mono) → Raw Transcript.
pub trait Transcriber {
    fn transcribe(&mut self, pcm_16k_mono: &[f32]) -> Result<String>;

    /// Sprache für die nächsten Aufrufe (`transcribe`, `begin`); Default: ignoriert.
    fn set_language(&mut self, _language: Language) {}
}

/// Geteilter Puffer für den Debug-Mitschnitt (ADR-0012): sammelt exakt das, was das Modell
/// bekommt (inkl. Vor-/Nachlauf-Stille). `Clone` teilt den Puffer.
#[derive(Clone, Default)]
pub struct ModelTap(Arc<Mutex<Vec<f32>>>);

impl ModelTap {
    /// Holt den bisherigen Inhalt und leert den Puffer.
    pub fn take(&self) -> Vec<f32> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }

    pub(crate) fn push(&self, pcm: &[f32]) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(pcm);
    }
}

pub struct NemotronTranscriber {
    inner: OnlineRecognizer,
    /// Laufende Utterance im Live-Betrieb (`begin` … `finish`).
    live: Option<OnlineStream>,
    /// Gesamttext des Streams an der letzten Segment-Grenze (ungetrimmt, ADR-0015).
    committed: String,
    /// Samples seit der letzten Grenze bzw. seit dem Beginn des offenen Segments.
    since_boundary: usize,
    /// Nur für den Debug-Mitschnitt gesetzt.
    tap: Option<ModelTap>,
    language: Language,
}

/// Stille vor dem Audio: das Streaming-Modell verschluckt sonst den Satzanfang.
const LEAD_SILENCE_SAMPLES: usize = 6_400; // 0,4 s bei 16 kHz
/// Stille nach dem Audio: füllt den letzten 560-ms-Chunk, sonst fehlen die
/// letzten Wörter (PoC: mit 0,3 s fehlten sie, mit 1,2 s nicht).
const TAIL_SILENCE_SAMPLES: usize = 19_200; // 1,2 s bei 16 kHz
/// Chunk des Pakets: 560 ms bei 16 kHz — der Nachlauf muss ihn füllen.
const CHUNK_SAMPLES: usize = 8_960;
const _: () = assert!(TAIL_SILENCE_SAMPLES >= CHUNK_SAMPLES);
/// Zwangs-Segment-Ende für sehr lange Segmente ohne Pause (talker zählt, nicht sherpa).
const MAX_SEGMENT_SAMPLES: usize = 60 * 16_000;
/// sherpas Regel 3 (Utterance-Länge) zählt ab dem Stream-Start und würde ohne Reset
/// dauerhaft feuern — sie darf nie greifen.
const RULE3_NEVER: f32 = 1.0e9;

use crate::config::DEFAULT_ENDPOINT_SILENCE_MS;

/// Mehr Threads verkürzen die Wartezeit nach dem Loslassen deutlich (Messung auf
/// einem 14-Kern-Mac: 4 → 1,09 s, 8 → 0,73 s); darüber bremsen Effizienz-Kerne.
fn num_threads() -> i32 {
    std::thread::available_parallelism().map_or(4, |n| n.get().min(8)) as i32
}

fn pad_with_silence(pcm: &[f32]) -> Vec<f32> {
    let mut padded = Vec::with_capacity(LEAD_SILENCE_SAMPLES + pcm.len() + TAIL_SILENCE_SAMPLES);
    padded.resize(LEAD_SILENCE_SAMPLES, 0.0);
    padded.extend_from_slice(pcm);
    padded.resize(padded.len() + TAIL_SILENCE_SAMPLES, 0.0);
    padded
}

const MODEL_FILES: [&str; 4] = [
    "encoder.int8.onnx",
    "decoder.int8.onnx",
    "joiner.int8.onnx",
    "tokens.txt",
];

impl NemotronTranscriber {
    /// Lädt das Nemotron-Modell aus `model_dir`. Fehlende Dateien → klarer Fehler.
    pub fn new(model_dir: &Path) -> Result<Self> {
        Self::with_endpoint_silence(model_dir, DEFAULT_ENDPOINT_SILENCE_MS)
    }

    /// Wie `new`; `endpoint_silence_ms` ist die Sprechpause, die im Live-Betrieb
    /// ein Segment beendet.
    pub fn with_endpoint_silence(model_dir: &Path, endpoint_silence_ms: u32) -> Result<Self> {
        for file in MODEL_FILES {
            let path = model_dir.join(file);
            if !path.is_file() {
                return Err(TalkerError::Stt(format!(
                    "Modell-Datei fehlt: {} — Nemotron-Modell nach {} entpacken \
                     (sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11)",
                    path.display(),
                    model_dir.display()
                )));
            }
        }
        let path = |file: &str| Some(model_dir.join(file).to_string_lossy().into_owned());
        let config = OnlineRecognizerConfig {
            model_config: OnlineModelConfig {
                transducer: OnlineTransducerModelConfig {
                    encoder: path("encoder.int8.onnx"),
                    decoder: path("decoder.int8.onnx"),
                    joiner: path("joiner.int8.onnx"),
                },
                tokens: path("tokens.txt"),
                num_threads: num_threads(),
                ..Default::default()
            },
            decoding_method: Some("greedy_search".into()),
            enable_endpoint: true,
            rule1_min_trailing_silence: endpoint_silence_ms as f32 / 1000.0,
            rule2_min_trailing_silence: endpoint_silence_ms as f32 / 1000.0,
            rule3_min_utterance_length: RULE3_NEVER,
            ..Default::default()
        };
        let inner = OnlineRecognizer::create(&config)
            .ok_or_else(|| TalkerError::Stt("Modell laden fehlgeschlagen".into()))?;
        Ok(Self {
            inner,
            live: None,
            committed: String::new(),
            since_boundary: 0,
            tap: None,
            language: Language::default(),
        })
    }

    /// Schreibt alles, was das Modell bekommt, zusätzlich in `tap` (Debug-Mitschnitt).
    pub fn with_model_tap(mut self, tap: ModelTap) -> Self {
        self.tap = Some(tap);
        self
    }

    /// Einziger Ort für `accept_waveform`: speist das Modell und den Tap.
    fn accept(&self, stream: &OnlineStream, pcm: &[f32]) {
        stream.accept_waveform(crate::audio::TARGET_SAMPLE_RATE as i32, pcm);
        if let Some(tap) = &self.tap {
            tap.push(pcm);
        }
    }

    /// Default-Ablageort: ~/Library/Application Support/talker/models/<modellname>
    pub fn default_model_dir() -> PathBuf {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        home.join(
            "Library/Application Support/talker/models/sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11",
        )
    }
}

impl Transcriber for NemotronTranscriber {
    fn transcribe(&mut self, pcm_16k_mono: &[f32]) -> Result<String> {
        if pcm_16k_mono.is_empty() {
            return Ok(String::new());
        }
        let stream = self.inner.create_stream();
        stream.set_option("language", self.language.code());
        self.accept(&stream, &pad_with_silence(pcm_16k_mono));
        stream.input_finished();
        while self.inner.is_ready(&stream) {
            self.inner.decode(&stream);
        }
        let text = self
            .inner
            .get_result(&stream)
            .map(|r| r.text)
            .unwrap_or_default();
        Ok(text.trim().to_string())
    }

    fn set_language(&mut self, language: Language) {
        self.language = language;
    }
}

/// Zwischenstand nach einem `feed`: Text des offenen Segments; `endpoint` ist
/// wahr, wenn eine Sprechpause das Segment beendet hat (Text ist dann final,
/// das nächste Segment beginnt hinter diesem Text; der Stream wird nicht zurückgesetzt).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamUpdate {
    pub text: String,
    pub endpoint: bool,
}

/// Live-Diktat: Audio in Chunks einspeisen (ADR-0009). Eine Utterance läuft von
/// `begin` bis `finish`.
pub trait StreamingTranscriber {
    fn begin(&mut self);
    fn feed(&mut self, pcm_16k_mono: &[f32]) -> Result<StreamUpdate>;
    /// Beendet die Utterance und liefert den Text des noch offenen Segments.
    fn finish(&mut self) -> Result<String>;
}

/// Wortgruppenweise: bestimmt aus Zwischenständen die stabilen Wörter. Stabil
/// = in zwei aufeinanderfolgenden Ständen an gleicher Stelle gleich und nicht
/// das letzte Wort. Bereits ausgegebene Wörter werden nie zurückgenommen.
#[derive(Debug, Default)]
pub struct PhraseCommitter {
    emitted: usize,
    previous: Vec<String>,
}

fn words(text: &str) -> Vec<String> {
    text.split_whitespace().map(str::to_string).collect()
}

impl PhraseCommitter {
    fn take(&mut self, current: &[String], upto: usize) -> Option<String> {
        let upto = upto.min(current.len());
        if upto <= self.emitted {
            return None;
        }
        let out = current[self.emitted..upto].join(" ");
        self.emitted = upto;
        Some(out)
    }

    /// Neuer Zwischenstand → neu stabile Wörter.
    pub fn update(&mut self, partial: &str) -> Option<String> {
        let current = words(partial);
        let common = self
            .previous
            .iter()
            .zip(&current)
            .take_while(|(a, b)| a == b)
            .count();
        let stable = common.min(current.len().saturating_sub(1));
        let out = self.take(&current, stable);
        self.previous = current;
        out
    }

    /// Segment-Ende: restliche Wörter ausgeben, neues Segment beginnen.
    pub fn flush(&mut self, text: &str) -> Option<String> {
        let current = words(text);
        let out = self.take(&current, current.len());
        *self = Self::default();
        out
    }
}

impl NemotronTranscriber {
    fn drain(&mut self, stream: &OnlineStream) -> String {
        while self.inner.is_ready(stream) {
            self.inner.decode(stream);
        }
        self.inner
            .get_result(stream)
            .map(|r| r.text)
            .unwrap_or_default()
    }
}

/// Offenes Segment: Gesamttext ohne das bereits ausgelieferte Präfix `committed`.
/// Ist es kein Präfix (bei Greedy ohne ITN nicht zu erwarten, ADR-0015), zählt der Text
/// ab dem gemeinsamen Präfix.
fn open_segment<'a>(total: &'a str, committed: &str) -> &'a str {
    let common = total
        .char_indices()
        .zip(committed.chars())
        .find(|((_, a), b)| a != b)
        .map_or_else(|| total.len().min(committed.len()), |((i, _), _)| i);
    &total[common..]
}

impl StreamingTranscriber for NemotronTranscriber {
    fn begin(&mut self) {
        let stream = self.inner.create_stream();
        stream.set_option("language", self.language.code());
        self.accept(&stream, &[0.0; LEAD_SILENCE_SAMPLES]);
        self.committed.clear();
        self.since_boundary = 0;
        self.live = Some(stream);
    }

    fn feed(&mut self, pcm_16k_mono: &[f32]) -> Result<StreamUpdate> {
        let stream = self
            .live
            .take()
            .ok_or_else(|| TalkerError::Stt("feed ohne begin".into()))?;
        if !pcm_16k_mono.is_empty() {
            self.accept(&stream, pcm_16k_mono);
            self.since_boundary += pcm_16k_mono.len();
        }
        let total = self.drain(&stream);
        if !total.starts_with(&self.committed) {
            log::warn!("Live-Text ist nicht präfixstabil; Segment ab dem gemeinsamen Präfix");
        }
        let text = open_segment(&total, &self.committed).trim().to_string();
        // Eine Pause ohne neuen Text ist kein Segment-Ende; ohne Text gibt es auch nichts zu erzwingen.
        let ended = !text.is_empty()
            && (self.inner.is_endpoint(&stream) || self.since_boundary >= MAX_SEGMENT_SAMPLES);
        if ended {
            self.committed = total;
            self.since_boundary = 0;
        } else if text.is_empty() {
            self.since_boundary = 0;
        }
        self.live = Some(stream);
        Ok(StreamUpdate {
            endpoint: ended,
            text,
        })
    }

    fn finish(&mut self) -> Result<String> {
        let stream = self
            .live
            .take()
            .ok_or_else(|| TalkerError::Stt("finish ohne begin".into()))?;
        self.accept(&stream, &[0.0; TAIL_SILENCE_SAMPLES]);
        stream.input_finished();
        let total = self.drain(&stream);
        Ok(open_segment(&total, &self.committed).trim().to_string())
    }
}

/// Pipeline-Isolation: Konsumenten hängen nur am Trait, nicht an die Engine.
#[cfg(test)]
pub(crate) struct FakeTranscriber {
    pub(crate) reply: &'static str,
}

#[cfg(test)]
impl Transcriber for FakeTranscriber {
    fn transcribe(&mut self, pcm: &[f32]) -> Result<String> {
        if pcm.is_empty() {
            return Ok(String::new());
        }
        Ok(self.reply.to_string())
    }
}

/// Skriptierter Streaming-Fake: `feed` liefert die vorbereiteten Updates der Reihe nach.
#[cfg(test)]
pub(crate) struct FakeStreaming {
    pub(crate) updates: std::collections::VecDeque<StreamUpdate>,
    pub(crate) rest: &'static str,
    pub(crate) fail_feed: bool,
}

#[cfg(test)]
impl FakeStreaming {
    pub(crate) fn new(updates: &[(&str, bool)], rest: &'static str) -> Self {
        Self {
            updates: updates
                .iter()
                .map(|(text, endpoint)| StreamUpdate {
                    text: (*text).to_string(),
                    endpoint: *endpoint,
                })
                .collect(),
            rest,
            fail_feed: false,
        }
    }
}

#[cfg(test)]
impl Transcriber for FakeStreaming {
    fn transcribe(&mut self, _pcm: &[f32]) -> Result<String> {
        Ok(self.rest.to_string())
    }
}

#[cfg(test)]
impl StreamingTranscriber for FakeStreaming {
    fn begin(&mut self) {}

    fn feed(&mut self, _pcm: &[f32]) -> Result<StreamUpdate> {
        if self.fail_feed {
            return Err(TalkerError::Stt("feed kaputt".into()));
        }
        Ok(self.updates.pop_front().unwrap_or(StreamUpdate {
            text: String::new(),
            endpoint: false,
        }))
    }

    fn finish(&mut self) -> Result<String> {
        Ok(self.rest.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REQ-STREAM-016: offenes Segment = Gesamttext ohne das bereits ausgelieferte Präfix.
    #[test]
    fn open_segment_is_the_text_after_the_committed_prefix() {
        assert_eq!(
            open_segment("Satz eins. Satz zwei", "Satz eins."),
            " Satz zwei"
        );
        assert_eq!(open_segment("Satz eins.", "Satz eins."), "");
        assert_eq!(open_segment("Hallo", ""), "Hallo");
        assert_eq!(open_segment("", ""), "");
    }

    /// Grenzfall: `committed` ist kein Präfix → ab dem gemeinsamen Präfix, auch mit Mehrbyte-Zeichen.
    #[test]
    fn open_segment_without_prefix_starts_at_the_common_prefix() {
        assert_eq!(open_segment("Grüne Wiese", "Grüße"), "ne Wiese");
        assert_eq!(open_segment("Äpfel", "Öl"), "Äpfel");
        assert_eq!(open_segment("ab", "abc"), "");
        assert_eq!(open_segment("xyz", "abc"), "xyz");
    }

    /// Trait-Default von `set_language` ist ein No-op: Fakes bleiben unverändert.
    #[test]
    fn set_language_default_is_a_noop_for_fakes() {
        let mut fake = FakeTranscriber { reply: "x" };
        fake.set_language(Language::De);
        assert_eq!(fake.transcribe(&[0.1]).unwrap(), "x");
    }

    /// REQ-DIAG-019: `take` leert, `Clone` teilt den Puffer.
    #[test]
    fn model_tap_take_drains_and_clones_share_the_buffer() {
        let tap = ModelTap::default();
        assert!(tap.take().is_empty());
        let shared = tap.clone();
        shared.push(&[0.5, -0.5]);
        tap.push(&[1.0]);
        assert_eq!(tap.take(), [0.5, -0.5, 1.0]);
        assert!(shared.take().is_empty());
    }

    #[test]
    fn padding_surrounds_the_audio_with_silence() {
        let pcm = [0.5_f32; 1_600];
        let padded = pad_with_silence(&pcm);
        assert_eq!(
            padded.len(),
            LEAD_SILENCE_SAMPLES + pcm.len() + TAIL_SILENCE_SAMPLES
        );
        assert!(padded[..LEAD_SILENCE_SAMPLES].iter().all(|s| *s == 0.0));
        assert_eq!(
            &padded[LEAD_SILENCE_SAMPLES..LEAD_SILENCE_SAMPLES + pcm.len()],
            &pcm
        );
        assert!(
            padded[LEAD_SILENCE_SAMPLES + pcm.len()..]
                .iter()
                .all(|s| *s == 0.0)
        );
    }

    /// 0039 Out of Scope: Dauer von Vor- und Nachlauf bleibt 0,4 s / 1,2 s (16 kHz) — nicht verkürzen (→ 0041).
    #[test]
    fn padding_durations_stay_0_4_s_lead_and_1_2_s_tail() {
        assert_eq!(LEAD_SILENCE_SAMPLES, 6_400);
        assert_eq!(TAIL_SILENCE_SAMPLES, 19_200);
        assert_eq!(pad_with_silence(&[]).len(), 6_400 + 19_200);
        assert_eq!(pad_with_silence(&[0.25]).len(), 6_400 + 1 + 19_200);
    }

    #[test]
    fn padding_of_empty_audio_is_only_silence() {
        assert_eq!(
            pad_with_silence(&[]).len(),
            LEAD_SILENCE_SAMPLES + TAIL_SILENCE_SAMPLES
        );
    }

    #[test]
    fn trait_is_object_safe_and_swappable() {
        let mut t: Box<dyn Transcriber> = Box::new(FakeTranscriber {
            reply: "hallo welt",
        });
        assert_eq!(t.transcribe(&[0.0; 160]).unwrap(), "hallo welt");
    }

    #[test]
    fn empty_pcm_gives_empty_transcript() {
        let mut t: Box<dyn Transcriber> = Box::new(FakeTranscriber { reply: "x" });
        assert_eq!(t.transcribe(&[]).unwrap(), "");
    }

    /// REQ-STREAM-005: nur Wörter, die in zwei aufeinanderfolgenden Zwischenständen
    /// übereinstimmen — und nie das letzte (evtl. noch unfertige) Wort.
    #[test]
    fn phrase_committer_emits_only_words_stable_across_two_partials() {
        let mut c = PhraseCommitter::default();
        assert_eq!(c.update("Das ist"), None);
        assert_eq!(c.update("Das ist ein"), Some("Das ist".into()));
        assert_eq!(c.update("Das ist ein Test"), Some("ein".into()));
        // Nichts Neues → nichts doppelt.
        assert_eq!(c.update("Das ist ein Test"), None);
    }

    #[test]
    fn phrase_committer_never_emits_the_last_word_early() {
        let mut c = PhraseCommitter::default();
        assert_eq!(c.update("Hallo"), None);
        assert_eq!(c.update("Hallo"), None);
    }

    #[test]
    fn phrase_committer_keeps_emitted_words_when_the_recognizer_revises_them() {
        let mut c = PhraseCommitter::default();
        c.update("Wir sind da");
        assert_eq!(c.update("Wir sind da dann"), Some("Wir sind da".into()));
        // Frühere Wörter ändern sich: bereits Eingefügtes wird nicht erneut gesendet.
        assert_eq!(c.update("Wie sind da dann los"), None);
        assert_eq!(
            c.update("Wie sind da dann los jetzt"),
            Some("dann los".into())
        );
    }

    /// REQ-STREAM-006: am Ende (Endpoint/Loslassen) kommt der Rest — genau einmal.
    #[test]
    fn phrase_committer_flush_returns_the_rest_and_resets() {
        let mut c = PhraseCommitter::default();
        c.update("Das ist");
        c.update("Das ist ein");
        assert_eq!(c.flush("Das ist ein Test"), Some("ein Test".into()));
        // Neues Segment beginnt bei null.
        assert_eq!(c.update("Neu"), None);
        assert_eq!(c.flush("Neu"), Some("Neu".into()));
    }

    #[test]
    fn phrase_committer_handles_empty_and_whitespace_input() {
        let mut c = PhraseCommitter::default();
        assert_eq!(c.update(""), None);
        assert_eq!(c.update("   "), None);
        assert_eq!(c.flush(""), None);
        assert_eq!(c.flush("  "), None);
    }

    #[test]
    fn missing_model_dir_is_a_clear_error_not_a_crash() {
        let Err(err) = NemotronTranscriber::new(Path::new("/nonexistent/model/dir")) else {
            panic!("erwartet: Fehler bei fehlendem Modell");
        };
        let msg = err.to_string();
        assert!(msg.contains("Modell-Datei fehlt"), "unklarer Fehler: {msg}");
    }
}
