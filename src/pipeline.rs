//! Dictation Worker: verarbeitet den Strom von Utterances — besitzt STT-Engine
//! und Cleanup-LLM, verwaltet deren Lebenszyklus (Lazy-Load, Fehler-Cache,
//! Modellwechsel zur Laufzeit, Live-Aktivierung nach Modell-Download) und
//! liefert pro Utterance den fertigen Text bzw. einen Ablehnungsgrund.
//! `main.rs` drainiert nur noch den Channel und mappt `Outcome` auf
//! Indicator/Injection (Ticket-0034).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::cleanup::{self, CleanupMode, LlmCleaner};
use crate::config::Config;
use crate::error::TalkerError;
use crate::models::ModelsState;
use crate::stt::Transcriber;
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

/// Lädt die STT-Engine aus einem Modell-Verzeichnis. Zwei Adapter: Parakeet
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

    /// Verarbeitet eine Utterance unter der übergebenen Config-Momentaufnahme.
    /// Bricht nie hart ab: jeder Fehlerpfad wird zu einem `Outcome`.
    pub fn handle(&mut self, pcm: &[f32], frontmost: Option<&str>, cfg: &Config) -> Outcome {
        // Live-Aktivierung (Ticket-0029): wird gemma nachträglich ready,
        // den Fehler-Cache löschen — der nächste LLM-Bedarf lädt dann.
        let llm_available = self.models.llm_modes_available();
        if llm_available && !self.llm_was_available {
            self.cleaner_failed = false;
        }
        self.llm_was_available = llm_available;

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
    // Deterministische Vokabular-Korrektur (Kölner Phonetik) — vor dem
    // Cleanup, wirkt damit auch im Roh-Modus; per Config abschaltbar.
    let raw = if cfg.phonetic_matching {
        vocab_match::apply(&raw, &cfg.vocabulary)
    } else {
        raw
    };
    let (text, cleanup_fell_back) = match cleaner {
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
    };
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
