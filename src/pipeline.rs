//! Dictation Worker: verarbeitet den Strom von Utterances — besitzt STT-Engine
//! und Cleanup-LLM, verwaltet deren Lebenszyklus (Lazy-Load, Fehler-Cache,
//! Modellwechsel zur Laufzeit, Live-Aktivierung nach Modell-Download) und
//! liefert pro Utterance den fertigen Text bzw. einen Ablehnungsgrund.
//! `main.rs` drainiert nur noch den Channel und mappt `Outcome` auf
//! Indicator/Injection (Ticket-0034).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::cleanup::{self, CleanupMode, LlmCleaner};
use crate::config::{Config, InsertMode};
use crate::error::TalkerError;
use crate::injection;
use crate::models::{ModelId, ModelsState};
use crate::stt::{PhraseCommitter, StreamingTranscriber, Transcriber};
use crate::{audio, vocab_match};

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
        };
        let t0 = Instant::now();
        match (worker.load_transcriber)(&worker.stt_dir) {
            Ok(t) => {
                worker.transcriber = Some(t);
                eprintln!(
                    "talker: STT-Modell geladen in {:.1}s.",
                    t0.elapsed().as_secs_f32()
                );
            }
            Err(e) => eprintln!("talker: STT nicht verfügbar — {e}"),
        }
        if worker.llm_was_available && config_wants_llm(cfg) {
            (worker.cleaner, worker.cleaner_failed) = (worker.load_cleaner)();
        }
        worker
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
            eprintln!(
                "talker: Kontext-Regel aktiv ({}) → Modus {}.",
                frontmost.unwrap_or("?"),
                resolved.label()
            );
        }

        // STT-Pfad geändert → Modell neu laden (alter bleibt bei Fehler aktiv).
        if cfg.stt_model_dir != self.stt_dir || self.transcriber.is_none() {
            match (self.load_transcriber)(&cfg.stt_model_dir) {
                Ok(t) => {
                    self.transcriber = Some(t);
                    self.stt_dir = cfg.stt_model_dir.clone();
                    eprintln!("talker: STT-Modell neu geladen: {}", self.stt_dir.display());
                }
                Err(e) => eprintln!("talker: STT-Neuladen fehlgeschlagen — {e}"),
            }
        }
        if self.transcriber.is_none() {
            eprintln!("talker: kein STT-Modell — Utterance verworfen.");
            return Outcome::Rejected("Kein STT-Modell");
        }

        if resolved.uses_llm() && self.cleaner.is_none() && !self.cleaner_failed {
            (self.cleaner, self.cleaner_failed) = (self.load_cleaner)();
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
        match process_utterance(pcm, &cfg, transcriber, cleaner_ref, self.cleaner_failed) {
            Processed::Text {
                text,
                cleanup_fell_back,
            } => Outcome::Inject {
                text,
                cleanup_fell_back,
            },
            Processed::Empty => {
                eprintln!("talker: leerer Transcript — nichts einzufügen.");
                Outcome::Rejected("Nichts erkannt")
            }
            Processed::SttFailed(e) => {
                eprintln!("talker: STT fehlgeschlagen: {e}");
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
            match (self.load_transcriber)(&cfg.stt_model_dir) {
                Ok(t) => {
                    self.transcriber = Some(t);
                    self.stt_dir = cfg.stt_model_dir.clone();
                }
                Err(e) => eprintln!("talker: STT-Neuladen fehlgeschlagen — {e}"),
            }
        }
        let Some(transcriber) = self.transcriber.as_mut() else {
            return LiveStart::Unavailable;
        };
        transcriber.begin();
        if resolved.uses_llm() && self.cleaner.is_none() && !self.cleaner_failed {
            (self.cleaner, self.cleaner_failed) = (self.load_cleaner)();
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
        let update = match transcriber.feed(pcm) {
            Ok(update) => update,
            Err(e) => {
                eprintln!("talker: Live-STT fehlgeschlagen: {e}");
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
        let rest = match transcriber.finish() {
            Ok(rest) => rest,
            Err(e) => {
                eprintln!("talker: Live-STT fehlgeschlagen: {e}");
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
        let (text, cleanup_fell_back) =
            postprocess(raw.to_string(), &live.cfg, cleaner, self.cleaner_failed);
        eprintln!("talker: Live-Segment: {text}");
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
            eprintln!("talker: Live-Audio unbrauchbar: {e}");
            emit(Outcome::Rejected("Aufnahme fehlgeschlagen"));
            return;
        }
    };
    if worker.live_begin(frontmost, cfg) == LiveStart::Unavailable {
        let raw: Vec<f32> = blocks.into_iter().flatten().collect();
        match audio::to_mono_16k(&raw, channels, sample_rate) {
            Ok(pcm) => emit(worker.handle(&pcm, frontmost, cfg)),
            Err(e) => {
                eprintln!("talker: Fallback-Aufnahme unbrauchbar: {e}");
                emit(Outcome::Rejected("Aufnahme fehlgeschlagen"));
            }
        }
        return;
    }
    for block in blocks {
        match assembler.push(&block) {
            Ok(chunks) => {
                for chunk in chunks {
                    worker.live_chunk(&chunk).into_iter().for_each(&mut *emit);
                }
            }
            Err(e) => {
                eprintln!("talker: Live-Audio fehlgeschlagen: {e}");
                emit(Outcome::Rejected("Aufnahme fehlgeschlagen"));
            }
        }
    }
    match assembler.flush() {
        Ok(rest) if !rest.is_empty() => {
            worker.live_chunk(&rest).into_iter().for_each(&mut *emit);
        }
        Ok(_) => {}
        Err(e) => eprintln!("talker: Live-Audio-Rest verworfen: {e}"),
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
            eprintln!(
                "talker: Cleaned Transcript (Cleanup {} ms): {cleaned}",
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
    eprintln!(
        "talker: Raw Transcript ({audio_ms} ms Audio, STT {} ms): {raw}",
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

/// Zustand einer laufenden PTT-Utterance zwischen Tastendruck und Loslassen —
/// ersetzt die früheren `thread_local!`-Zellen in `main.rs` (Ticket-0038).
/// Läuft ausschließlich auf dem Main-Thread (Hotkey-Callbacks), geteilt über
/// `Arc<Mutex<PttSession>>`, weil `hotkey::install` `Send`-Closures verlangt.
#[derive(Default)]
pub struct PttSession {
    slot: Option<Arc<Mutex<MicSlot<audio::Recording>>>>,
    frontmost: Option<String>,
    /// Die laufende Aufnahme speist das Live-Diktat (Block-Kanal).
    live: bool,
}

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

/// Rohe Audio-Blöcke einer Live-Aufnahme samt Format und Ziel-App.
pub struct LiveFeed {
    pub frontmost: Option<String>,
    pub blocks: std::sync::mpsc::Receiver<Vec<f32>>,
    pub format: (u16, u32),
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
    /// PCM + der bei Press erfasste Frontmost, bereit für den STT-Worker.
    Ready {
        pcm: Vec<f32>,
        frontmost: Option<String>,
        ms: u64,
    },
    /// Stop der Aufnahme fehlgeschlagen.
    StopFailed(TalkerError),
}

impl PttSession {
    pub fn new() -> Self {
        Self::default()
    }

    /// PTT gedrückt: Setup-Gate prüfen, Frontmost-App VOR Aufnahmestart
    /// erfassen (Kontext-Awareness, Ticket-0026 — bleibt stabil, auch wenn
    /// die App während der Utterance wechselt), Mikrofon im Hintergrund
    /// starten. Der Tastendruck-Callback blockiert nie auf CoreAudio;
    /// `on_ready` meldet den Start (oder seinen Fehler) vom Start-Thread aus.
    pub fn press(
        &mut self,
        cfg: &Config,
        models: &ModelsState,
        on_ready: impl FnOnce(MicReady) + Send + 'static,
    ) -> PressOutcome {
        let pressed_at = Instant::now();
        if let Some(hint) = models.get(ModelId::Stt).setup_hint() {
            return PressOutcome::Blocked(hint);
        }
        let frontmost = injection::frontmost_bundle_id();
        let live = cfg.insert_mode != InsertMode::AtEnd;
        let mut mode = resolve_mode(cfg, frontmost.as_deref());
        if live {
            // Im Live-Betrieb zeigt der Indicator den tatsächlich wirkenden Modus.
            mode = cfg.insert_mode.resolve(mode);
        }
        let device = cfg.mic_device.clone();
        let slot = Arc::new(Mutex::new(MicSlot::Starting));
        self.slot = Some(Arc::clone(&slot));
        self.frontmost = frontmost.clone();
        self.live = live;
        std::thread::spawn(move || {
            let (tx, rx) = std::sync::mpsc::channel();
            let started = audio::start(device.as_deref(), live.then_some(tx));
            eprintln!(
                "talker: Mikrofon-Start dauerte {} ms",
                pressed_at.elapsed().as_millis()
            );
            complete_start(&slot, started, |result| match result {
                Ok(rec) => on_ready(MicReady::Recording {
                    level: rec.level(),
                    mode,
                    live: live.then(|| LiveFeed {
                        frontmost,
                        blocks: rx,
                        format: rec.format(),
                    }),
                }),
                Err(e) => on_ready(MicReady::Failed(e)),
            });
        });
        PressOutcome::Starting
    }

    /// PTT losgelassen: Aufnahme stoppen, Mindestlänge prüfen (versehentlicher
    /// Tastendruck), sonst PCM + Frontmost zum Senden an den STT-Worker freigeben.
    pub fn release(&mut self) -> ReleaseOutcome {
        let frontmost = self.frontmost.take();
        let Some(slot) = self.slot.take() else {
            return ReleaseOutcome::NoRecording;
        };
        let rec = match take_on_release(&slot) {
            Taken::Ready(rec) => rec,
            Taken::StartAborted => return ReleaseOutcome::StartAborted,
            Taken::Gone => return ReleaseOutcome::NoRecording,
        };
        if std::mem::take(&mut self.live) {
            rec.finish_live();
            return ReleaseOutcome::LiveEnded;
        }
        match rec.stop() {
            Ok(pcm) => {
                let ms = audio::duration_ms(&pcm);
                if ms < audio::MIN_UTTERANCE_MS {
                    ReleaseOutcome::TooShort { ms }
                } else {
                    ReleaseOutcome::Ready { pcm, frontmost, ms }
                }
            }
            Err(e) => ReleaseOutcome::StopFailed(e),
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
        let mut session = PttSession::new();

        let outcome = session.press(&Config::default(), &models, |_| {});

        assert!(matches!(outcome, PressOutcome::Blocked(_)));
    }

    #[test]
    fn release_without_a_press_reports_no_recording() {
        let mut session = PttSession::new();

        assert!(matches!(session.release(), ReleaseOutcome::NoRecording));
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
}
