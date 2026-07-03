//! Pipeline-Kern: eine Utterance durch STT → vocab_match → Cleanup führen.
//!
//! Reine Funktion ohne Channel-/Thread-/UI-Kopplung (Ticket-0015) — der
//! Worker in `main.rs` besitzt Modelle und Config und ruft hier hinein.

use std::time::Instant;

use crate::cleanup::{self, CleanupMode, LlmCleaner};
use crate::config::Config;
use crate::error::TalkerError;
use crate::stt::Transcriber;
use crate::{audio, vocab_match};

/// Ergebnis einer verarbeiteten Utterance.
#[derive(Debug)]
pub enum Processed {
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
pub fn process_utterance(
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
/// Modells in `main.rs` — pro Utterance entscheidet `resolve_mode`.
pub fn config_wants_llm(cfg: &Config) -> bool {
    cfg.cleanup_mode.uses_llm()
        || (cfg.context_aware_enabled && cfg.context_rules.iter().any(|(_, m)| m.uses_llm()))
}

/// Roh-Modus setzt Cleaner und Fehler-Cache zurück: ein späterer Wechsel auf
/// einen LLM-Modus versucht das Laden dadurch erneut.
pub fn reset_cleaner_on_raw_mode<C>(mode: CleanupMode, cleaner: &mut Option<C>, failed: &mut bool) {
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
}
