//! Dictation Worker: verarbeitet den Strom von Utterances — besitzt STT-Engine
//! und Cleanup-LLM, verwaltet deren Lebenszyklus (Lazy-Load, Fehler-Cache,
//! Modellwechsel zur Laufzeit, Live-Aktivierung nach Modell-Download) und
//! liefert pro Utterance den fertigen Text bzw. einen Ablehnungsgrund.
//! `main.rs` drainiert nur noch den Channel und mappt `Outcome` auf
//! Indicator/Injection (Ticket-0034).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use crate::cleanup::{self, CleanupMode, LlmCleaner};
use crate::config::{Config, InsertMode};
use crate::diagnostics::{DebugCapture, DictationResult, DictationTrace, TraceReport};
use crate::error::TalkerError;
use crate::injection;
use crate::models::{ModelId, ModelsState};
use crate::stt::{ModelTap, PhraseCommitter, StreamingTranscriber, Transcriber};
use crate::{audio, vocab_match};

/// Hinweis, wenn kein Text erkannt wurde.
const NOTHING_RECOGNIZED: &str = "Nichts erkannt";
/// Wie die Messzeile „kein Text" nennt.
const NOTHING_RECOGNIZED_REASON: &str = "nichts erkannt";

/// Ergebnis von [`DictationWorker::handle`] für eine Utterance.
#[derive(Debug)]
pub enum Outcome {
    /// Fertiger Text zum Einfügen in die Target App; `cleanup_fell_back`
    /// meldet, dass statt des Cleaned Transcript der Rohtext geliefert wurde.
    Inject {
        text: String,
        cleanup_fell_back: bool,
    },
    /// Utterance verworfen — der Hinweis geht sichtbar an den Indicator.
    Rejected(&'static str),
}

/// Lädt die STT-Engine aus einem Modell-Verzeichnis. Zwei Adapter: Nemotron
/// in `main.rs`, Fakes in den Tests.
pub type TranscriberLoader<T> = Box<dyn FnMut(&Path) -> crate::error::Result<T>>;
/// Lädt das Cleanup-LLM; das zweite Element meldet den Fehlschlag
/// (Fehler-Cache: bis zum Moduswechsel nicht erneut versuchen).
pub type CleanerLoader<C> = Box<dyn FnMut() -> (Option<C>, bool)>;

/// Der Dictation Worker (CONTEXT.md): genau einer pro laufender App, lebt auf
/// dem STT-Worker-Thread abseits des Main-Threads.
pub struct DictationWorker<T: Transcriber, C: LlmCleaner> {
    transcriber: Option<T>,
    stt_dir: PathBuf,
    cleaner: Option<C>,
    cleaner_failed: bool,
    llm_was_available: bool,
    live: Option<LiveSession>,
    models: Arc<ModelsState>,
    load_transcriber: TranscriberLoader<T>,
    load_cleaner: CleanerLoader<C>,
    /// Zeit in STT und Cleanup seit dem letzten `take_timing` (Messzeile).
    timing: JobTiming,
    /// Modelle, die während einer Dictation geladen wurden (nicht die in `new`).
    model_loads: Vec<(ModelId, Duration)>,
}

/// In STT und Cleanup verbrachte Zeit einer Dictation; `None` = nicht gelaufen.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct JobTiming {
    pub stt: Option<Duration>,
    pub cleanup: Option<Duration>,
}

fn add_time(total: &mut Option<Duration>, d: Duration) {
    *total = Some(total.unwrap_or_default() + d);
}

/// Misst die Zeit in `transcribe` (Batch-Pfad) in einen Summanden.
struct TimedTranscriber<'a> {
    inner: &'a mut dyn Transcriber,
    elapsed: &'a mut Option<Duration>,
}

impl Transcriber for TimedTranscriber<'_> {
    fn transcribe(&mut self, pcm_16k_mono: &[f32]) -> crate::error::Result<String> {
        let t0 = Instant::now();
        let result = self.inner.transcribe(pcm_16k_mono);
        add_time(self.elapsed, t0.elapsed());
        result
    }
}

/// Misst die Zeit in `clean` in einen Summanden.
struct TimedCleaner<'a> {
    inner: &'a mut dyn LlmCleaner,
    elapsed: &'a mut Option<Duration>,
}

impl LlmCleaner for TimedCleaner<'_> {
    fn clean(&mut self, raw: &str) -> crate::error::Result<String> {
        let t0 = Instant::now();
        let result = self.inner.clean(raw);
        add_time(self.elapsed, t0.elapsed());
        result
    }

    fn set_mode(&mut self, mode: CleanupMode) {
        self.inner.set_mode(mode);
    }

    fn set_vocab(&mut self, vocab: &[String]) {
        self.inner.set_vocab(vocab);
    }
}

impl<T: Transcriber, C: LlmCleaner> DictationWorker<T, C> {
    /// Lädt die STT-Engine sofort und das Cleanup-LLM vor, wenn die Config es
    /// braucht UND das Modell laut Downloader-State da ist — beim Erst-Start
    /// (Download läuft noch) wäre der Versuch nur ein Fehler.
    pub fn new(
        cfg: &Config,
        models: Arc<ModelsState>,
        load_transcriber: TranscriberLoader<T>,
        load_cleaner: CleanerLoader<C>,
    ) -> Self {
        let mut worker = Self {
            transcriber: None,
            stt_dir: cfg.stt_model_dir.clone(),
            cleaner: None,
            cleaner_failed: false,
            llm_was_available: models.llm_modes_available(),
            live: None,
            models,
            load_transcriber,
            load_cleaner,
            timing: JobTiming::default(),
            model_loads: Vec::new(),
        };
        let t0 = Instant::now();
        match (worker.load_transcriber)(&worker.stt_dir) {
            Ok(t) => {
                worker.transcriber = Some(t);
                log::info!("STT-Modell geladen in {:.1}s.", t0.elapsed().as_secs_f32());
            }
            Err(e) => log::warn!("STT nicht verfügbar — {e}"),
        }
        if worker.llm_was_available && config_wants_llm(cfg) {
            (worker.cleaner, worker.cleaner_failed) = (worker.load_cleaner)();
        }
        worker
    }

    /// Zeit in STT/Cleanup seit dem letzten Aufruf; setzt sie zurück.
    pub fn take_timing(&mut self) -> JobTiming {
        std::mem::take(&mut self.timing)
    }

    /// Modell-Ladevorgänge seit dem letzten Aufruf (nur solche während einer Dictation).
    pub fn take_model_loads(&mut self) -> Vec<(ModelId, Duration)> {
        std::mem::take(&mut self.model_loads)
    }

    /// STT-Engine für `dir` laden und die Ladezeit für die Messzeile merken.
    fn load_stt_timed(&mut self, dir: &Path) -> crate::error::Result<T> {
        let t0 = Instant::now();
        let loaded = (self.load_transcriber)(dir);
        if loaded.is_ok() {
            self.model_loads.push((ModelId::Stt, t0.elapsed()));
        }
        loaded
    }

    /// Cleanup-LLM laden und die Ladezeit für die Messzeile merken.
    fn load_cleaner_timed(&mut self) {
        let t0 = Instant::now();
        (self.cleaner, self.cleaner_failed) = (self.load_cleaner)();
        if self.cleaner.is_some() {
            self.model_loads.push((ModelId::Gemma, t0.elapsed()));
        }
    }

    /// Live-Aktivierung (Ticket-0029): wird gemma nachträglich ready,
    /// den Fehler-Cache löschen — der nächste LLM-Bedarf lädt dann.
    fn refresh_llm_availability(&mut self) {
        let llm_available = self.models.llm_modes_available();
        if llm_available && !self.llm_was_available {
            self.cleaner_failed = false;
        }
        self.llm_was_available = llm_available;
    }

    /// Verarbeitet eine Utterance unter der übergebenen Config-Momentaufnahme.
    /// Bricht nie hart ab: jeder Fehlerpfad wird zu einem `Outcome`.
    pub fn handle(&mut self, pcm: &[f32], frontmost: Option<&str>, cfg: &Config) -> Outcome {
        self.refresh_llm_availability();

        // Kontext-Awareness (Ticket-0026): Modus für DIESE Utterance auflösen.
        let resolved = resolve_mode(cfg, frontmost);
        if resolved != cfg.cleanup_mode {
            log::debug!(
                "Kontext-Regel aktiv ({}) → Modus {}.",
                frontmost.unwrap_or("?"),
                resolved.label()
            );
        }

        // STT-Pfad geändert → Modell neu laden (alter bleibt bei Fehler aktiv).
        if cfg.stt_model_dir != self.stt_dir || self.transcriber.is_none() {
            match self.load_stt_timed(&cfg.stt_model_dir) {
                Ok(t) => {
                    self.transcriber = Some(t);
                    self.stt_dir = cfg.stt_model_dir.clone();
                    log::info!("STT-Modell neu geladen: {}", self.stt_dir.display());
                }
                Err(e) => log::warn!("STT-Neuladen fehlgeschlagen — {e}"),
            }
        }
        if self.transcriber.is_none() {
            log::warn!("kein STT-Modell — Utterance verworfen.");
            return Outcome::Rejected("Kein STT-Modell");
        }

        if resolved.uses_llm() && self.cleaner.is_none() && !self.cleaner_failed {
            self.load_cleaner_timed();
        }
        // Entladen/Fehler-Reset folgt dem GESAMT-Bedarf der Config, nicht
        // der einzelnen Utterance — sonst würde jede Raw-geregelte App das
        // Modell entladen und die nächste LLM-Utterance den Kaltstart zahlen.
        if !config_wants_llm(cfg) {
            reset_cleaner_on_raw_mode(
                cfg.cleanup_mode,
                &mut self.cleaner,
                &mut self.cleaner_failed,
            );
        }

        // Ab hier gilt der aufgelöste Modus (process_utterance, set_mode).
        let cfg = Config {
            cleanup_mode: resolved,
            ..cfg.clone()
        };
        let Some(transcriber) = self.transcriber.as_mut() else {
            return Outcome::Rejected("Kein STT-Modell");
        };
        // Cleaner nur im LLM-Modus übergeben: aufgelöstes Raw darf einen
        // (für andere Apps) geladenen Cleaner nicht anwenden.
        let cleaner_ref = resolved
            .uses_llm()
            .then_some(self.cleaner.as_mut())
            .flatten()
            .map(|c| {
                c.set_mode(cfg.cleanup_mode);
                c.set_vocab(&cfg.vocabulary);
                c as &mut dyn LlmCleaner
            });
        let mut timing = JobTiming::default();
        let mut timed_stt = TimedTranscriber {
            inner: transcriber,
            elapsed: &mut timing.stt,
        };
        let mut timed_cleaner = cleaner_ref.map(|inner| TimedCleaner {
            inner,
            elapsed: &mut timing.cleanup,
        });
        let processed = process_utterance(
            pcm,
            &cfg,
            &mut timed_stt,
            timed_cleaner.as_mut().map(|c| c as &mut dyn LlmCleaner),
            self.cleaner_failed,
        );
        if let Some(t) = timing.stt {
            add_time(&mut self.timing.stt, t);
        }
        if let Some(t) = timing.cleanup {
            add_time(&mut self.timing.cleanup, t);
        }
        match processed {
            Processed::Text {
                text,
                cleanup_fell_back,
            } => Outcome::Inject {
                text,
                cleanup_fell_back,
            },
            Processed::Empty => {
                log::debug!("leerer Transcript — nichts einzufügen.");
                Outcome::Rejected(NOTHING_RECOGNIZED)
            }
            Processed::SttFailed(e) => {
                log::error!("STT fehlgeschlagen: {e}");
                Outcome::Rejected("Spracherkennung fehlgeschlagen")
            }
        }
    }
}

/// Ergebnis von [`DictationWorker::live_begin`].
#[derive(Debug, PartialEq, Eq)]
pub enum LiveStart {
    Started,
    /// Kein Live möglich (Einfüge-Verhalten Am Ende oder kein STT-Modell) —
    /// der Aufrufer fällt auf den Batch-Pfad zurück.
    Unavailable,
}

/// Zustand einer laufenden Live-Utterance (ADR-0009).
struct LiveSession {
    insert_mode: InsertMode,
    /// Config-Momentaufnahme mit dem für diese Utterance aufgelösten Modus.
    cfg: Config,
    committer: PhraseCommitter,
    emitted_any: bool,
}

/// Ergebnis einer verarbeiteten Utterance (worker-intern; nach außen: [`Outcome`]).
#[derive(Debug)]
enum Processed {
    /// Fertiger Text zum Einfügen; `cleanup_fell_back` meldet, dass statt des
    /// Cleaned Transcript der Rohtext geliefert wurde (fürs UI, Ticket-0009).
    Text {
        text: String,
        cleanup_fell_back: bool,
    },
    /// Leerer Transcript — nichts einzufügen.
    Empty,
    /// Spracherkennung fehlgeschlagen.
    SttFailed(TalkerError),
}

/// Live-Diktat: braucht eine Engine, die auch streamt.
impl<T: Transcriber + StreamingTranscriber, C: LlmCleaner> DictationWorker<T, C> {
    /// Startet eine Live-Utterance. Der Cleanup-Modus wird mit der Erlaubnismatrix
    /// des Einfüge-Verhaltens aufgelöst (REQ-STREAM-010).
    pub fn live_begin(&mut self, frontmost: Option<&str>, cfg: &Config) -> LiveStart {
        self.live = None;
        if cfg.insert_mode == InsertMode::AtEnd {
            return LiveStart::Unavailable;
        }
        self.refresh_llm_availability();
        let resolved = cfg.insert_mode.resolve(resolve_mode(cfg, frontmost));
        if cfg.stt_model_dir != self.stt_dir || self.transcriber.is_none() {
            match self.load_stt_timed(&cfg.stt_model_dir) {
                Ok(t) => {
                    self.transcriber = Some(t);
                    self.stt_dir = cfg.stt_model_dir.clone();
                }
                Err(e) => log::warn!("STT-Neuladen fehlgeschlagen — {e}"),
            }
        }
        let Some(transcriber) = self.transcriber.as_mut() else {
            return LiveStart::Unavailable;
        };
        transcriber.begin();
        if resolved.uses_llm() && self.cleaner.is_none() && !self.cleaner_failed {
            self.load_cleaner_timed();
        }
        self.live = Some(LiveSession {
            insert_mode: cfg.insert_mode,
            cfg: Config {
                cleanup_mode: resolved,
                ..cfg.clone()
            },
            committer: PhraseCommitter::default(),
            emitted_any: false,
        });
        LiveStart::Started
    }

    /// Ein Chunk (16 kHz mono) → null oder mehr Einfüge-Aufträge.
    pub fn live_chunk(&mut self, pcm: &[f32]) -> Vec<Outcome> {
        let (Some(live), Some(transcriber)) = (self.live.as_mut(), self.transcriber.as_mut())
        else {
            return Vec::new();
        };
        let t0 = Instant::now();
        let fed = transcriber.feed(pcm);
        add_time(&mut self.timing.stt, t0.elapsed());
        let update = match fed {
            Ok(update) => update,
            Err(e) => {
                log::error!("Live-STT fehlgeschlagen: {e}");
                return vec![Outcome::Rejected("Spracherkennung fehlgeschlagen")];
            }
        };
        let raw = match live.insert_mode {
            InsertMode::PerSentence => update.endpoint.then_some(update.text),
            InsertMode::PerPhrase if update.endpoint => live.committer.flush(&update.text),
            InsertMode::PerPhrase => live.committer.update(&update.text),
            InsertMode::AtEnd => None,
        };
        raw.and_then(|raw| self.live_segment(&raw))
            .into_iter()
            .collect()
    }

    /// PTT losgelassen: der noch offene Rest, danach ist die Sitzung zu.
    pub fn live_end(&mut self) -> Vec<Outcome> {
        let (Some(live), Some(transcriber)) = (self.live.as_mut(), self.transcriber.as_mut())
        else {
            return Vec::new();
        };
        let t0 = Instant::now();
        let finished = transcriber.finish();
        add_time(&mut self.timing.stt, t0.elapsed());
        let rest = match finished {
            Ok(rest) => rest,
            Err(e) => {
                log::error!("Live-STT fehlgeschlagen: {e}");
                self.live = None;
                return vec![Outcome::Rejected("Spracherkennung fehlgeschlagen")];
            }
        };
        let raw = match live.insert_mode {
            InsertMode::PerSentence => Some(rest),
            InsertMode::PerPhrase => live.committer.flush(&rest),
            InsertMode::AtEnd => None,
        };
        let out = raw.and_then(|raw| self.live_segment(&raw));
        self.live = None;
        out.into_iter().collect()
    }

    /// Ein erkanntes Segment → Vokabular-Korrektur + Cleanup → Einfüge-Auftrag.
    /// Leere Segmente erzeugen keinen; ab dem zweiten steht ein Leerzeichen davor.
    fn live_segment(&mut self, raw: &str) -> Option<Outcome> {
        let live = self.live.as_mut()?;
        let raw = raw.trim();
        if raw.is_empty() {
            return None;
        }
        let cleaner = live
            .cfg
            .cleanup_mode
            .uses_llm()
            .then_some(self.cleaner.as_mut())
            .flatten()
            .map(|c| {
                c.set_mode(live.cfg.cleanup_mode);
                c.set_vocab(&live.cfg.vocabulary);
                c as &mut dyn LlmCleaner
            });
        let mut cleanup_time = None;
        let mut timed_cleaner = cleaner.map(|inner| TimedCleaner {
            inner,
            elapsed: &mut cleanup_time,
        });
        let (text, cleanup_fell_back) = postprocess(
            raw.to_string(),
            &live.cfg,
            timed_cleaner.as_mut().map(|c| c as &mut dyn LlmCleaner),
            self.cleaner_failed,
        );
        if let Some(t) = cleanup_time {
            add_time(&mut self.timing.cleanup, t);
        }
        log::debug!("Live-Segment: {} Zeichen", text.chars().count());
        let text = if live.emitted_any {
            format!(" {text}")
        } else {
            text
        };
        live.emitted_any = true;
        Some(Outcome::Inject {
            text,
            cleanup_fell_back,
        })
    }
}

/// Fährt eine ganze Live-Utterance: rohe Callback-Blöcke → 560-ms-Chunks →
/// Worker; jeder Einfüge-Auftrag geht sofort an `emit`. Endet der Block-Strom
/// (Aufnahme gestoppt), wird der Rest eingespeist und die Sitzung beendet.
/// Ist Live nicht möglich, wird die Aufnahme stattdessen als ganze Utterance
/// verarbeitet (Fallback auf Am Ende).
pub fn run_live<T: Transcriber + StreamingTranscriber, C: LlmCleaner>(
    worker: &mut DictationWorker<T, C>,
    blocks: impl IntoIterator<Item = Vec<f32>>,
    format: (u16, u32),
    frontmost: Option<&str>,
    cfg: &Config,
    emit: &mut dyn FnMut(Outcome),
) {
    let (channels, sample_rate) = format;
    let mut assembler = match audio::ChunkAssembler::new(channels, sample_rate) {
        Ok(a) => a,
        Err(e) => {
            log::error!("Live-Audio unbrauchbar: {e}");
            emit(Outcome::Rejected("Aufnahme fehlgeschlagen"));
            return;
        }
    };
    if worker.live_begin(frontmost, cfg) == LiveStart::Unavailable {
        let raw: Vec<f32> = blocks.into_iter().flatten().collect();
        match audio::to_mono_16k(&raw, channels, sample_rate) {
            Ok(pcm) => emit(worker.handle(&pcm, frontmost, cfg)),
            Err(e) => {
                log::error!("Fallback-Aufnahme unbrauchbar: {e}");
                emit(Outcome::Rejected("Aufnahme fehlgeschlagen"));
            }
        }
        return;
    }
    for block in blocks {
        match assembler.push(&block) {
            Ok(chunks) => {
                for chunk in chunks {
                    log::debug!("Live-Chunk: {} Samples @16 kHz", chunk.len());
                    worker.live_chunk(&chunk).into_iter().for_each(&mut *emit);
                }
            }
            Err(e) => {
                log::error!("Live-Audio fehlgeschlagen: {e}");
                emit(Outcome::Rejected("Aufnahme fehlgeschlagen"));
            }
        }
    }
    // Der Block-Strom endet mit `Feed::End`: ab hier läuft nur noch der Rest nach dem
    // Loslassen. Die Zeit der Chunks davor gehört nicht in „Loslassen→Text (STT · Cleanup)“.
    let _ = worker.take_timing();
    match assembler.flush() {
        Ok(rest) if !rest.is_empty() => {
            worker.live_chunk(&rest).into_iter().for_each(&mut *emit);
        }
        Ok(_) => {}
        Err(e) => log::warn!("Live-Audio-Rest verworfen: {e}"),
    }
    worker.live_end().into_iter().for_each(&mut *emit);
}

/// Vokabular-Korrektur und optionaler Cleanup für einen erkannten Text (ganze
/// Utterance oder Live-Segment). Liefert Text + „Cleanup fiel auf Rohtext zurück".
fn postprocess(
    raw: String,
    cfg: &Config,
    cleaner: Option<&mut dyn LlmCleaner>,
    cleaner_failed: bool,
) -> (String, bool) {
    // Deterministische Vokabular-Korrektur (Kölner Phonetik) — vor dem
    // Cleanup, wirkt damit auch im Roh-Modus; per Config abschaltbar.
    let raw = if cfg.phonetic_matching {
        vocab_match::apply(&raw, &cfg.vocabulary)
    } else {
        raw
    };
    match cleaner {
        Some(c) => {
            let t2 = Instant::now();
            let (cleaned, fell_back) = cleanup::clean_with_fallback(c, &raw);
            log::debug!(
                "Cleaned Transcript: {} Zeichen (Cleanup {} ms)",
                cleaned.chars().count(),
                t2.elapsed().as_millis()
            );
            (cleaned, fell_back)
        }
        // Cleanup gewollt, aber Modell nicht ladbar → ebenfalls sichtbar machen.
        None => {
            let degraded = cfg.cleanup_mode.uses_llm() && cleaner_failed;
            (raw, degraded)
        }
    }
}

/// Verarbeitet eine Utterance: STT → optionale phonetische Vokabular-Korrektur
/// → optionaler Cleanup (mit Raw-Fallback). `cleaner_failed` meldet, dass ein
/// gewollter Cleaner nicht ladbar war → Ergebnis gilt als Fallback.
fn process_utterance(
    pcm: &[f32],
    cfg: &Config,
    transcriber: &mut dyn Transcriber,
    cleaner: Option<&mut dyn LlmCleaner>,
    cleaner_failed: bool,
) -> Processed {
    let audio_ms = audio::duration_ms(pcm);
    let t1 = Instant::now();
    let raw = match transcriber.transcribe(pcm) {
        Ok(text) if text.is_empty() => return Processed::Empty,
        Ok(text) => text,
        Err(e) => return Processed::SttFailed(e),
    };
    log::debug!(
        "Raw Transcript: {} Zeichen ({audio_ms} ms Audio, STT {} ms)",
        raw.chars().count(),
        t1.elapsed().as_millis()
    );
    let (text, cleanup_fell_back) = postprocess(raw, cfg, cleaner, cleaner_failed);
    Processed::Text {
        text,
        cleanup_fell_back,
    }
}

/// Löst den effektiven Cleanup-Modus auf (Kontext-Awareness, Ticket-0026):
/// Feature aktiv + Regel-Match auf die frontmost bundle-id → Regel-Modus;
/// sonst (Feature aus, kein Match, App unbekannt) → manueller `cleanup_mode`.
pub fn resolve_mode(cfg: &Config, frontmost_bundle_id: Option<&str>) -> CleanupMode {
    if !cfg.context_aware_enabled {
        return cfg.cleanup_mode;
    }
    frontmost_bundle_id
        .and_then(|id| {
            cfg.context_rules
                .iter()
                .find(|(bundle_id, _)| bundle_id == id)
                .map(|(_, mode)| *mode)
        })
        .unwrap_or(cfg.cleanup_mode)
}

/// Sammelt, was beim Einfügen der Ergebnisse eines Auftrags passiert.
#[derive(Default)]
struct Delivery {
    /// Erster Hinweis für den Indicator.
    hint: Option<&'static str>,
    inserted: bool,
    /// Erstes Ende ohne Erfolg (Fehler/verworfen); gewinnt gegen `inserted`.
    ended: Option<DictationResult>,
    /// Zeitpunkt des zuletzt fertigen Texts (vor dem Einfügen).
    text_at: Option<audio::HostInstant>,
    /// Dauer des zuletzt eingefügten Texts.
    injected: Option<Duration>,
    segments: u32,
    first_segment_at: Option<audio::HostInstant>,
}

/// Ablehnungsgrund des Workers → Ergebnis der Messzeile.
fn rejection_result(hint: &'static str) -> DictationResult {
    if hint == NOTHING_RECOGNIZED {
        DictationResult::Discarded(NOTHING_RECOGNIZED_REASON)
    } else {
        DictationResult::Error(hint.to_string())
    }
}

/// Fügt ein Ergebnis ein und hält fest, was passiert ist.
fn deliver(
    outcome: Outcome,
    inject: &mut dyn FnMut(&str) -> crate::error::Result<()>,
    delivery: &mut Delivery,
) {
    let t0 = Instant::now();
    let hint = match outcome {
        Outcome::Inject {
            text,
            cleanup_fell_back,
        } => {
            let ready_at = audio::HostInstant::now();
            delivery.text_at = Some(ready_at);
            delivery.segments += 1;
            delivery.first_segment_at.get_or_insert(ready_at);
            match inject(&text) {
                Ok(()) => {
                    delivery.inserted = true;
                    delivery.injected = Some(t0.elapsed());
                    // Sichtbar statt still (Ticket-0009): Text ist drin, aber roh.
                    cleanup_fell_back.then_some("Cleanup übersprungen — Rohtext eingefügt")
                }
                Err(e) => {
                    log::error!("Injection fehlgeschlagen: {e}");
                    delivery.ended.get_or_insert(DictationResult::Error(format!(
                        "Einfügen fehlgeschlagen: {e}"
                    )));
                    Some("Einfügen fehlgeschlagen")
                }
            }
        }
        Outcome::Rejected(hint) => {
            delivery.ended.get_or_insert(rejection_result(hint));
            Some(hint)
        }
    };
    log::debug!(
        "Einfügen abgeschlossen nach {} ms ({})",
        t0.elapsed().as_millis(),
        hint.unwrap_or("ok")
    );
    delivery.hint = delivery.hint.or(hint);
}

/// Verarbeitet einen Auftrag vollständig: STT → Cleanup → Einfügen über `inject`.
/// Liefert den ersten Hinweis für den Indicator und die fertige Messzeile.
/// Bricht nie hart ab: jeder Fehlerpfad wird zu Hinweis + Ergebnis in der Trace.
/// `debug`: Debug-Mitschnitt; geschrieben wird erst NACH dem Einfügen.
pub fn process_job<T, C>(
    worker: &mut DictationWorker<T, C>,
    job: SttJob,
    cfg: &Config,
    inject: &mut dyn FnMut(&str) -> crate::error::Result<()>,
    debug: Option<(&DebugCapture, &ModelTap)>,
) -> (Option<&'static str>, DictationTrace)
where
    T: Transcriber + StreamingTranscriber,
    C: LlmCleaner,
{
    let mut delivery = Delivery::default();
    // Reste eines früheren, nicht abgeholten Auftrags verwerfen.
    if let Some((_, tap)) = debug {
        tap.take();
    }
    let (mut trace, captured) = match job {
        SttJob::Utterance {
            pcm,
            frontmost,
            captured,
            trace,
        } => {
            let outcome = worker.handle(&pcm, frontmost.as_deref(), cfg);
            deliver(outcome, inject, &mut delivery);
            (*trace, Some(captured))
        }
        SttJob::Live { feed } => {
            let mut trace = *feed.trace;
            let mut blocks = FeedBlocks::new(feed.blocks);
            run_live(
                worker,
                &mut blocks,
                feed.format,
                feed.frontmost.as_deref(),
                cfg,
                &mut |outcome| deliver(outcome, inject, &mut delivery),
            );
            let mut captured = None;
            if let Some(ended) = blocks.take_ended() {
                trace.apply_capture(ended.released_at, &ended.captured);
                captured = Some(ended.captured);
            }
            trace.segments = Some(delivery.segments);
            trace.first_segment_at = delivery.first_segment_at;
            (trace, captured)
        }
    };
    let timing = worker.take_timing();
    trace.result = match delivery.ended {
        Some(ended) => ended,
        None if delivery.inserted => DictationResult::Inserted,
        None => DictationResult::Discarded(NOTHING_RECOGNIZED_REASON),
    };
    let failed = matches!(trace.result, DictationResult::Error(_));
    trace.stt = timing.stt.filter(|_| !failed);
    trace.cleanup = timing.cleanup.filter(|_| !failed);
    trace.text_at = delivery.text_at;
    trace.injected = delivery.injected;
    trace.model_loads = worker.take_model_loads();
    if let Some((capture, tap)) = debug {
        match &captured {
            Some(captured) => capture.write(trace.nr, captured, &tap.take()),
            None => log::debug!("Debug-Mitschnitt: keine Rohaufnahme vorhanden"),
        }
    }
    (delivery.hint, trace)
}

/// Zustand einer laufenden PTT-Utterance zwischen Tastendruck und Loslassen —
/// ersetzt die früheren `thread_local!`-Zellen in `main.rs` (Ticket-0038).
/// Läuft ausschließlich auf dem Main-Thread (Hotkey-Callbacks), geteilt über
/// `Arc<Mutex<PttSession>>`, weil `hotkey::install` `Send`-Closures verlangt.
pub struct PttSession {
    /// Liefert je Tastendruck die Audioquelle (App: Mikrofon, Tests: WAV).
    open_source: SourceFactory,
    /// Nimmt die Messzeile jeder Dictation entgegen, die in der Session endet.
    report: TraceReport,
    /// Laufende Diktat-Nummer ab App-Start (jeder Tastendruck zählt, auch gesperrte).
    nr: u64,
    /// Messpunkte der laufenden Dictation; der Start-Thread ergänzt Mikro-Bereitschaft und Gerät.
    trace: Option<Arc<Mutex<DictationTrace>>>,
    slot: Option<Arc<Mutex<MicSlot<audio::Recording>>>>,
    frontmost: Option<String>,
    /// Die laufende Aufnahme speist das Live-Diktat (Block-Kanal).
    live: bool,
}

/// Erzeugt die Audioquelle für eine Aufnahme aus der aktuellen Config.
pub type SourceFactory = Box<dyn Fn(&Config) -> Box<dyn audio::AudioSource> + Send + Sync>;

/// Zustand des (im Hintergrund startenden) Mikrofons zwischen Press und Release.
enum MicSlot<R> {
    Starting,
    Ready(R),
    /// Taste vor dem Start losgelassen — eine spät fertige Aufnahme wird verworfen.
    Cancelled,
    Failed,
}

/// Was `release` im Slot vorfindet.
enum Taken<R> {
    Ready(R),
    /// Mikrofon war noch nicht gestartet: Aufnahme abgebrochen.
    StartAborted,
    /// Nichts (mehr) da: Start fehlgeschlagen oder schon abgeholt.
    Gone,
}

/// Der Start-Thread meldet sein Ergebnis. Der Callback läuft unter dem
/// Slot-Lock — ein gleichzeitiges `release` wartet, bis er durch ist.
fn complete_start<R>(
    slot: &Mutex<MicSlot<R>>,
    started: Result<R, TalkerError>,
    on_ready: impl FnOnce(Result<&R, TalkerError>),
) {
    let Ok(mut guard) = slot.lock() else { return };
    if matches!(*guard, MicSlot::Cancelled) {
        return;
    }
    match started {
        Ok(rec) => {
            on_ready(Ok(&rec));
            *guard = MicSlot::Ready(rec);
        }
        Err(e) => {
            *guard = MicSlot::Failed;
            on_ready(Err(e));
        }
    }
}

fn take_on_release<R>(slot: &Mutex<MicSlot<R>>) -> Taken<R> {
    let Ok(mut guard) = slot.lock() else {
        return Taken::Gone;
    };
    match std::mem::replace(&mut *guard, MicSlot::Cancelled) {
        MicSlot::Ready(rec) => Taken::Ready(rec),
        MicSlot::Starting => Taken::StartAborted,
        MicSlot::Cancelled | MicSlot::Failed => Taken::Gone,
    }
}

/// Ergebnis von [`PttSession::press`].
pub enum PressOutcome {
    /// PTT gesperrt (Setup-Gate, AK 3) — Grund als sichtbarer Hinweis.
    Blocked(String),
    /// Mikrofon startet im Hintergrund; das Ergebnis kommt als [`MicReady`].
    Starting,
}

/// Ergebnis des Mikrofon-Starts, gemeldet vom Start-Thread.
pub enum MicReady {
    /// Aufnahme läuft, mit dem für diese Utterance aufgelösten Modus.
    Recording {
        level: audio::LevelHandle,
        mode: CleanupMode,
        /// Gesetzt bei Live-Diktat: Strom für den STT-Worker (`run_live`).
        live: Option<LiveFeed>,
    },
    /// Mikrofon-Start fehlgeschlagen (z.B. fehlende Permission).
    Failed(TalkerError),
}

fn lock_trace(trace: &Mutex<DictationTrace>) -> MutexGuard<'_, DictationTrace> {
    trace.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Rohe Audio-Blöcke einer Live-Aufnahme samt Format, Ziel-App und den bis zum
/// Mikrofon-Start gemessenen Punkten.
pub struct LiveFeed {
    pub trace: Box<DictationTrace>,
    pub frontmost: Option<String>,
    pub blocks: std::sync::mpsc::Receiver<audio::Feed>,
    pub format: (u16, u32),
}

/// Rohsamples der Blöcke eines Live-Kanals, bis `Feed::End` oder das Kanal-Ende kommt;
/// das `End` bleibt für die Messzeile erhalten.
pub struct FeedBlocks {
    rx: std::sync::mpsc::Receiver<audio::Feed>,
    ended: Option<Box<audio::Ended>>,
}

impl FeedBlocks {
    pub fn new(rx: std::sync::mpsc::Receiver<audio::Feed>) -> Self {
        Self { rx, ended: None }
    }

    /// Das Ende der Aufnahme (Loslass-Zeitpunkt + Rohaufnahme), falls es kam.
    pub fn take_ended(&mut self) -> Option<Box<audio::Ended>> {
        self.ended.take()
    }
}

impl Iterator for FeedBlocks {
    type Item = Vec<f32>;

    fn next(&mut self) -> Option<Vec<f32>> {
        match self.rx.recv() {
            Ok(audio::Feed::Block(block)) => Some(block.samples),
            Ok(audio::Feed::End(ended)) => {
                self.ended = Some(ended);
                None
            }
            Err(_) => None,
        }
    }
}

/// Auftrag an den STT-Worker: eine fertige Utterance (Am Ende) oder ein
/// Live-Strom (Satzweise/Wortgruppenweise), der bis zum Ende der Aufnahme läuft.
pub enum SttJob {
    Utterance {
        pcm: Vec<f32>,
        frontmost: Option<String>,
        captured: audio::Captured,
        trace: Box<DictationTrace>,
    },
    Live {
        feed: LiveFeed,
    },
}

impl SttJob {
    /// Die Messpunkte des Auftrags — z. B. um ihn mit Fehler zu melden, ohne ihn zu verarbeiten.
    pub fn into_trace(self) -> DictationTrace {
        match self {
            SttJob::Utterance { trace, .. } => *trace,
            SttJob::Live { feed } => *feed.trace,
        }
    }
}

/// Ergebnis von [`PttSession::release`].
#[derive(Debug)]
pub enum ReleaseOutcome {
    /// Keine laufende Aufnahme (Press war gesperrt oder scheiterte).
    NoRecording,
    /// Taste vor dem Mikrofon-Start losgelassen — Aufnahme abgebrochen.
    StartAborted,
    /// Live-Aufnahme beendet — der STT-Worker schließt die Utterance selbst ab.
    LiveEnded,
    /// Utterance zu kurz — gilt als versehentlicher Tastendruck, verworfen.
    TooShort { ms: u64 },
    /// PCM + der bei Press erfasste Frontmost, bereit für den STT-Worker; `captured`
    /// ist die rohe Aufnahme, `trace` die Messpunkte bis zum Loslassen.
    Ready {
        pcm: Vec<f32>,
        frontmost: Option<String>,
        ms: u64,
        captured: audio::Captured,
        trace: Box<DictationTrace>,
    },
    /// Stop der Aufnahme fehlgeschlagen.
    StopFailed(TalkerError),
}

impl PttSession {
    pub fn new(open_source: SourceFactory, report: TraceReport) -> Self {
        Self {
            open_source,
            report,
            nr: 0,
            trace: None,
            slot: None,
            frontmost: None,
            live: false,
        }
    }

    /// PTT gedrückt: Setup-Gate prüfen, Frontmost-App VOR Aufnahmestart
    /// erfassen (Kontext-Awareness, Ticket-0026 — bleibt stabil, auch wenn
    /// die App während der Utterance wechselt), Mikrofon im Hintergrund
    /// starten. Der Tastendruck-Callback blockiert nie auf CoreAudio;
    /// `on_ready` meldet den Start (oder seinen Fehler) vom Start-Thread aus.
    /// `pressed_at` stempelt der Event-Tap beim Eintritt (gemeinsame Uhr).
    pub fn press(
        &mut self,
        pressed_at: audio::HostInstant,
        cfg: &Config,
        models: &ModelsState,
        on_ready: impl FnOnce(MicReady) + Send + 'static,
    ) -> PressOutcome {
        self.nr += 1;
        let mut trace = DictationTrace::new(self.nr, pressed_at, cfg.insert_mode, cfg.cleanup_mode);
        if let Some(hint) = models.get(ModelId::Stt).setup_hint() {
            trace.result = DictationResult::Blocked;
            (self.report)(trace);
            return PressOutcome::Blocked(hint);
        }
        let frontmost = injection::frontmost_bundle_id();
        let live = cfg.insert_mode != InsertMode::AtEnd;
        let mut mode = resolve_mode(cfg, frontmost.as_deref());
        if live {
            // Im Live-Betrieb zeigt der Indicator den tatsächlich wirkenden Modus.
            mode = cfg.insert_mode.resolve(mode);
        }
        trace.cleanup_mode = mode;
        let source = (self.open_source)(cfg);
        let slot = Arc::new(Mutex::new(MicSlot::Starting));
        let trace = Arc::new(Mutex::new(trace));
        self.slot = Some(Arc::clone(&slot));
        self.trace = Some(Arc::clone(&trace));
        self.frontmost = frontmost.clone();
        self.live = live;
        let report = Arc::clone(&self.report);
        std::thread::spawn(move || {
            let (tx, rx) = std::sync::mpsc::channel();
            let started = audio::start(source, live.then_some(tx));
            let ready_at = audio::HostInstant::now();
            complete_start(&slot, started, |result| match result {
                Ok(rec) => {
                    let snapshot = {
                        let mut t = lock_trace(&trace);
                        t.mic_ready_at = Some(ready_at);
                        t.device = Some(rec.info().clone());
                        t.clone()
                    };
                    let format = rec.info().format;
                    on_ready(MicReady::Recording {
                        level: rec.level(),
                        mode,
                        live: live.then(|| LiveFeed {
                            trace: Box::new(snapshot),
                            frontmost,
                            blocks: rx,
                            format: (format.channels, format.sample_rate),
                        }),
                    });
                }
                Err(e) => {
                    let mut failed = lock_trace(&trace).clone();
                    failed.result = DictationResult::Error(e.to_string());
                    report(failed);
                    on_ready(MicReady::Failed(e));
                }
            });
        });
        PressOutcome::Starting
    }

    /// PTT losgelassen: Aufnahme stoppen, Mindestlänge prüfen (versehentlicher
    /// Tastendruck), sonst PCM + Frontmost zum Senden an den STT-Worker freigeben.
    /// `released_at` stempelt der Event-Tap beim Eintritt. Enden, die hier
    /// vorzeitig abbrechen, meldet die Session selbst über `report`.
    pub fn release(&mut self, released_at: audio::HostInstant) -> ReleaseOutcome {
        let frontmost = self.frontmost.take();
        let (Some(slot), Some(trace)) = (self.slot.take(), self.trace.take()) else {
            return ReleaseOutcome::NoRecording;
        };
        let mut trace = lock_trace(&trace).clone();
        let rec = match take_on_release(&slot) {
            Taken::Ready(rec) => rec,
            Taken::StartAborted => {
                trace.released_at = Some(released_at);
                trace.result = DictationResult::Discarded("abgebrochen");
                (self.report)(trace);
                return ReleaseOutcome::StartAborted;
            }
            // Fehlgeschlagener Start ist schon gemeldet.
            Taken::Gone => return ReleaseOutcome::NoRecording,
        };
        if std::mem::take(&mut self.live) {
            rec.finish_live(released_at);
            return ReleaseOutcome::LiveEnded;
        }
        let captured = rec.stop();
        trace.apply_capture(released_at, &captured);
        match captured.to_mono_16k() {
            Ok(pcm) => {
                let ms = audio::duration_ms(&pcm);
                if ms < audio::MIN_UTTERANCE_MS {
                    trace.result = DictationResult::Discarded("zu kurz");
                    (self.report)(trace);
                    ReleaseOutcome::TooShort { ms }
                } else {
                    ReleaseOutcome::Ready {
                        pcm,
                        frontmost,
                        ms,
                        captured,
                        trace: Box::new(trace),
                    }
                }
            }
            Err(e) => {
                trace.result = DictationResult::Error(e.to_string());
                (self.report)(trace);
                ReleaseOutcome::StopFailed(e)
            }
        }
    }
}

/// Kann diese Config überhaupt einen LLM-Cleanup brauchen? (Fallback-Modus
/// oder irgendeine aktive Kontext-Regel.) Steuert Vorladen/Entladen des
/// Modells im Worker — pro Utterance entscheidet `resolve_mode`.
fn config_wants_llm(cfg: &Config) -> bool {
    cfg.cleanup_mode.uses_llm()
        || (cfg.context_aware_enabled && cfg.context_rules.iter().any(|(_, m)| m.uses_llm()))
}

/// Roh-Modus setzt Cleaner und Fehler-Cache zurück: ein späterer Wechsel auf
/// einen LLM-Modus versucht das Laden dadurch erneut.
fn reset_cleaner_on_raw_mode<C>(mode: CleanupMode, cleaner: &mut Option<C>, failed: &mut bool) {
    if !mode.uses_llm() {
        *cleaner = None;
        *failed = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cleanup::FakeCleaner;
    use crate::stt::FakeTranscriber;

    fn text_of(p: Processed) -> (String, bool) {
        match p {
            Processed::Text {
                text,
                cleanup_fell_back,
            } => (text, cleanup_fell_back),
            other => panic!("erwartet Text, bekam {other:?}"),
        }
    }

    #[test]
    fn pure_stt_path_returns_raw_transcript() {
        let cfg = Config {
            cleanup_mode: CleanupMode::Raw,
            ..Config::default()
        };
        let mut t = FakeTranscriber {
            reply: "hallo welt",
        };

        let result = process_utterance(&[0.0; 1600], &cfg, &mut t, None, false);

        assert_eq!(text_of(result), ("hallo welt".to_string(), false));
    }

    #[test]
    fn stt_vocab_cleanup_path_corrects_and_cleans() {
        let cfg = Config {
            vocabulary: vec!["Claude CLI".to_string()],
            ..Config::default()
        };
        assert!(cfg.phonetic_matching, "Default muss phonetisch matchen");
        let mut t = FakeTranscriber {
            reply: "die Clotzelei ähm ist offen",
        };
        // Der Fake echot sein Ergebnis — hier belegt er, dass er die
        // vokabular-korrigierte Fassung (nicht den Rohtext) erhält.
        let mut c = FakeCleaner {
            result: Ok("Die Claude CLI ist offen.".to_string()),
        };

        let result = process_utterance(&[0.0; 1600], &cfg, &mut t, Some(&mut c), false);

        assert_eq!(
            text_of(result),
            ("Die Claude CLI ist offen.".to_string(), false)
        );
    }

    #[test]
    fn vocab_match_applies_before_cleanup_and_in_raw_mode() {
        let cfg = Config {
            cleanup_mode: CleanupMode::Raw,
            vocabulary: vec!["Claude CLI".to_string()],
            ..Config::default()
        };
        let mut t = FakeTranscriber {
            reply: "die Clotzelei ist offen",
        };

        let result = process_utterance(&[0.0; 1600], &cfg, &mut t, None, false);

        assert_eq!(
            text_of(result),
            ("die Claude CLI ist offen".to_string(), false)
        );
    }

    #[test]
    fn cleaner_error_falls_back_to_raw_with_flag() {
        let cfg = Config::default();
        let mut t = FakeTranscriber {
            reply: "roher text bleibt",
        };
        let mut c = FakeCleaner {
            result: Err(TalkerError::Cleanup("Timeout".into())),
        };

        let result = process_utterance(&[0.0; 1600], &cfg, &mut t, Some(&mut c), false);

        assert_eq!(text_of(result), ("roher text bleibt".to_string(), true));
    }

    #[test]
    fn unloadable_cleaner_marks_fallback_when_llm_mode_wanted() {
        let cfg = Config::default();
        assert!(cfg.cleanup_mode.uses_llm());
        let mut t = FakeTranscriber {
            reply: "roher text",
        };

        let result = process_utterance(&[0.0; 1600], &cfg, &mut t, None, true);

        assert_eq!(text_of(result), ("roher text".to_string(), true));
    }

    #[test]
    fn empty_transcript_yields_empty_outcome() {
        let cfg = Config::default();
        let mut t = FakeTranscriber { reply: "x" };

        let result = process_utterance(&[], &cfg, &mut t, None, false);

        assert!(matches!(result, Processed::Empty));
    }

    #[test]
    fn stt_error_is_reported_not_swallowed() {
        struct FailingTranscriber;
        impl Transcriber for FailingTranscriber {
            fn transcribe(&mut self, _pcm: &[f32]) -> crate::error::Result<String> {
                Err(TalkerError::Stt("Modell kaputt".into()))
            }
        }
        let cfg = Config::default();

        let result = process_utterance(&[0.0; 1600], &cfg, &mut FailingTranscriber, None, false);

        assert!(matches!(result, Processed::SttFailed(_)));
    }

    fn ctx_cfg(enabled: bool) -> Config {
        Config {
            cleanup_mode: CleanupMode::Business,
            context_aware_enabled: enabled,
            context_rules: vec![
                ("com.apple.Terminal".to_string(), CleanupMode::LlmOptimized),
                ("com.apple.mail".to_string(), CleanupMode::Casual),
            ],
            ..Config::default()
        }
    }

    #[test]
    fn resolve_mode_feature_off_ignores_rules() {
        let cfg = ctx_cfg(false);
        assert_eq!(
            resolve_mode(&cfg, Some("com.apple.Terminal")),
            CleanupMode::Business
        );
    }

    #[test]
    fn resolve_mode_match_wins_over_manual_mode() {
        let cfg = ctx_cfg(true);
        assert_eq!(
            resolve_mode(&cfg, Some("com.apple.Terminal")),
            CleanupMode::LlmOptimized
        );
        assert_eq!(
            resolve_mode(&cfg, Some("com.apple.mail")),
            CleanupMode::Casual
        );
    }

    #[test]
    fn resolve_mode_no_match_falls_back_to_manual_mode() {
        let cfg = ctx_cfg(true);
        assert_eq!(
            resolve_mode(&cfg, Some("com.unbekannt.app")),
            CleanupMode::Business
        );
    }

    #[test]
    fn resolve_mode_unknown_frontmost_falls_back() {
        let cfg = ctx_cfg(true);
        assert_eq!(resolve_mode(&cfg, None), CleanupMode::Business);
    }

    #[test]
    fn resolve_mode_empty_rules_fall_back() {
        let cfg = Config {
            cleanup_mode: CleanupMode::Raw,
            context_aware_enabled: true,
            context_rules: Vec::new(),
            ..Config::default()
        };
        assert_eq!(
            resolve_mode(&cfg, Some("com.apple.Terminal")),
            CleanupMode::Raw
        );
    }

    // PttSession: press()/release() rühren nur dann echte Hardware an (Audio,
    // Frontmost-App), wenn das Setup-Gate offen ist bzw. eine Aufnahme läuft —
    // beides ohne Hardware nicht fakebar (kein Trait dafür, reduzierter Scope,
    // Ticket-0038). Getestet wird deshalb nur, was ohne Hardware entscheidbar
    // ist: das Setup-Gate selbst und der Leerlauf-Fall von `release()`.

    fn mic_factory() -> SourceFactory {
        Box::new(|cfg| Box::new(audio::MicSource::new(cfg.mic_device.clone())))
    }

    // ---- asynchroner Mikrofon-Start (MicSlot) ----

    struct DropFlag(Arc<std::sync::atomic::AtomicBool>);
    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[test]
    fn mic_started_before_release_hands_the_recording_over() {
        let slot = Mutex::new(MicSlot::<u32>::Starting);
        let mut seen = None;
        complete_start(&slot, Ok(7), |r| {
            seen = Some(r.copied().map_err(|e| e.to_string()))
        });
        assert_eq!(seen, Some(Ok(7)));
        assert!(matches!(take_on_release(&slot), Taken::Ready(7)));
        // Ein zweites Loslassen findet nichts mehr.
        assert!(matches!(take_on_release(&slot), Taken::Gone));
    }

    /// Grenzfall: Taste vor dem Mikrofon-Start losgelassen → abgebrochen, die
    /// spät fertige Aufnahme wird sofort verworfen, nichts wird gemeldet.
    #[test]
    fn release_before_mic_start_aborts_and_drops_the_late_recording() {
        let slot = Mutex::new(MicSlot::<DropFlag>::Starting);
        assert!(matches!(take_on_release(&slot), Taken::StartAborted));
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut called = false;
        complete_start(&slot, Ok(DropFlag(Arc::clone(&dropped))), |_| called = true);
        assert!(!called, "abgebrochener Start darf nichts melden");
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    }

    /// Fehlerpfad: der Fehler wird gemeldet; das Loslassen danach ist still.
    #[test]
    fn mic_start_failure_is_reported_and_release_stays_silent() {
        let slot = Mutex::new(MicSlot::<u32>::Starting);
        let mut error = None;
        complete_start(
            &slot,
            Err(TalkerError::Audio("kein Mikrofon".into())),
            |r| {
                error = r.err().map(|e| e.to_string());
            },
        );
        assert!(error.is_some_and(|e| e.contains("kein Mikrofon")));
        assert!(matches!(take_on_release(&slot), Taken::Gone));
    }

    #[test]
    fn press_blocked_by_setup_gate_never_touches_audio() {
        let models = ModelsState::new(ModelState::Missing, ModelState::Missing);
        let mut session = PttSession::new(mic_factory(), Arc::new(|_| {}));

        let outcome = session.press(
            audio::HostInstant::now(),
            &Config::default(),
            &models,
            |_| {},
        );

        assert!(matches!(outcome, PressOutcome::Blocked(_)));
    }

    #[test]
    fn release_without_a_press_reports_no_recording() {
        let mut session = PttSession::new(mic_factory(), Arc::new(|_| {}));

        assert!(matches!(
            session.release(audio::HostInstant::now()),
            ReleaseOutcome::NoRecording
        ));
    }

    // ---- PttSession mit WAV-Quelle: Diktat-Nummer, Messpunkte, vorzeitige Enden ----

    type Reports = Arc<Mutex<Vec<DictationTrace>>>;

    fn collecting_report() -> (TraceReport, Reports) {
        let reports: Reports = Arc::default();
        let sink = Arc::clone(&reports);
        (Arc::new(move |t| sink.lock().unwrap().push(t)), reports)
    }

    fn stt_ready() -> ModelsState {
        ModelsState::new(ModelState::Ready, ModelState::Missing)
    }

    fn wav_opts(block_frames: usize, start_delay_ms: u64) -> audio::WavOptions {
        audio::WavOptions {
            format: audio::AudioFormat {
                channels: 1,
                sample_rate: 16_000,
            },
            block_frames,
            start_delay: std::time::Duration::from_millis(start_delay_ms),
            pace: audio::Pace::Immediate,
            epoch: audio::HostInstant::from_nanos(1),
        }
    }

    /// Factory über einer WAV-Quelle; die `SourceDone`-Signale kommen über den Kanal heraus.
    fn wav_factory(
        samples: Vec<f32>,
        done: std::sync::mpsc::Sender<audio::SourceDone>,
    ) -> SourceFactory {
        let done = Mutex::new(done);
        Box::new(move |_| {
            let (source, signal) =
                audio::WavSource::from_samples(samples.clone(), wav_opts(160, 0));
            let _ = done.lock().unwrap().send(signal);
            Box::new(source)
        })
    }

    struct FailingSource;
    impl audio::AudioSource for FailingSource {
        fn open(self: Box<Self>, _: audio::OnBlock) -> crate::error::Result<audio::Opened> {
            Err(TalkerError::Audio("kein Mikrofon verfügbar".into()))
        }
    }

    /// Ein Diktat vom Tastendruck bis zur Quelle-fertig; liefert das Release-Ergebnis.
    fn dictate_once(
        session: &mut PttSession,
        done: &std::sync::mpsc::Receiver<audio::SourceDone>,
    ) -> ReleaseOutcome {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let outcome = session.press(
            audio::HostInstant::now(),
            &Config::default(),
            &stt_ready(),
            move |ready| {
                let _ = ready_tx.send(matches!(ready, MicReady::Recording { .. }));
            },
        );
        assert!(matches!(outcome, PressOutcome::Starting));
        assert!(
            ready_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
        );
        let signal = done
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(signal.wait(std::time::Duration::from_secs(5)));
        session.release(audio::HostInstant::now())
    }

    /// REQ-DIAG-012: Nr. zählt ab 1 fortlaufend, Kaltstart nur bei Nr. 1.
    #[test]
    fn dictation_numbers_count_from_one_and_only_the_first_is_a_cold_start() {
        let (report, reports) = collecting_report();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        // 100 ms Audio → „zu kurz" → die Session meldet selbst.
        let mut session = PttSession::new(wav_factory(vec![0.1; 1_600], done_tx), report);
        for _ in 0..2 {
            assert!(matches!(
                dictate_once(&mut session, &done_rx),
                ReleaseOutcome::TooShort { .. }
            ));
        }
        let reports = reports.lock().unwrap();
        let nrs: Vec<_> = reports.iter().map(|t| (t.nr, t.cold_start)).collect();
        assert_eq!(nrs, [(1, true), (2, false)]);
        assert!(
            reports
                .iter()
                .all(|t| t.result == DictationResult::Discarded("zu kurz"))
        );
    }

    /// Messpunkte bis zum Loslassen: Mikro bereit, Blöcke, Signal, Gerät, Dauer.
    #[test]
    fn early_end_trace_carries_start_marks_device_and_recorded_duration() {
        let (report, reports) = collecting_report();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let mut session = PttSession::new(wav_factory(vec![0.1; 1_600], done_tx), report);
        let pressed_at = audio::HostInstant::now();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        session.press(pressed_at, &Config::default(), &stt_ready(), move |_| {
            let _ = ready_tx.send(());
        });
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert!(
            done_rx
                .recv()
                .unwrap()
                .wait(std::time::Duration::from_secs(5))
        );
        session.release(audio::HostInstant::now());
        let t = reports.lock().unwrap()[0].clone();
        assert!(t.mic_ready_at.unwrap() >= pressed_at);
        assert!(t.first_block_at.is_some() && t.first_signal_at.is_some());
        assert!(t.released_at.unwrap() >= t.mic_ready_at.unwrap());
        assert_eq!(t.recorded, Some(std::time::Duration::from_millis(100)));
        assert!(t.device.unwrap().device.starts_with("WAV: "));
    }

    /// REQ-DICT-002: Setup-Gate offen → genau eine Messzeile „gesperrt", Nr. verbraucht.
    #[test]
    fn blocked_press_reports_one_trace_and_consumes_the_number() {
        let (report, reports) = collecting_report();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let mut session = PttSession::new(wav_factory(vec![0.1; 1_600], done_tx), report);
        let missing = ModelsState::new(ModelState::Missing, ModelState::Missing);
        let outcome = session.press(
            audio::HostInstant::now(),
            &Config::default(),
            &missing,
            |_| {},
        );
        assert!(matches!(outcome, PressOutcome::Blocked(_)));
        assert!(matches!(
            session.release(audio::HostInstant::now()),
            ReleaseOutcome::NoRecording
        ));
        assert!(matches!(
            dictate_once(&mut session, &done_rx),
            ReleaseOutcome::Ready { .. } | ReleaseOutcome::TooShort { .. }
        ));
        let reports = reports.lock().unwrap();
        assert_eq!(reports[0].nr, 1);
        assert_eq!(reports[0].result, DictationResult::Blocked);
        assert_eq!(reports[1].nr, 2);
        assert_eq!(reports.len(), 2, "Gesperrt + zu kurz, sonst nichts");
    }

    /// REQ-DICT-006: Mikrofon-Start scheitert → Fehler wie bisher + `Ergebnis: Fehler`; kein Doppel.
    #[test]
    fn mic_start_failure_reports_an_error_trace_once() {
        let (report, reports) = collecting_report();
        let mut session = PttSession::new(Box::new(|_| Box::new(FailingSource)), report);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        session.press(
            audio::HostInstant::now(),
            &Config::default(),
            &stt_ready(),
            move |ready| {
                let _ = ready_tx.send(matches!(ready, MicReady::Failed(_)));
            },
        );
        assert!(
            ready_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
        );
        assert!(matches!(
            session.release(audio::HostInstant::now()),
            ReleaseOutcome::NoRecording
        ));
        let reports = reports.lock().unwrap();
        assert_eq!(reports.len(), 1);
        assert!(
            matches!(&reports[0].result, DictationResult::Error(e) if e.contains("kein Mikrofon"))
        );
        assert!(reports[0].mic_ready_at.is_none());
    }

    /// Release vor Mikrofon-Start → „verworfen (abgebrochen)"; die spät fertige Aufnahme meldet nichts.
    #[test]
    fn release_before_mic_start_reports_aborted_and_the_late_start_stays_silent() {
        struct SlowSource {
            gate: std::sync::mpsc::Receiver<()>,
            dropped: std::sync::mpsc::Sender<()>,
        }
        struct DropSignal(std::sync::mpsc::Sender<()>);
        impl Drop for DropSignal {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }
        impl audio::AudioSource for SlowSource {
            fn open(self: Box<Self>, _: audio::OnBlock) -> crate::error::Result<audio::Opened> {
                let _ = self.gate.recv();
                Ok(audio::Opened {
                    info: audio::SourceInfo {
                        format: audio::AudioFormat {
                            channels: 1,
                            sample_rate: 16_000,
                        },
                        device: "langsam".into(),
                    },
                    stop: Box::new(DropSignal(self.dropped)),
                })
            }
        }
        let (report, reports) = collecting_report();
        let (gate_tx, gate_rx) = std::sync::mpsc::channel();
        let (dropped_tx, dropped_rx) = std::sync::mpsc::channel();
        let source = Mutex::new(Some(SlowSource {
            gate: gate_rx,
            dropped: dropped_tx,
        }));
        let mut session = PttSession::new(
            Box::new(move |_| Box::new(source.lock().unwrap().take().unwrap())),
            report,
        );
        session.press(
            audio::HostInstant::now(),
            &Config::default(),
            &stt_ready(),
            |_| {},
        );
        assert!(matches!(
            session.release(audio::HostInstant::now()),
            ReleaseOutcome::StartAborted
        ));
        // Doppeltes Loslassen: nichts mehr da, keine zweite Zeile.
        assert!(matches!(
            session.release(audio::HostInstant::now()),
            ReleaseOutcome::NoRecording
        ));
        gate_tx.send(()).unwrap();
        dropped_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let reports = reports.lock().unwrap();
        assert_eq!(reports.len(), 1, "die späte Aufnahme meldet nichts");
        assert_eq!(reports[0].result, DictationResult::Discarded("abgebrochen"));
        assert!(reports[0].released_at.is_some());
    }

    /// Release ohne vorherige Taste oder doppelt: keine Messzeile.
    #[test]
    fn release_without_press_reports_nothing() {
        let (report, reports) = collecting_report();
        let mut session = PttSession::new(mic_factory(), report);
        session.release(audio::HostInstant::now());
        session.release(audio::HostInstant::now());
        assert!(reports.lock().unwrap().is_empty());
    }

    /// Ausreichend lange Aufnahme → `Ready` mit Trace, noch keine Messzeile (kommt vom Worker).
    #[test]
    fn long_enough_recording_hands_a_trace_to_the_worker_without_reporting() {
        let (report, reports) = collecting_report();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let mut session = PttSession::new(wav_factory(vec![0.1; 16_000], done_tx), report);
        match dictate_once(&mut session, &done_rx) {
            ReleaseOutcome::Ready {
                trace,
                captured,
                ms,
                ..
            } => {
                assert_eq!(ms, 1_000);
                assert_eq!(captured.raw.len(), 16_000);
                assert_eq!(trace.nr, 1);
                assert!(trace.released_at.is_some() && trace.recorded.is_some());
            }
            other => panic!("Ready erwartet, bekam {other:?}"),
        }
        assert!(reports.lock().unwrap().is_empty());
    }

    #[test]
    fn config_wants_llm_covers_fallback_and_rules() {
        // Feature aus: nur der Fallback-Modus zählt.
        assert!(config_wants_llm(&ctx_cfg(false)));
        assert!(!config_wants_llm(&Config {
            cleanup_mode: CleanupMode::Raw,
            ..Config::default()
        }));
        // Raw-Fallback + aktive LLM-Regel → Modell wird gebraucht.
        assert!(config_wants_llm(&Config {
            cleanup_mode: CleanupMode::Raw,
            context_aware_enabled: true,
            context_rules: vec![("com.foo".into(), CleanupMode::Business)],
            ..Config::default()
        }));
        // Dieselbe Regel inaktiv (Feature aus) → kein Bedarf.
        assert!(!config_wants_llm(&Config {
            cleanup_mode: CleanupMode::Raw,
            context_aware_enabled: false,
            context_rules: vec![("com.foo".into(), CleanupMode::Business)],
            ..Config::default()
        }));
    }

    /// Regression (Guard aus dem Orchestrierungs-Prompt): Feature AUS →
    /// process_utterance über den aufgelösten Modus verhält sich wie heute.
    #[test]
    fn feature_off_process_utterance_is_unchanged() {
        let cfg = ctx_cfg(false);
        let resolved = Config {
            cleanup_mode: resolve_mode(&cfg, Some("com.apple.Terminal")),
            ..cfg.clone()
        };
        assert_eq!(resolved, cfg, "Feature AUS darf die Config nicht ändern");
        let mut t = FakeTranscriber { reply: "hallo" };

        let result = process_utterance(&[0.0; 1600], &resolved, &mut t, None, false);

        // Business-Modus + kein Cleaner + kein Fehler-Cache → Rohtext, kein
        // Fallback-Flag: exakt das heutige Verhalten.
        assert_eq!(text_of(result), ("hallo".to_string(), false));
    }

    #[test]
    fn raw_mode_resets_failed_cleaner_so_reenabling_retries() {
        let mut cleaner: Option<FakeCleaner> = None;
        let mut failed = true;

        reset_cleaner_on_raw_mode(CleanupMode::Raw, &mut cleaner, &mut failed);

        assert!(!failed, "Roh-Modus muss den Fehler-Cache zurücksetzen");
        assert!(cleaner.is_none());
    }

    #[test]
    fn llm_mode_keeps_cleaner_and_failure_cache() {
        let mut cleaner = Some(FakeCleaner {
            result: Ok("x".to_string()),
        });
        let mut failed = false;
        let mode = CleanupMode::ALL
            .into_iter()
            .find(|m| m.uses_llm())
            .expect("mindestens ein LLM-Modus");

        reset_cleaner_on_raw_mode(mode, &mut cleaner, &mut failed);

        assert!(
            cleaner.is_some(),
            "LLM-Modus darf den Cleaner nicht wegwerfen"
        );
        assert!(!failed);
    }

    // ------- DictationWorker (Ticket-0034): Lifecycle über einen Utterance-Strom -------

    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::models::{ModelId, ModelState, ModelsState};

    /// Cleaner-Fake, der Modus-Konfiguration und clean-Aufrufe protokolliert
    /// und den Input markiert zurückgibt (belegt: Cleanup lief wirklich).
    struct ScriptedCleaner {
        log: Rc<RefCell<Vec<String>>>,
    }

    impl LlmCleaner for ScriptedCleaner {
        fn clean(&mut self, raw: &str) -> crate::error::Result<String> {
            self.log.borrow_mut().push(format!("clean:{raw}"));
            Ok(format!("[{raw}]"))
        }
        fn set_mode(&mut self, mode: CleanupMode) {
            self.log.borrow_mut().push(format!("mode:{}", mode.label()));
        }
    }

    fn models_with_gemma(gemma: ModelState) -> Arc<ModelsState> {
        Arc::new(ModelsState::new(ModelState::Ready, gemma))
    }

    fn fake_stt_loader() -> TranscriberLoader<FakeTranscriber> {
        Box::new(|_dir: &Path| Ok(FakeTranscriber { reply: "hallo" }))
    }

    /// Cleaner-Loader, der Ladeversuche zählt und ein gemeinsames Log teilt.
    fn counted_cleaner_loader(
        loads: &Rc<RefCell<usize>>,
        log: &Rc<RefCell<Vec<String>>>,
    ) -> CleanerLoader<ScriptedCleaner> {
        let loads = Rc::clone(loads);
        let log = Rc::clone(log);
        Box::new(move || {
            *loads.borrow_mut() += 1;
            (
                Some(ScriptedCleaner {
                    log: Rc::clone(&log),
                }),
                false,
            )
        })
    }

    fn inject_text(out: Outcome) -> (String, bool) {
        match out {
            Outcome::Inject {
                text,
                cleanup_fell_back,
            } => (text, cleanup_fell_back),
            other => panic!("erwartet Inject, bekam {other:?}"),
        }
    }

    // ---- Live-Diktat (ADR-0009, REQ-STREAM-004..011) ----

    use crate::config::InsertMode;
    use crate::stt::FakeStreaming;

    fn live_cfg(insert_mode: InsertMode, cleanup_mode: CleanupMode) -> Config {
        Config {
            insert_mode,
            cleanup_mode,
            ..Config::default()
        }
    }

    fn live_worker(
        cfg: &Config,
        stt: impl Fn() -> FakeStreaming + 'static,
        log: &Rc<RefCell<Vec<String>>>,
    ) -> DictationWorker<FakeStreaming, ScriptedCleaner> {
        let loads = Rc::new(RefCell::new(0usize));
        DictationWorker::new(
            cfg,
            models_with_gemma(ModelState::Ready),
            Box::new(move |_dir: &Path| Ok(stt())),
            counted_cleaner_loader(&loads, log),
        )
    }

    fn texts(outcomes: Vec<Outcome>) -> Vec<String> {
        outcomes
            .into_iter()
            .map(|o| inject_text(o).0)
            .collect::<Vec<_>>()
    }

    /// REQ-STREAM-004/006: Satzweise fügt beim Endpoint ein, der Rest kommt beim
    /// Loslassen; ab dem zweiten Segment mit Leerzeichen davor.
    #[test]
    fn live_sentence_mode_injects_at_endpoint_and_rest_at_end() {
        let cfg = live_cfg(InsertMode::PerSentence, CleanupMode::Raw);
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut w = live_worker(
            &cfg,
            || {
                FakeStreaming::new(
                    &[("das ist", false), ("Erster Satz.", true), ("noch", false)],
                    "Zweiter Satz.",
                )
            },
            &log,
        );
        assert!(matches!(w.live_begin(None, &cfg), LiveStart::Started));
        assert!(w.live_chunk(&[0.0; 160]).is_empty());
        assert_eq!(texts(w.live_chunk(&[0.0; 160])), ["Erster Satz."]);
        assert!(w.live_chunk(&[0.0; 160]).is_empty());
        assert_eq!(texts(w.live_end()), [" Zweiter Satz."]);
    }

    /// REQ-STREAM-011: Geschäftlich/Natürlich korrigieren jedes Segment einzeln.
    #[test]
    fn live_sentence_mode_cleans_each_segment() {
        let cfg = live_cfg(InsertMode::PerSentence, CleanupMode::Business);
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut w = live_worker(&cfg, || FakeStreaming::new(&[("eins", true)], "zwei"), &log);
        w.live_begin(None, &cfg);
        assert_eq!(texts(w.live_chunk(&[0.0; 160])), ["[eins]"]);
        assert_eq!(texts(w.live_end()), [" [zwei]"]);
    }

    /// REQ-STREAM-010: LLM-optimiert ist bei Satzweise nicht erlaubt → Natürlich;
    /// auch eine Kontext-Regel wird so aufgelöst.
    #[test]
    fn live_sentence_mode_resolves_disallowed_modes() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut cfg = live_cfg(InsertMode::PerSentence, CleanupMode::LlmOptimized);
        cfg.context_aware_enabled = true;
        cfg.context_rules = vec![("com.app".into(), CleanupMode::LlmOptimized)];
        let mut w = live_worker(&cfg, || FakeStreaming::new(&[("eins", true)], ""), &log);
        w.live_begin(Some("com.app"), &cfg);
        w.live_chunk(&[0.0; 160]);
        assert!(
            log.borrow().iter().any(|e| e.starts_with("mode:Natürlich")),
            "erwartet set_mode(Natürlich): {:?}",
            log.borrow()
        );
        assert!(log.borrow().iter().all(|e| !e.contains("LLM-optimiert")));
    }

    /// Cleanup-Fehler brechen nie ab: Rohtext des Segments + Fallback-Flag.
    #[test]
    fn live_sentence_mode_falls_back_to_raw_when_cleaner_is_unavailable() {
        let cfg = live_cfg(InsertMode::PerSentence, CleanupMode::Business);
        let mut w: DictationWorker<FakeStreaming, ScriptedCleaner> = DictationWorker::new(
            &cfg,
            models_with_gemma(ModelState::Ready),
            Box::new(|_dir: &Path| Ok(FakeStreaming::new(&[("eins", true)], ""))),
            Box::new(|| (None, true)),
        );
        w.live_begin(None, &cfg);
        let out = w.live_chunk(&[0.0; 160]);
        assert_eq!(out.len(), 1);
        assert_eq!(
            inject_text(out.into_iter().next().unwrap()),
            ("eins".into(), true)
        );
    }

    /// REQ-STREAM-005/009: Wortgruppenweise gibt nur stabile Wörter aus, nur Roh.
    #[test]
    fn live_phrase_mode_injects_stable_words_without_cleanup() {
        let cfg = live_cfg(InsertMode::PerPhrase, CleanupMode::Business);
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut w = live_worker(
            &cfg,
            || {
                FakeStreaming::new(
                    &[
                        ("das ist", false),
                        ("das ist ein", false),
                        ("das ist ein test", false),
                    ],
                    "das ist ein test",
                )
            },
            &log,
        );
        w.live_begin(None, &cfg);
        assert!(w.live_chunk(&[0.0; 160]).is_empty());
        assert_eq!(texts(w.live_chunk(&[0.0; 160])), ["das ist"]);
        assert_eq!(texts(w.live_chunk(&[0.0; 160])), [" ein"]);
        assert_eq!(texts(w.live_end()), [" test"]);
        assert!(log.borrow().iter().all(|e| !e.starts_with("clean:")));
    }

    /// Grenzfälle: leere Segmente und leerer Rest erzeugen keinen Einfüge-Auftrag.
    #[test]
    fn live_empty_segments_inject_nothing() {
        let cfg = live_cfg(InsertMode::PerSentence, CleanupMode::Raw);
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut w = live_worker(&cfg, || FakeStreaming::new(&[("  ", true)], "  "), &log);
        w.live_begin(None, &cfg);
        assert!(w.live_chunk(&[0.0; 160]).is_empty());
        assert!(w.live_end().is_empty());
        // Nach dem Ende ist die Sitzung zu: weitere Chunks/Ends sind harmlos.
        assert!(w.live_chunk(&[0.0; 160]).is_empty());
        assert!(w.live_end().is_empty());
    }

    /// REQ-STREAM-013: Am Ende oder fehlendes Modell → kein Live (Fallback auf Batch).
    #[test]
    fn live_is_unavailable_for_at_end_and_without_model() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let at_end = live_cfg(InsertMode::AtEnd, CleanupMode::Raw);
        let mut w = live_worker(&at_end, || FakeStreaming::new(&[], ""), &log);
        assert!(matches!(
            w.live_begin(None, &at_end),
            LiveStart::Unavailable
        ));

        let cfg = live_cfg(InsertMode::PerSentence, CleanupMode::Raw);
        let mut w: DictationWorker<FakeStreaming, ScriptedCleaner> = DictationWorker::new(
            &cfg,
            models_with_gemma(ModelState::Ready),
            Box::new(|_dir: &Path| Err(TalkerError::Stt("kein Modell".into()))),
            Box::new(|| (None, false)),
        );
        assert!(matches!(w.live_begin(None, &cfg), LiveStart::Unavailable));
    }

    /// Fehlerpfad: ein STT-Fehler im Stream wird sichtbar gemeldet, nicht verschluckt.
    #[test]
    fn live_feed_error_is_reported() {
        let cfg = live_cfg(InsertMode::PerSentence, CleanupMode::Raw);
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut w = live_worker(
            &cfg,
            || {
                let mut f = FakeStreaming::new(&[], "");
                f.fail_feed = true;
                f
            },
            &log,
        );
        w.live_begin(None, &cfg);
        let out = w.live_chunk(&[0.0; 160]);
        assert!(matches!(out.as_slice(), [Outcome::Rejected(_)]));
    }

    // ---- run_live: Block-Strom → Chunks → Worker (ADR-0009) ----

    fn collect_live(
        w: &mut DictationWorker<FakeStreaming, ScriptedCleaner>,
        blocks: Vec<Vec<f32>>,
        format: (u16, u32),
        cfg: &Config,
    ) -> Vec<Outcome> {
        let mut out = Vec::new();
        run_live(w, blocks, format, None, cfg, &mut |o| out.push(o));
        out
    }

    /// Chunks gehen der Reihe nach an den Worker; der Rest unter 560 ms wird
    /// ebenfalls eingespeist (hier kommt der Endpoint erst mit dem Rest-Chunk).
    #[test]
    fn run_live_feeds_full_chunks_and_the_remainder_then_ends() {
        let cfg = live_cfg(InsertMode::PerSentence, CleanupMode::Raw);
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut w = live_worker(
            &cfg,
            || FakeStreaming::new(&[("", false), ("", false), ("Drittes.", true)], "Ende."),
            &log,
        );
        let n = crate::audio::CHUNK_16K_SAMPLES;
        let blocks = vec![vec![0.0; n + 10], vec![0.0; n + 90]];
        assert_eq!(
            texts(collect_live(&mut w, blocks, (1, 16_000), &cfg)),
            ["Drittes.", " Ende."]
        );
    }

    /// Fallback (REQ-STREAM-013): ohne Live-Engine wird die Aufnahme als ganze
    /// Utterance verarbeitet — nichts geht verloren.
    #[test]
    fn run_live_falls_back_to_batch_when_live_is_unavailable() {
        let cfg = live_cfg(InsertMode::AtEnd, CleanupMode::Raw);
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut w = live_worker(&cfg, || FakeStreaming::new(&[], "Batch-Text."), &log);
        let blocks = vec![vec![0.1; 2_000], vec![0.1; 2_000]];
        assert_eq!(
            texts(collect_live(&mut w, blocks, (1, 16_000), &cfg)),
            ["Batch-Text."]
        );
    }

    /// Grenzfälle: kein Audio → kein Einfügen; unbrauchbares Format → sichtbarer Fehler.
    #[test]
    fn run_live_handles_no_audio_and_bad_format() {
        let cfg = live_cfg(InsertMode::PerSentence, CleanupMode::Raw);
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut w = live_worker(&cfg, || FakeStreaming::new(&[], ""), &log);
        assert!(collect_live(&mut w, vec![], (1, 16_000), &cfg).is_empty());
        let out = collect_live(&mut w, vec![vec![0.0; 100]], (0, 16_000), &cfg);
        assert!(matches!(out.as_slice(), [Outcome::Rejected(_)]));
    }

    /// AK2a: gemma wird zwischen zwei Utterances ready → Fehler-Cache-Reset,
    /// Cleanup aktiviert sich ohne Neustart (Live-Aktivierung).
    #[test]
    fn worker_gemma_becoming_ready_reactivates_cleanup_without_restart() {
        let cfg = Config::default();
        assert!(
            cfg.cleanup_mode.uses_llm(),
            "Vorbedingung: Default will LLM"
        );
        let models = models_with_gemma(ModelState::Missing);
        let loads = Rc::new(RefCell::new(0usize));
        let log = Rc::new(RefCell::new(Vec::new()));
        let loader = {
            let loads = Rc::clone(&loads);
            let log = Rc::clone(&log);
            Box::new(move || {
                *loads.borrow_mut() += 1;
                // Erster Versuch: Modell-Datei fehlt noch (Download läuft).
                if *loads.borrow() == 1 {
                    (None, true)
                } else {
                    (
                        Some(ScriptedCleaner {
                            log: Rc::clone(&log),
                        }),
                        false,
                    )
                }
            })
        };
        let mut w = DictationWorker::new(&cfg, Arc::clone(&models), fake_stt_loader(), loader);
        assert_eq!(*loads.borrow(), 0, "gemma fehlt → kein Vorladen");

        // Utterance 1: Ladeversuch scheitert → Rohtext mit Fallback-Flag.
        assert_eq!(
            inject_text(w.handle(&[0.0; 1600], None, &cfg)),
            ("hallo".to_string(), true)
        );
        // Utterance 2: Fehler-Cache → KEIN erneuter Ladeversuch.
        w.handle(&[0.0; 1600], None, &cfg);
        assert_eq!(*loads.borrow(), 1, "Fehler-Cache muss Ladeversuche stoppen");

        // gemma wird ready → nächste Utterance lädt und cleant, ohne Neustart.
        models.set(ModelId::Gemma, ModelState::Ready);
        assert_eq!(
            inject_text(w.handle(&[0.0; 1600], None, &cfg)),
            ("[hallo]".to_string(), false)
        );
        assert_eq!(*loads.borrow(), 2);
    }

    /// AK2b (Teil 1): Modus-Flip LLM→Roh→LLM entlädt den Cleaner im Roh-Modus
    /// und lädt ihn beim Zurückwechseln neu (Retry nach Reset).
    #[test]
    fn worker_mode_flip_llm_raw_llm_unloads_and_reloads_cleaner() {
        let business = Config::default();
        let raw = Config {
            cleanup_mode: CleanupMode::Raw,
            ..Config::default()
        };
        let models = models_with_gemma(ModelState::Ready);
        let loads = Rc::new(RefCell::new(0usize));
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut w = DictationWorker::new(
            &business,
            models,
            fake_stt_loader(),
            counted_cleaner_loader(&loads, &log),
        );
        assert_eq!(*loads.borrow(), 1, "LLM-Config + gemma ready → Vorladen");

        assert_eq!(
            inject_text(w.handle(&[0.0; 1600], None, &business)),
            ("[hallo]".to_string(), false)
        );
        // Roh-Modus: Cleaner wird nicht angewandt UND entladen (Config-Bedarf weg).
        assert_eq!(
            inject_text(w.handle(&[0.0; 1600], None, &raw)),
            ("hallo".to_string(), false)
        );
        // Zurück auf LLM: lazy Reload.
        assert_eq!(
            inject_text(w.handle(&[0.0; 1600], None, &business)),
            ("[hallo]".to_string(), false)
        );
        assert_eq!(
            *loads.borrow(),
            2,
            "Roh-Flip muss entladen, LLM-Flip neu laden"
        );
    }

    /// AK2b (Teil 2): Reset folgt der CONFIG, nicht der Utterance — eine
    /// Raw-geregelte App darf den (für andere Apps) geladenen Cleaner weder
    /// anwenden noch entladen.
    #[test]
    fn worker_raw_context_rule_skips_cleaner_but_keeps_it_loaded() {
        let cfg = Config {
            cleanup_mode: CleanupMode::Business,
            context_aware_enabled: true,
            context_rules: vec![("com.raw.app".to_string(), CleanupMode::Raw)],
            ..Config::default()
        };
        let models = models_with_gemma(ModelState::Ready);
        let loads = Rc::new(RefCell::new(0usize));
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut w = DictationWorker::new(
            &cfg,
            models,
            fake_stt_loader(),
            counted_cleaner_loader(&loads, &log),
        );

        // Raw-geregelte App: Rohtext, kein Fallback-Flag, kein clean-Aufruf.
        assert_eq!(
            inject_text(w.handle(&[0.0; 1600], Some("com.raw.app"), &cfg)),
            ("hallo".to_string(), false)
        );
        assert!(
            log.borrow().iter().all(|e| !e.starts_with("clean:")),
            "aufgelöstes Raw darf den Cleaner nicht anwenden: {:?}",
            log.borrow()
        );
        // Andere App: Cleanup läuft — OHNE Kaltstart (kein zweiter Load).
        assert_eq!(
            inject_text(w.handle(&[0.0; 1600], Some("com.other.app"), &cfg)),
            ("[hallo]".to_string(), false)
        );
        assert_eq!(
            *loads.borrow(),
            1,
            "Raw-Regel darf das Modell nicht entladen"
        );
        // Der aufgelöste Modus wurde vor dem clean gesetzt (set_mode).
        assert_eq!(
            log.borrow().as_slice(),
            [
                format!("mode:{}", CleanupMode::Business.label()),
                "clean:hallo".to_string()
            ]
        );
    }

    /// AK2c: stt_model_dir-Wechsel lädt den Transcriber neu; scheitert das
    /// Neuladen, bleibt der alte aktiv (kein Diktat-Ausfall).
    #[test]
    fn worker_stt_dir_change_reloads_and_keeps_old_on_failure() {
        let cfg = Config::default();
        let dirs = Rc::new(RefCell::new(Vec::<PathBuf>::new()));
        let loader: TranscriberLoader<FakeTranscriber> = {
            let dirs = Rc::clone(&dirs);
            Box::new(move |dir: &Path| {
                dirs.borrow_mut().push(dir.to_path_buf());
                if dir.ends_with("kaputt") {
                    Err(TalkerError::Stt("Modellordner kaputt".into()))
                } else {
                    Ok(FakeTranscriber { reply: "hallo" })
                }
            })
        };
        let models = models_with_gemma(ModelState::Missing);
        let raw_cfg = Config {
            cleanup_mode: CleanupMode::Raw,
            ..cfg.clone()
        };
        let mut w = DictationWorker::new(
            &raw_cfg,
            models,
            loader,
            Box::new(|| (None::<ScriptedCleaner>, false)),
        );
        assert_eq!(dirs.borrow().len(), 1, "Konstruktor lädt sofort");

        // Gleicher Pfad → kein Reload.
        w.handle(&[0.0; 1600], None, &raw_cfg);
        assert_eq!(dirs.borrow().len(), 1);

        // Neuer Pfad → Reload.
        let moved = Config {
            stt_model_dir: PathBuf::from("/modelle/neu"),
            ..raw_cfg.clone()
        };
        assert_eq!(
            inject_text(w.handle(&[0.0; 1600], None, &moved)),
            ("hallo".to_string(), false)
        );
        assert_eq!(
            dirs.borrow().last().unwrap(),
            &PathBuf::from("/modelle/neu")
        );

        // Kaputter Pfad → Ladefehler, aber der alte Transcriber liefert weiter.
        let broken = Config {
            stt_model_dir: PathBuf::from("/modelle/kaputt"),
            ..raw_cfg.clone()
        };
        assert_eq!(
            inject_text(w.handle(&[0.0; 1600], None, &broken)),
            ("hallo".to_string(), false)
        );
    }

    /// AK2d: Cleanup-Fehler → Fallback auf den Raw Transcript mit Flag —
    /// jetzt durch den ganzen Worker statt nur durch process_utterance.
    #[test]
    fn worker_cleanup_error_falls_back_to_raw_transcript() {
        struct FailingCleaner;
        impl LlmCleaner for FailingCleaner {
            fn clean(&mut self, _raw: &str) -> crate::error::Result<String> {
                Err(TalkerError::Cleanup("Timeout".into()))
            }
        }
        let cfg = Config::default();
        let models = models_with_gemma(ModelState::Ready);
        let mut w = DictationWorker::new(
            &cfg,
            models,
            fake_stt_loader(),
            Box::new(|| (Some(FailingCleaner), false)),
        );

        assert_eq!(
            inject_text(w.handle(&[0.0; 1600], None, &cfg)),
            ("hallo".to_string(), true)
        );
    }

    /// AK2e: jeder Verwerfungs-Pfad wird ein sichtbares Outcome mit Hinweis —
    /// die Pipeline bricht an keiner Stufe hart ab.
    #[test]
    fn worker_rejection_outcomes_carry_indicator_hints() {
        let raw_cfg = Config {
            cleanup_mode: CleanupMode::Raw,
            ..Config::default()
        };
        let no_cleaner = || Box::new(|| (None::<ScriptedCleaner>, false)) as CleanerLoader<_>;

        // Kein STT-Modell ladbar → verworfen, mit erneutem Versuch pro Utterance.
        let attempts = Rc::new(RefCell::new(0usize));
        let failing_stt: TranscriberLoader<FakeTranscriber> = {
            let attempts = Rc::clone(&attempts);
            Box::new(move |_dir: &Path| {
                *attempts.borrow_mut() += 1;
                Err(TalkerError::Stt("fehlt".into()))
            })
        };
        let mut w = DictationWorker::new(
            &raw_cfg,
            models_with_gemma(ModelState::Missing),
            failing_stt,
            no_cleaner(),
        );
        assert!(matches!(
            w.handle(&[0.0; 1600], None, &raw_cfg),
            Outcome::Rejected("Kein STT-Modell")
        ));
        assert_eq!(*attempts.borrow(), 2, "Konstruktor + Retry pro Utterance");

        // Leerer Transcript → „Nichts erkannt".
        let mut w = DictationWorker::new(
            &raw_cfg,
            models_with_gemma(ModelState::Missing),
            fake_stt_loader(),
            no_cleaner(),
        );
        assert!(matches!(
            w.handle(&[], None, &raw_cfg),
            Outcome::Rejected("Nichts erkannt")
        ));

        // STT-Fehler → „Spracherkennung fehlgeschlagen".
        struct FailingTranscriber;
        impl Transcriber for FailingTranscriber {
            fn transcribe(&mut self, _pcm: &[f32]) -> crate::error::Result<String> {
                Err(TalkerError::Stt("Modell kaputt".into()))
            }
        }
        let mut w = DictationWorker::new(
            &raw_cfg,
            models_with_gemma(ModelState::Missing),
            Box::new(|_dir: &Path| Ok(FailingTranscriber)),
            no_cleaner(),
        );
        assert!(matches!(
            w.handle(&[0.0; 1600], None, &raw_cfg),
            Outcome::Rejected("Spracherkennung fehlgeschlagen")
        ));
    }

    /// AK3 (E2E-Join): PCM → Worker → Injection — Cleaned Transcript landet
    /// per Fake-Pasteboard + Fake-Cmd+V in der Target App, Nutzer-Clipboard
    /// wird restauriert. Der vor Ticket-0034 unschreibbare Test.
    #[test]
    fn e2e_pcm_to_injected_text_through_worker_and_fakes() {
        use crate::clipboard::{ItemData, Pasteboard};
        use crate::injection::{self, KeySender};

        struct MemPasteboard {
            items: RefCell<Vec<ItemData>>,
            writes: RefCell<Vec<String>>,
        }
        impl Pasteboard for MemPasteboard {
            fn read_items(&self) -> crate::error::Result<Vec<ItemData>> {
                Ok(self.items.borrow().clone())
            }
            fn write_items(&self, items: &[ItemData]) -> crate::error::Result<()> {
                *self.items.borrow_mut() = items.to_vec();
                Ok(())
            }
            fn write_text(&self, text: &str) -> crate::error::Result<()> {
                self.writes.borrow_mut().push(text.to_string());
                *self.items.borrow_mut() = vec![vec![(
                    "public.utf8-plain-text".to_string(),
                    text.as_bytes().to_vec(),
                )]];
                Ok(())
            }
        }
        struct CountingKeys(RefCell<usize>);
        impl KeySender for CountingKeys {
            fn send_cmd_v(&self) -> crate::error::Result<()> {
                *self.0.borrow_mut() += 1;
                Ok(())
            }
        }

        let cfg = Config::default();
        let loads = Rc::new(RefCell::new(0usize));
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut w = DictationWorker::new(
            &cfg,
            models_with_gemma(ModelState::Ready),
            fake_stt_loader(),
            counted_cleaner_loader(&loads, &log),
        );

        let (text, fell_back) = inject_text(w.handle(&[0.0; 1600], None, &cfg));
        assert!(!fell_back);

        let user_content: Vec<ItemData> = vec![vec![(
            "public.utf8-plain-text".to_string(),
            b"Nutzer-Inhalt".to_vec(),
        )]];
        let pb = MemPasteboard {
            items: RefCell::new(user_content.clone()),
            writes: RefCell::new(Vec::new()),
        };
        let keys = CountingKeys(RefCell::new(0));
        injection::inject(&pb, &keys, &text).unwrap();

        assert_eq!(
            pb.writes.borrow().as_slice(),
            ["[hallo]"],
            "der Cleaned Transcript wurde eingefügt"
        );
        assert_eq!(*keys.0.borrow(), 1, "genau ein Cmd+V in die Target App");
        assert_eq!(
            *pb.items.borrow(),
            user_content,
            "Nutzer-Clipboard nach der Injection restauriert"
        );
    }

    // ---- process_job: Messpunkte, Ergebnis, Verhalten unverändert (T13) ----

    use crate::diagnostics::DictationTrace;

    fn job_info() -> audio::SourceInfo {
        audio::SourceInfo {
            format: audio::AudioFormat {
                channels: 1,
                sample_rate: 16_000,
            },
            device: "Test-Mikro".into(),
        }
    }

    fn base_trace(cfg: &Config) -> Box<DictationTrace> {
        Box::new(DictationTrace::new(
            1,
            audio::HostInstant::from_nanos(1_000),
            cfg.insert_mode,
            cfg.cleanup_mode,
        ))
    }

    fn utterance_job(cfg: &Config) -> SttJob {
        SttJob::Utterance {
            pcm: vec![0.1; 16_000],
            frontmost: None,
            captured: audio::Captured {
                raw: vec![0.1; 16_000],
                info: job_info(),
                marks: audio::StartMarks::default(),
            },
            trace: base_trace(cfg),
        }
    }

    /// Live-Auftrag: `blocks` volle Chunks, dann `End` mit Loslass-Zeitpunkt 9 000 ns.
    fn live_job(cfg: &Config, blocks: usize) -> SttJob {
        let (tx, rx) = std::sync::mpsc::channel();
        for i in 0..blocks {
            tx.send(audio::Feed::Block(audio::Block {
                samples: vec![0.0; crate::audio::CHUNK_16K_SAMPLES],
                at: audio::HostInstant::from_nanos(2_000 + i as u64),
            }))
            .unwrap();
        }
        tx.send(audio::Feed::End(Box::new(audio::Ended {
            released_at: audio::HostInstant::from_nanos(9_000),
            captured: audio::Captured {
                raw: vec![0.0; blocks * crate::audio::CHUNK_16K_SAMPLES],
                info: job_info(),
                marks: audio::StartMarks::default(),
            },
        })))
        .unwrap();
        SttJob::Live {
            feed: LiveFeed {
                trace: base_trace(cfg),
                frontmost: None,
                blocks: rx,
                format: (1, 16_000),
            },
        }
    }

    type Injected = Rc<RefCell<Vec<String>>>;

    fn recording_inject(
        injected: &Injected,
    ) -> impl FnMut(&str) -> crate::error::Result<()> + use<> {
        let injected = Rc::clone(injected);
        move |text: &str| {
            injected.borrow_mut().push(text.to_string());
            Ok(())
        }
    }

    fn streaming_worker(
        cfg: &Config,
        rest: &'static str,
        updates: &'static [(&'static str, bool)],
    ) -> DictationWorker<FakeStreaming, ScriptedCleaner> {
        let log = Rc::new(RefCell::new(Vec::new()));
        live_worker(cfg, move || FakeStreaming::new(updates, rest), &log)
    }

    /// REQ-DIAG-012: Utterance-Pfad füllt Text-/STT-/Cleanup-/Einfüge-Messpunkte.
    #[test]
    fn utterance_job_fills_the_measurement_points() {
        let cfg = Config::default();
        let mut w = streaming_worker(&cfg, "hallo", &[]);
        let injected: Injected = Rc::default();
        let (hint, trace) = process_job(
            &mut w,
            utterance_job(&cfg),
            &cfg,
            &mut recording_inject(&injected),
            None,
        );
        assert_eq!(hint, None);
        assert_eq!(injected.borrow().as_slice(), ["[hallo]"]);
        assert_eq!(trace.result, DictationResult::Inserted);
        assert!(trace.text_at.is_some());
        assert!(trace.stt.is_some() && trace.cleanup.is_some() && trace.injected.is_some());
        assert_eq!(trace.nr, 1);
        assert!(trace.model_loads.is_empty());
    }

    /// Modus Roh → kein Cleanup-Zeitwert (`—`).
    #[test]
    fn raw_mode_job_has_no_cleanup_time() {
        let cfg = live_cfg(InsertMode::AtEnd, CleanupMode::Raw);
        let mut w = streaming_worker(&cfg, "hallo", &[]);
        let injected: Injected = Rc::default();
        let (_, trace) = process_job(
            &mut w,
            utterance_job(&cfg),
            &cfg,
            &mut recording_inject(&injected),
            None,
        );
        assert_eq!(injected.borrow().as_slice(), ["hallo"]);
        assert!(trace.stt.is_some());
        assert_eq!(trace.cleanup, None);
        assert_eq!(trace.result, DictationResult::Inserted);
    }

    /// AK-13: leerer STT-Text → verworfen (nichts erkannt), nichts eingefügt.
    #[test]
    fn empty_transcript_job_is_discarded_as_nothing_recognized() {
        let cfg = live_cfg(InsertMode::AtEnd, CleanupMode::Raw);
        let mut w = streaming_worker(&cfg, "", &[]);
        let injected: Injected = Rc::default();
        let (hint, trace) = process_job(
            &mut w,
            utterance_job(&cfg),
            &cfg,
            &mut recording_inject(&injected),
            None,
        );
        assert_eq!(hint, Some("Nichts erkannt"));
        assert!(injected.borrow().is_empty());
        assert_eq!(trace.result, DictationResult::Discarded("nichts erkannt"));
        assert_eq!(trace.text_at, None);
        assert_eq!(trace.injected, None);
    }

    /// AK-13: STT-Fehler → Ergebnis Fehler, keine Zeitwerte, nichts eingefügt.
    #[test]
    fn stt_error_job_is_reported_as_error() {
        struct FailingStt;
        impl Transcriber for FailingStt {
            fn transcribe(&mut self, _: &[f32]) -> crate::error::Result<String> {
                Err(TalkerError::Stt("kaputt".into()))
            }
        }
        impl StreamingTranscriber for FailingStt {
            fn begin(&mut self) {}
            fn feed(&mut self, _: &[f32]) -> crate::error::Result<crate::stt::StreamUpdate> {
                Err(TalkerError::Stt("kaputt".into()))
            }
            fn finish(&mut self) -> crate::error::Result<String> {
                Err(TalkerError::Stt("kaputt".into()))
            }
        }
        let cfg = live_cfg(InsertMode::AtEnd, CleanupMode::Raw);
        let mut w = DictationWorker::new(
            &cfg,
            models_with_gemma(ModelState::Ready),
            Box::new(|_: &Path| Ok(FailingStt)),
            Box::new(|| (None::<ScriptedCleaner>, false)),
        );
        let injected: Injected = Rc::default();
        let (hint, trace) = process_job(
            &mut w,
            utterance_job(&cfg),
            &cfg,
            &mut recording_inject(&injected),
            None,
        );
        assert_eq!(hint, Some("Spracherkennung fehlgeschlagen"));
        assert!(injected.borrow().is_empty());
        assert!(matches!(trace.result, DictationResult::Error(_)));
        assert_eq!(
            (trace.stt, trace.cleanup, trace.text_at),
            (None, None, None)
        );
    }

    /// Verhalten unverändert: Cleanup-Fehler → Rohtext eingefügt, Ergebnis `eingefügt`, Hinweis bleibt.
    #[test]
    fn cleanup_error_job_still_injects_the_raw_text() {
        struct FailingCleaner;
        impl LlmCleaner for FailingCleaner {
            fn clean(&mut self, _: &str) -> crate::error::Result<String> {
                Err(TalkerError::Cleanup("Timeout".into()))
            }
        }
        let cfg = Config::default();
        let mut w = DictationWorker::new(
            &cfg,
            models_with_gemma(ModelState::Ready),
            Box::new(|_: &Path| Ok(FakeStreaming::new(&[], "hallo"))),
            Box::new(|| (Some(FailingCleaner), false)),
        );
        let injected: Injected = Rc::default();
        let (hint, trace) = process_job(
            &mut w,
            utterance_job(&cfg),
            &cfg,
            &mut recording_inject(&injected),
            None,
        );
        assert_eq!(hint, Some("Cleanup übersprungen — Rohtext eingefügt"));
        assert_eq!(injected.borrow().as_slice(), ["hallo"]);
        assert_eq!(trace.result, DictationResult::Inserted);
    }

    /// Einfügen schlägt fehl → Hinweis wie bisher, Ergebnis Fehler.
    #[test]
    fn injection_error_is_reported_with_the_old_hint() {
        let cfg = live_cfg(InsertMode::AtEnd, CleanupMode::Raw);
        let mut w = streaming_worker(&cfg, "hallo", &[]);
        let mut failing = |_: &str| Err(TalkerError::Keystroke("kein Zugriff".into()));
        let (hint, trace) = process_job(&mut w, utterance_job(&cfg), &cfg, &mut failing, None);
        assert_eq!(hint, Some("Einfügen fehlgeschlagen"));
        assert!(matches!(&trace.result, DictationResult::Error(e) if e.contains("kein Zugriff")));
    }

    /// AK-11: Live mit 3 Segmenten → Segmentzahl, erstes Segment, Loslass-Daten aus `End`.
    #[test]
    fn live_job_counts_segments_and_takes_release_data_from_end() {
        let cfg = live_cfg(InsertMode::PerSentence, CleanupMode::Raw);
        let mut w = streaming_worker(
            &cfg,
            "Dritter.",
            &[
                ("a", false),
                ("Erster.", true),
                ("b", false),
                ("Zweiter.", true),
            ],
        );
        let injected: Injected = Rc::default();
        let (hint, trace) = process_job(
            &mut w,
            live_job(&cfg, 4),
            &cfg,
            &mut recording_inject(&injected),
            None,
        );
        assert_eq!(hint, None);
        assert_eq!(
            injected.borrow().as_slice(),
            ["Erster.", " Zweiter.", " Dritter."]
        );
        assert_eq!(trace.segments, Some(3));
        assert!(trace.first_segment_at.is_some());
        assert!(trace.first_segment_at <= trace.text_at);
        assert_eq!(
            trace.released_at,
            Some(audio::HostInstant::from_nanos(9_000))
        );
        assert_eq!(
            trace.recorded,
            Some(std::time::Duration::from_millis(2_240))
        );
        assert!(trace.device.is_some() && trace.stt.is_some());
        assert_eq!(trace.result, DictationResult::Inserted);
    }

    /// Live ohne Segment → 0 Segmente, kein erstes Segment, verworfen (nichts erkannt).
    #[test]
    fn live_job_without_segments_reports_zero() {
        let cfg = live_cfg(InsertMode::PerSentence, CleanupMode::Raw);
        let mut w = streaming_worker(&cfg, "", &[]);
        let injected: Injected = Rc::default();
        let (hint, trace) = process_job(
            &mut w,
            live_job(&cfg, 2),
            &cfg,
            &mut recording_inject(&injected),
            None,
        );
        assert_eq!(
            hint, None,
            "Live ohne Text meldet dem Indicator weiter nichts"
        );
        assert!(injected.borrow().is_empty());
        assert_eq!(trace.segments, Some(0));
        assert_eq!(trace.first_segment_at, None);
        assert_eq!(trace.result, DictationResult::Discarded("nichts erkannt"));
    }

    /// Live ohne `End` (Kanal einfach zu): Loslass-Punkte bleiben leer, kein Panic.
    #[test]
    fn live_job_without_end_keeps_release_points_empty() {
        let cfg = live_cfg(InsertMode::PerSentence, CleanupMode::Raw);
        let mut w = streaming_worker(&cfg, "Rest.", &[]);
        let (tx, rx) = std::sync::mpsc::channel();
        drop(tx);
        let job = SttJob::Live {
            feed: LiveFeed {
                trace: base_trace(&cfg),
                frontmost: None,
                blocks: rx,
                format: (1, 16_000),
            },
        };
        let injected: Injected = Rc::default();
        let (_, trace) = process_job(&mut w, job, &cfg, &mut recording_inject(&injected), None);
        assert_eq!(trace.released_at, None);
        assert_eq!(trace.recorded, None);
    }

    /// REQ-DIAG-016: Ladezeit nur bei Laden während der Dictation; `take` leert.
    #[test]
    fn model_loads_are_only_recorded_during_a_dictation_and_drain() {
        let raw_cfg = live_cfg(InsertMode::AtEnd, CleanupMode::Raw);
        let loads = Rc::new(RefCell::new(0usize));
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut w = DictationWorker::new(
            &raw_cfg,
            models_with_gemma(ModelState::Ready),
            fake_stt_loader(),
            counted_cleaner_loader(&loads, &log),
        );
        assert!(
            w.take_model_loads().is_empty(),
            "Laden in new() zählt nicht"
        );
        // STT-Pfad ändert sich → Neuladen während der Dictation.
        let moved = Config {
            stt_model_dir: PathBuf::from("/anderes/modell"),
            ..raw_cfg.clone()
        };
        w.handle(&[0.0; 1600], None, &moved);
        let first = w.take_model_loads();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].0, ModelId::Stt);
        assert!(w.take_model_loads().is_empty());
        // Wechsel auf einen LLM-Modus → Cleaner wird erst jetzt geladen.
        let business = Config {
            cleanup_mode: CleanupMode::Business,
            ..moved
        };
        w.handle(&[0.0; 1600], None, &business);
        let second = w.take_model_loads();
        assert_eq!(
            second.iter().map(|l| l.0).collect::<Vec<_>>(),
            [ModelId::Gemma]
        );
        // Nächste Dictation ohne Laden: leer.
        w.handle(&[0.0; 1600], None, &business);
        assert!(w.take_model_loads().is_empty());
    }

    /// Modell-Laden landet in der Trace des Auftrags.
    #[test]
    fn job_trace_lists_models_loaded_during_the_dictation() {
        let raw_cfg = live_cfg(InsertMode::AtEnd, CleanupMode::Raw);
        let mut w = streaming_worker(&raw_cfg, "hallo", &[]);
        let business = Config {
            cleanup_mode: CleanupMode::Business,
            ..raw_cfg
        };
        let injected: Injected = Rc::default();
        let (_, trace) = process_job(
            &mut w,
            utterance_job(&business),
            &business,
            &mut recording_inject(&injected),
            None,
        );
        assert_eq!(trace.model_loads.len(), 1);
        assert_eq!(trace.model_loads[0].0, ModelId::Gemma);
    }

    /// AK-14 „darf nicht passieren": kein Transcript-Text in irgendeiner Logzeile, auch nicht auf Debug.
    #[test]
    fn no_log_line_contains_the_transcript_even_on_debug() {
        let cfg = live_cfg(InsertMode::PerSentence, CleanupMode::Business);
        let mut live = streaming_worker(&cfg, "geheimrest", &[("x", false), ("geheimsatz", true)]);
        let batch_cfg = live_cfg(InsertMode::AtEnd, CleanupMode::Business);
        let mut batch = streaming_worker(&batch_cfg, "geheimbatch", &[]);
        let injected: Injected = Rc::default();
        let (_, lines) = crate::logging::capture(|| {
            process_job(
                &mut live,
                live_job(&cfg, 2),
                &cfg,
                &mut recording_inject(&injected),
                None,
            );
            process_job(
                &mut batch,
                utterance_job(&batch_cfg),
                &batch_cfg,
                &mut recording_inject(&injected),
                None,
            );
        });
        assert!(!lines.is_empty(), "auf Debug entstehen Detailzeilen");
        assert!(injected.borrow().iter().any(|t| t.contains("geheim")));
        assert!(
            lines.iter().all(|(_, l)| !l.contains("geheim")),
            "Transcript im Log: {lines:?}"
        );
    }

    /// `FeedBlocks`: Samples bis `End`, das `End` bleibt abholbar.
    #[test]
    fn feed_blocks_stop_at_end_and_keep_it() {
        let (tx, rx) = std::sync::mpsc::channel();
        let block = |v: f32| {
            audio::Feed::Block(audio::Block {
                samples: vec![v],
                at: audio::HostInstant::from_nanos(1),
            })
        };
        tx.send(block(1.0)).unwrap();
        tx.send(block(2.0)).unwrap();
        tx.send(audio::Feed::End(Box::new(audio::Ended {
            released_at: audio::HostInstant::from_nanos(5),
            captured: audio::Captured {
                raw: vec![1.0, 2.0],
                info: job_info(),
                marks: audio::StartMarks::default(),
            },
        })))
        .unwrap();
        tx.send(block(3.0)).unwrap();
        let mut blocks = FeedBlocks::new(rx);
        assert_eq!((&mut blocks).collect::<Vec<_>>(), [vec![1.0], vec![2.0]]);
        let ended = blocks.take_ended().unwrap();
        assert_eq!(ended.released_at, audio::HostInstant::from_nanos(5));
        assert!(blocks.take_ended().is_none());
    }

    /// REQ-DIAG-009/-012: auf `info` genau eine Zeile (die Messzeile) für eine Live-Dictation
    /// mit 3 Segmenten; auf `debug` kommen Chunk- und Segment-Zeilen dazu.
    #[test]
    fn live_dictation_with_three_segments_logs_exactly_one_line_on_info() {
        let cfg = live_cfg(InsertMode::PerSentence, CleanupMode::Raw);
        let mut w = streaming_worker(
            &cfg,
            "Dritter.",
            &[
                ("a", false),
                ("Erster.", true),
                ("b", false),
                ("Zweiter.", true),
            ],
        );
        let report = crate::diagnostics::log_report();
        let injected: Injected = Rc::default();
        let (_, lines) = crate::logging::capture(|| {
            let (_, trace) = process_job(
                &mut w,
                live_job(&cfg, 4),
                &cfg,
                &mut recording_inject(&injected),
                None,
            );
            report(trace);
        });
        assert_eq!(injected.borrow().len(), 3);
        let on_info: Vec<_> = lines
            .iter()
            .filter(|(l, _)| *l <= log::Level::Info)
            .collect();
        assert_eq!(on_info.len(), 1, "auf info: {on_info:?}");
        assert!(on_info[0].1.starts_with("talker::messung: Diktat #1: "));
        assert!(on_info[0].1.contains("Segmente 3"));
        let detail: Vec<_> = lines
            .iter()
            .filter(|(l, _)| *l == log::Level::Debug)
            .collect();
        assert!(detail.iter().any(|(_, l)| l.contains("Live-Chunk")));
        assert!(detail.iter().any(|(_, l)| l.contains("Live-Segment")));
    }

    // ---- Debug-Mitschnitt in process_job (T15) ----

    struct DebugDir(PathBuf);
    impl DebugDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("talker-job-debug-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn files(&self) -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_dir(&self.0)
                .map(|d| {
                    d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            v.sort();
            v
        }
    }
    impl Drop for DebugDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn debug_capture(dir: &Path) -> DebugCapture {
        // Nie über die Umgebung: parallele Tests würden sich stören.
        DebugCapture::from_var(dir.to_str()).unwrap()
    }

    /// REQ-DIAG-019/-022: Mitschnitt wird NACH dem Einfügen geschrieben; mic.wav = Rohaufnahme,
    /// model.wav = Tap-Inhalt; Dateien tragen die Diktat-Nummer.
    #[test]
    fn debug_wavs_are_written_after_injection_with_the_dictation_number() {
        let dir = DebugDir::new("order");
        let capture = debug_capture(&dir.0);
        let tap = ModelTap::default();
        let cfg = live_cfg(InsertMode::AtEnd, CleanupMode::Raw);
        let mut w = streaming_worker(&cfg, "hallo", &[]);
        let watched = dir.0.clone();
        let tap_in_worker = tap.clone();
        let seen_before_inject = Rc::new(RefCell::new(None));
        let seen = Rc::clone(&seen_before_inject);
        let mut inject = move |_: &str| {
            *seen.borrow_mut() = Some(std::fs::read_dir(&watched).unwrap().count());
            // Der Transcriber füllt den Tap während der Verarbeitung.
            tap_in_worker.push(&[0.5; 100]);
            Ok(())
        };
        let (_, trace) = process_job(
            &mut w,
            utterance_job(&cfg),
            &cfg,
            &mut inject,
            Some((&capture, &tap)),
        );
        assert_eq!(
            *seen_before_inject.borrow(),
            Some(0),
            "inject vor dem Schreiben"
        );
        assert_eq!(dir.files(), ["1-mic.wav", "1-model.wav"]);
        assert_eq!(trace.nr, 1);
        let mic = hound::WavReader::open(dir.0.join("1-mic.wav")).unwrap();
        assert_eq!(mic.len(), 16_000);
        let model = hound::WavReader::open(dir.0.join("1-model.wav")).unwrap();
        assert_eq!(model.len(), 100);
        assert!(tap.take().is_empty(), "der Tap wurde abgeholt");
    }

    /// Live: mic.wav stammt aus `End.captured`.
    #[test]
    fn live_job_writes_the_debug_wavs_from_the_end_capture() {
        let dir = DebugDir::new("live");
        let capture = debug_capture(&dir.0);
        let tap = ModelTap::default();
        let cfg = live_cfg(InsertMode::PerSentence, CleanupMode::Raw);
        let mut w = streaming_worker(&cfg, "Rest.", &[]);
        let injected: Injected = Rc::default();
        process_job(
            &mut w,
            live_job(&cfg, 2),
            &cfg,
            &mut recording_inject(&injected),
            Some((&capture, &tap)),
        );
        assert_eq!(dir.files(), ["1-mic.wav", "1-model.wav"]);
        let mic = hound::WavReader::open(dir.0.join("1-mic.wav")).unwrap();
        assert_eq!(mic.len() as usize, 2 * crate::audio::CHUNK_16K_SAMPLES);
    }

    /// REQ-DIAG-021: nicht beschreibbares Verzeichnis → Warnung, Text wurde normal eingefügt.
    #[test]
    fn unwritable_debug_dir_does_not_affect_the_inserted_text() {
        let dir = DebugDir::new("blocked");
        let blocker = dir.0.join("datei");
        std::fs::write(&blocker, "x").unwrap();
        let capture = debug_capture(&blocker.join("unter"));
        let tap = ModelTap::default();
        let cfg = live_cfg(InsertMode::AtEnd, CleanupMode::Raw);
        let mut w = streaming_worker(&cfg, "hallo", &[]);
        let injected: Injected = Rc::default();
        let ((hint, trace), lines) = crate::logging::capture(|| {
            process_job(
                &mut w,
                utterance_job(&cfg),
                &cfg,
                &mut recording_inject(&injected),
                Some((&capture, &tap)),
            )
        });
        assert_eq!(hint, None);
        assert_eq!(injected.borrow().as_slice(), ["hallo"]);
        assert_eq!(trace.result, DictationResult::Inserted);
        assert!(lines.iter().any(|(l, _)| *l == log::Level::Warn));
    }

    /// REQ-DIAG-022/REQ-DICT-003: ohne Debug (None) wird nichts auf die Platte geschrieben.
    #[test]
    fn without_debug_nothing_is_written() {
        let dir = DebugDir::new("none");
        let cfg = live_cfg(InsertMode::AtEnd, CleanupMode::Raw);
        let mut w = streaming_worker(&cfg, "hallo", &[]);
        let injected: Injected = Rc::default();
        process_job(
            &mut w,
            utterance_job(&cfg),
            &cfg,
            &mut recording_inject(&injected),
            None,
        );
        assert!(dir.files().is_empty());
    }

    /// AK-16: im Audio-Modul gibt es keine Datei-I/O (kein Schreiben im Callback-Pfad).
    #[test]
    fn audio_module_has_no_file_writing() {
        let source = include_str!("audio.rs");
        let production = source.split("#[cfg(test)]\nmod tests").next().unwrap();
        for forbidden in ["WavWriter", "std::fs", "File::create", "OpenOptions"] {
            assert!(!production.contains(forbidden), "{forbidden} in audio.rs");
        }
    }

    // ---- Ende zu Ende ohne Mikrofon und ohne Modell (T16, AK-21) ----

    use crate::audio::AudioSource as _;
    use std::sync::mpsc::{Receiver, channel};
    use std::time::Duration as StdDuration;

    /// STT-Fake, das den Modell-Eingang der Batch-Utterance festhält.
    struct RecordingStt {
        received: Rc<RefCell<Vec<f32>>>,
        reply: &'static str,
        fail: bool,
        updates: std::collections::VecDeque<crate::stt::StreamUpdate>,
        /// Künstliche Dauer je `feed` (Live-Messung).
        feed_delay: StdDuration,
    }

    impl RecordingStt {
        fn new(received: &Rc<RefCell<Vec<f32>>>, reply: &'static str) -> Self {
            Self {
                received: Rc::clone(received),
                reply,
                fail: false,
                updates: Default::default(),
                feed_delay: StdDuration::ZERO,
            }
        }
    }

    impl Transcriber for RecordingStt {
        fn transcribe(&mut self, pcm: &[f32]) -> crate::error::Result<String> {
            if self.fail {
                return Err(TalkerError::Stt("kaputt".into()));
            }
            *self.received.borrow_mut() = pcm.to_vec();
            Ok(self.reply.to_string())
        }
    }

    impl StreamingTranscriber for RecordingStt {
        fn begin(&mut self) {}
        fn feed(&mut self, _: &[f32]) -> crate::error::Result<crate::stt::StreamUpdate> {
            std::thread::sleep(self.feed_delay);
            Ok(self
                .updates
                .pop_front()
                .unwrap_or(crate::stt::StreamUpdate {
                    text: String::new(),
                    endpoint: false,
                }))
        }
        fn finish(&mut self) -> crate::error::Result<String> {
            Ok(self.reply.to_string())
        }
    }

    fn e2e_worker(
        cfg: &Config,
        make: impl Fn() -> RecordingStt + 'static,
    ) -> DictationWorker<RecordingStt, ScriptedCleaner> {
        let loads = Rc::new(RefCell::new(0usize));
        let log = Rc::new(RefCell::new(Vec::new()));
        DictationWorker::new(
            cfg,
            models_with_gemma(ModelState::Ready),
            Box::new(move |_: &Path| Ok(make())),
            counted_cleaner_loader(&loads, &log),
        )
    }

    fn opts(channels: u16, rate: u32, delay_ms: u64) -> audio::WavOptions {
        audio::WavOptions {
            format: audio::AudioFormat {
                channels,
                sample_rate: rate,
            },
            block_frames: 480,
            start_delay: StdDuration::from_millis(delay_ms),
            pace: audio::Pace::Immediate,
            epoch: audio::HostInstant::from_nanos(1),
        }
    }

    fn wav_factory_with(
        samples: Vec<f32>,
        options: audio::WavOptions,
        done: std::sync::mpsc::Sender<audio::SourceDone>,
    ) -> SourceFactory {
        let done = Mutex::new(done);
        Box::new(move |_| {
            let (source, signal) = audio::WavSource::from_samples(samples.clone(), options);
            let _ = done.lock().unwrap().send(signal);
            Box::new(source)
        })
    }

    fn ramp_16k(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| 0.01 + (i % 1_000) as f32 / 10_000.0)
            .collect()
    }

    /// Ein Diktat vom Tastendruck bis zur fertigen Messzeile. `None`, wenn die Session selbst
    /// geendet hat (gesperrt, Mikrofon-Fehler, zu kurz, abgebrochen) — die Zeile steht dann in den Reports.
    fn dictate_e2e(
        session: &mut PttSession,
        cfg: &Config,
        models: &ModelsState,
        worker: &mut DictationWorker<RecordingStt, ScriptedCleaner>,
        done: &Receiver<audio::SourceDone>,
        injected: &Injected,
        debug: Option<(&DebugCapture, &ModelTap)>,
    ) -> Option<DictationTrace> {
        let (ready_tx, ready_rx) = channel();
        let outcome = session.press(audio::HostInstant::now(), cfg, models, move |ready| {
            let _ = ready_tx.send(ready);
        });
        if matches!(outcome, PressOutcome::Blocked(_)) {
            return None;
        }
        let live = match ready_rx.recv_timeout(StdDuration::from_secs(5)).unwrap() {
            MicReady::Recording { live, .. } => live,
            MicReady::Failed(_) => return None,
        };
        // Ein Hilfsthread im echten Ablauf; hier genügt das Warten auf das Quellen-Ende.
        let signal = done.recv_timeout(StdDuration::from_secs(5)).unwrap();
        assert!(signal.wait(StdDuration::from_secs(10)));
        let mut inject = recording_inject(injected);
        match session.release(audio::HostInstant::now()) {
            ReleaseOutcome::Ready {
                pcm,
                frontmost,
                captured,
                trace,
                ..
            } => Some(
                process_job(
                    worker,
                    SttJob::Utterance {
                        pcm,
                        frontmost,
                        captured,
                        trace,
                    },
                    cfg,
                    &mut inject,
                    debug,
                )
                .1,
            ),
            ReleaseOutcome::LiveEnded => Some(
                process_job(
                    worker,
                    SttJob::Live {
                        feed: live.expect("Live-Aufnahme hat einen Feed"),
                    },
                    cfg,
                    &mut inject,
                    debug,
                )
                .1,
            ),
            _ => None,
        }
    }

    /// AK-11/CR-3: im Live-Modus zählt STT nur ab dem Loslassen (`Feed::End`) — die Chunks
    /// während des Sprechens stehen nicht in der Klammer hinter „Loslassen→Text“.
    #[test]
    fn live_stt_time_counts_only_after_the_release() {
        let cfg = live_cfg(InsertMode::PerSentence, CleanupMode::Raw);
        let mut w = e2e_worker(&cfg, || RecordingStt {
            feed_delay: StdDuration::from_millis(60),
            ..RecordingStt::new(&Rc::default(), "Rest.")
        });
        let injected: Injected = Rc::default();
        let (_, trace) = process_job(
            &mut w,
            live_job(&cfg, 3),
            &cfg,
            &mut recording_inject(&injected),
            None,
        );
        let stt = trace.stt.expect("STT nach dem Loslassen gemessen");
        assert!(
            stt < StdDuration::from_millis(60),
            "3 Chunks à 60 ms vor dem Loslassen dürfen nicht zählen: {stt:?}"
        );
        assert_eq!(injected.borrow().as_slice(), ["Rest."]);
    }

    /// AK-9/-12/-21: Batch-Diktat Ende zu Ende → vollständige Messzeile; #1 Kaltstart, #2 nicht.
    #[test]
    fn e2e_batch_dictation_gives_a_complete_measurement_line_and_cold_start_only_first() {
        let (report, reports) = collecting_report();
        let (done_tx, done_rx) = channel();
        let mut session = PttSession::new(
            wav_factory_with(ramp_16k(16_000 * 2), opts(2, 48_000, 100), done_tx),
            report,
        );
        let cfg = Config::default();
        let received = Rc::new(RefCell::new(Vec::new()));
        let mut worker = e2e_worker(&cfg, {
            let received = Rc::clone(&received);
            move || RecordingStt::new(&received, "hallo welt")
        });
        let injected: Injected = Rc::default();
        let models = stt_ready();
        let first = dictate_e2e(
            &mut session,
            &cfg,
            &models,
            &mut worker,
            &done_rx,
            &injected,
            None,
        )
        .unwrap();
        let second = dictate_e2e(
            &mut session,
            &cfg,
            &models,
            &mut worker,
            &done_rx,
            &injected,
            None,
        )
        .unwrap();
        assert_eq!(
            injected.borrow().as_slice(),
            ["[hallo welt]", "[hallo welt]"]
        );
        let line = first.to_string();
        assert!(!line.contains('—'), "alle Felder gefüllt: {line}");
        assert!(line.starts_with("Diktat #1: Taste→Mikro bereit "));
        assert!(
            line.contains("Gerät: WAV: Samples (48 kHz, 2 Kanäle)"),
            "{line}"
        );
        assert!(line.contains("Einfügen: Am Ende") && line.contains("Ergebnis: eingefügt"));
        assert!(line.contains("Kaltstart: ja"));
        assert!(
            second.to_string().contains("Diktat #2:")
                && second.to_string().contains("Kaltstart: nein")
        );
        assert!(
            reports.lock().unwrap().is_empty(),
            "Worker meldet, nicht die Session"
        );
        assert_eq!(first.recorded, Some(StdDuration::from_secs(2)));
    }

    /// AK-20/-21: Startverzögerung 0/100/300 ms × Formate → kein Sample geht bis zum Modell verloren.
    #[test]
    fn e2e_no_sample_is_lost_for_any_start_delay_and_format() {
        let input = ramp_16k(16_000);
        for delay_ms in [0, 100, 300] {
            for (channels, rate) in [(1u16, 16_000u32), (2, 44_100), (1, 48_000)] {
                let (report, _) = collecting_report();
                let (done_tx, done_rx) = channel();
                let mut session = PttSession::new(
                    wav_factory_with(input.clone(), opts(channels, rate, delay_ms), done_tx),
                    report,
                );
                let cfg = live_cfg(InsertMode::AtEnd, CleanupMode::Raw);
                let received = Rc::new(RefCell::new(Vec::new()));
                let mut worker = e2e_worker(&cfg, {
                    let received = Rc::clone(&received);
                    move || RecordingStt::new(&received, "hallo")
                });
                let injected: Injected = Rc::default();
                dictate_e2e(
                    &mut session,
                    &cfg,
                    &stt_ready(),
                    &mut worker,
                    &done_rx,
                    &injected,
                    None,
                )
                .unwrap();
                let got = received.borrow();
                if rate == 16_000 {
                    assert_eq!(*got, input, "{delay_ms} ms, {channels} Kanäle, {rate} Hz");
                } else {
                    assert!(
                        got.len().abs_diff(input.len()) <= 2_048,
                        "{delay_ms} ms, {channels} Kanäle, {rate} Hz: {} statt {}",
                        got.len(),
                        input.len()
                    );
                }
            }
        }
    }

    /// AK-13: nur Null-Blöcke → erstes Signal `—`, Zeile trotzdem vollständig.
    #[test]
    fn e2e_only_zero_blocks_leave_first_signal_empty() {
        let (report, _) = collecting_report();
        let (done_tx, done_rx) = channel();
        let mut session = PttSession::new(
            wav_factory_with(vec![0.0; 16_000], opts(1, 16_000, 0), done_tx),
            report,
        );
        let cfg = live_cfg(InsertMode::AtEnd, CleanupMode::Raw);
        let received = Rc::new(RefCell::new(Vec::new()));
        let mut worker = e2e_worker(&cfg, {
            let received = Rc::clone(&received);
            move || RecordingStt::new(&received, "hallo")
        });
        let injected: Injected = Rc::default();
        let trace = dictate_e2e(
            &mut session,
            &cfg,
            &stt_ready(),
            &mut worker,
            &done_rx,
            &injected,
            None,
        )
        .unwrap();
        let line = trace.to_string();
        assert!(line.contains("→erster Block ") && !line.contains("→erster Block —"));
        assert!(line.contains("→erstes Signal —"), "{line}");
        assert!(line.contains("Ergebnis: eingefügt"));
    }

    /// AK-13: zu kurz / gesperrt / Mikrofon-Fehler enden in der Session mit eigener Zeile.
    #[test]
    fn e2e_early_endings_report_their_own_line_and_never_reach_the_worker() {
        let cfg = live_cfg(InsertMode::AtEnd, CleanupMode::Raw);
        let received = Rc::new(RefCell::new(Vec::new()));
        let mut worker = e2e_worker(&cfg, {
            let received = Rc::clone(&received);
            move || RecordingStt::new(&received, "hallo")
        });
        let injected: Injected = Rc::default();

        // zu kurz
        let (report, reports) = collecting_report();
        let (done_tx, done_rx) = channel();
        let mut session = PttSession::new(
            wav_factory_with(ramp_16k(1_600), opts(1, 16_000, 0), done_tx),
            report,
        );
        assert!(
            dictate_e2e(
                &mut session,
                &cfg,
                &stt_ready(),
                &mut worker,
                &done_rx,
                &injected,
                None
            )
            .is_none()
        );
        assert!(
            reports.lock().unwrap()[0]
                .to_string()
                .ends_with("Ergebnis: verworfen (zu kurz)")
        );

        // gesperrt
        let (report, reports) = collecting_report();
        let (done_tx, done_rx) = channel();
        let mut session = PttSession::new(
            wav_factory_with(ramp_16k(16_000), opts(1, 16_000, 0), done_tx),
            report,
        );
        let closed = ModelsState::new(ModelState::Missing, ModelState::Missing);
        assert!(
            dictate_e2e(
                &mut session,
                &cfg,
                &closed,
                &mut worker,
                &done_rx,
                &injected,
                None
            )
            .is_none()
        );
        assert!(
            reports.lock().unwrap()[0]
                .to_string()
                .ends_with("Ergebnis: gesperrt")
        );

        // Mikrofon-Start scheitert
        let (report, reports) = collecting_report();
        let (_unused_tx, done_rx) = channel();
        let mut session = PttSession::new(Box::new(|_| Box::new(FailingSource)), report);
        assert!(
            dictate_e2e(
                &mut session,
                &cfg,
                &stt_ready(),
                &mut worker,
                &done_rx,
                &injected,
                None
            )
            .is_none()
        );
        let line = reports.lock().unwrap()[0].to_string();
        assert!(
            line.contains("Taste→Mikro bereit —")
                && line.contains("Ergebnis: Fehler (Audio-Aufnahme fehlgeschlagen: kein Mikrofon")
        );
        assert!(injected.borrow().is_empty(), "nichts erreicht den Worker");
    }

    /// AK-13: STT-Fehler → Ergebnis Fehler, Zeile vollständig, nichts eingefügt.
    #[test]
    fn e2e_stt_error_gives_an_error_line_without_insertion() {
        let (report, _) = collecting_report();
        let (done_tx, done_rx) = channel();
        let mut session = PttSession::new(
            wav_factory_with(ramp_16k(16_000), opts(1, 16_000, 0), done_tx),
            report,
        );
        let cfg = live_cfg(InsertMode::AtEnd, CleanupMode::Raw);
        let received = Rc::new(RefCell::new(Vec::new()));
        let mut worker = e2e_worker(&cfg, {
            let received = Rc::clone(&received);
            move || {
                let mut stt = RecordingStt::new(&received, "hallo");
                stt.fail = true;
                stt
            }
        });
        let injected: Injected = Rc::default();
        let trace = dictate_e2e(
            &mut session,
            &cfg,
            &stt_ready(),
            &mut worker,
            &done_rx,
            &injected,
            None,
        )
        .unwrap();
        assert!(injected.borrow().is_empty());
        assert!(
            trace
                .to_string()
                .ends_with("Ergebnis: Fehler (Spracherkennung fehlgeschlagen)")
        );
    }

    /// AK-11/AK-7: Live-Diktat mit 3 Segmenten Ende zu Ende; auf `info` genau eine Zeile.
    #[test]
    fn e2e_live_dictation_with_three_segments_logs_one_info_line() {
        let (report, _) = collecting_report();
        let (done_tx, done_rx) = channel();
        let mut session = PttSession::new(
            wav_factory_with(ramp_16k(16_000 * 3), opts(1, 16_000, 0), done_tx),
            report,
        );
        let cfg = live_cfg(InsertMode::PerSentence, CleanupMode::Raw);
        let received = Rc::new(RefCell::new(Vec::new()));
        let mut worker = e2e_worker(&cfg, {
            let received = Rc::clone(&received);
            move || {
                let mut stt = RecordingStt::new(&received, "Dritter.");
                stt.updates = [
                    ("a", false),
                    ("Erster.", true),
                    ("b", false),
                    ("Zweiter.", true),
                ]
                .iter()
                .map(|(text, endpoint)| crate::stt::StreamUpdate {
                    text: (*text).to_string(),
                    endpoint: *endpoint,
                })
                .collect();
                stt
            }
        });
        let injected: Injected = Rc::default();
        let models = stt_ready();
        let log_report = crate::diagnostics::log_report();
        let (trace, lines) = crate::logging::capture(|| {
            let trace = dictate_e2e(
                &mut session,
                &cfg,
                &models,
                &mut worker,
                &done_rx,
                &injected,
                None,
            )
            .unwrap();
            log_report(trace.clone());
            trace
        });
        assert_eq!(
            injected.borrow().as_slice(),
            ["Erster.", " Zweiter.", " Dritter."]
        );
        assert_eq!(trace.segments, Some(3));
        let line = trace.to_string();
        assert!(
            line.contains("→erstes Segment ") && line.contains("Segmente 3"),
            "{line}"
        );
        let on_info: Vec<_> = lines
            .iter()
            .filter(|(l, _)| *l <= log::Level::Info)
            .collect();
        assert_eq!(on_info.len(), 1, "{on_info:?}");
    }

    /// AK-15/-21: Debug-Mitschnitt Ende zu Ende; mic.wav = die gelieferten Blöcke (2 Kanäle, 44,1 kHz).
    #[test]
    fn e2e_debug_capture_mic_wav_matches_the_delivered_blocks() {
        let dir = DebugDir::new("e2e");
        let capture = debug_capture(&dir.0);
        let tap = ModelTap::default();
        let input = ramp_16k(16_000);
        // Erwartete Blöcke: dieselbe Quelle separat vollständig einsammeln.
        let (reference, reference_done) =
            audio::WavSource::from_samples(input.clone(), opts(2, 44_100, 0));
        let delivered = Arc::new(Mutex::new(Vec::<f32>::new()));
        let sink = Arc::clone(&delivered);
        let _guard = Box::new(reference)
            .open(Box::new(move |data, _| {
                sink.lock().unwrap().extend_from_slice(data)
            }))
            .unwrap();
        assert!(reference_done.wait(StdDuration::from_secs(10)));

        let (report, _) = collecting_report();
        let (done_tx, done_rx) = channel();
        let mut session =
            PttSession::new(wav_factory_with(input, opts(2, 44_100, 0), done_tx), report);
        let cfg = live_cfg(InsertMode::AtEnd, CleanupMode::Raw);
        let received = Rc::new(RefCell::new(Vec::new()));
        let mut worker = e2e_worker(&cfg, {
            let received = Rc::clone(&received);
            move || RecordingStt::new(&received, "hallo")
        });
        let injected: Injected = Rc::default();
        let trace = dictate_e2e(
            &mut session,
            &cfg,
            &stt_ready(),
            &mut worker,
            &done_rx,
            &injected,
            Some((&capture, &tap)),
        )
        .unwrap();
        let path = dir.0.join(format!("{}-mic.wav", trace.nr));
        let mut reader = hound::WavReader::open(path).unwrap();
        assert_eq!(
            (reader.spec().channels, reader.spec().sample_rate),
            (2, 44_100)
        );
        let written: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
        let expected = delivered.lock().unwrap();
        assert_eq!(
            written.iter().map(|s| s.to_bits()).collect::<Vec<_>>(),
            expected.iter().map(|s| s.to_bits()).collect::<Vec<_>>()
        );
        assert!(dir.0.join(format!("{}-model.wav", trace.nr)).is_file());
    }

    /// AK-8 (Pipeline-Seite): blockierendes Log-Backend → Ergebnis unverändert, kein Hängen.
    #[test]
    fn e2e_blocking_log_backend_does_not_stall_or_change_the_dictation() {
        struct StuckSink(std::sync::mpsc::Receiver<()>);
        impl crate::logging::LogSink for StuckSink {
            fn write_line(&mut self, _: &str) -> std::io::Result<()> {
                let _ = self.0.recv();
                Ok(())
            }
        }
        let (finished_tx, finished_rx) = channel();
        std::thread::spawn(move || {
            let (_keep_blocked, gate) = channel();
            let logger = Arc::new(crate::logging::Logger::spawn(Box::new(StuckSink(gate)), 2));
            // Puffer überfluten, damit der Kanal voll ist.
            for i in 0..50 {
                log::Log::log(
                    &*logger,
                    &log::Record::builder()
                        .level(log::Level::Error)
                        .target("talker::x")
                        .args(format_args!("flut {i}"))
                        .build(),
                );
            }
            let sink_logger = Arc::clone(&logger);
            let report: TraceReport = Arc::new(move |t| {
                log::Log::log(
                    &*sink_logger,
                    &log::Record::builder()
                        .level(log::Level::Info)
                        .target("talker::messung")
                        .args(format_args!("{t}"))
                        .build(),
                );
            });
            let (done_tx, done_rx) = channel();
            let mut session = PttSession::new(
                wav_factory_with(ramp_16k(16_000), opts(1, 16_000, 0), done_tx),
                Arc::clone(&report),
            );
            let cfg = live_cfg(InsertMode::AtEnd, CleanupMode::Raw);
            let received = Rc::new(RefCell::new(Vec::new()));
            let mut worker = e2e_worker(&cfg, {
                let received = Rc::clone(&received);
                move || RecordingStt::new(&received, "hallo")
            });
            let injected: Injected = Rc::default();
            let trace = dictate_e2e(
                &mut session,
                &cfg,
                &stt_ready(),
                &mut worker,
                &done_rx,
                &injected,
                None,
            )
            .unwrap();
            report(trace.clone());
            let _ = finished_tx.send((trace.result, injected.borrow().clone()));
        });
        let (result, injected) = finished_rx
            .recv_timeout(StdDuration::from_secs(10))
            .expect("Dictation hängt am blockierenden Log-Backend");
        assert_eq!(result, DictationResult::Inserted);
        assert_eq!(injected, ["hallo"]);
    }
}
