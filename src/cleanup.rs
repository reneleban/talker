//! cleanup: Raw Transcript → Cleaned Transcript via lokalem LLM.
//!
//! Runtime laut Spike-0001/ADR-0003: eingebettetes llama.cpp (`llama-cpp-2`),
//! Modell gemma4:e2b (GGUF), kein Server. Ausfallsicher: jeder Fehler fällt
//! auf den unveränderten Raw Transcript zurück — die Pipeline bricht nie ab.

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::time::{Duration, Instant};

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;

use serde::{Deserialize, Serialize};

use crate::error::{Result, TalkerError};

/// Cleanup-Modus: benanntes Prompt-Profil an dasselbe Modell (CONTEXT.md).
/// `Raw` umgeht das LLM komplett. Genau ein Modus ist aktiv.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CleanupMode {
    Raw,
    #[default]
    Business,
    Casual,
    #[serde(rename = "llm")]
    LlmOptimized,
}

impl CleanupMode {
    pub const ALL: [CleanupMode; 4] = [
        CleanupMode::Raw,
        CleanupMode::Business,
        CleanupMode::Casual,
        CleanupMode::LlmOptimized,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CleanupMode::Raw => "Roh",
            CleanupMode::Business => "Geschäftlich",
            CleanupMode::Casual => "Natürlich (Stil erhalten)",
            CleanupMode::LlmOptimized => "LLM-optimiert",
        }
    }

    /// Braucht dieser Modus das LLM?
    pub fn uses_llm(self) -> bool {
        self != CleanupMode::Raw
    }
}

/// Raw Transcript → Cleaned Transcript.
pub trait LlmCleaner {
    fn clean(&mut self, raw: &str) -> Result<String>;
}

/// Ausfallsicherer Einstiegspunkt: Fehler/Timeout/leerer Output → Raw Transcript
/// unverändert. Das zweite Element meldet den Fallback (fürs UI, Ticket-0009).
pub fn clean_with_fallback(cleaner: &mut dyn LlmCleaner, raw: &str) -> (String, bool) {
    match cleaner.clean(raw) {
        Ok(cleaned) if !cleaned.is_empty() => (cleaned, false),
        Ok(_) => {
            eprintln!("talker: Cleanup lieferte leeren Text — nutze Raw Transcript.");
            (raw.to_string(), true)
        }
        Err(e) => {
            eprintln!("talker: Cleanup fehlgeschlagen ({e}) — nutze Raw Transcript.");
            (raw.to_string(), true)
        }
    }
}

/// Harte Obergrenze, danach Fallback auf den Raw Transcript.
const CLEAN_TIMEOUT: Duration = Duration::from_secs(15);
/// Kontextgröße; Diktat-Utterances sind kurz, 4K reicht für v1.
const N_CTX: u32 = 4096;
/// Prompt-Obergrenze (Token). Darüber liefert `clean` einen Fehler (→ Raw-
/// Fallback) statt llama.cpp in einen nicht abfangbaren C-Abort laufen zu
/// lassen (GGML_ASSERT n_tokens ≤ n_batch; Eval-0001, 1600-Wörter-Fall).
const MAX_PROMPT_TOKENS: usize = 3000;

pub struct GemmaCleaner {
    backend: LlamaBackend,
    model: LlamaModel,
    mode: CleanupMode,
    vocab: Vec<String>,
}

/// System-Instruktion des Modus; `None` für Raw (kein LLM-Call — Bypass).
/// Gemeinsamer Kern: Werkzeug-Rolle, Delimiter, kein Nachdenken (Spike-0001).
pub fn mode_instruction(mode: CleanupMode) -> Option<&'static str> {
    match mode {
        CleanupMode::Raw => None,
        CleanupMode::Business => Some(
            "Du bist ein Korrektur-Werkzeug, kein Gesprächspartner. Du bekommst ein rohes \
             Diktat-Transkript zwischen den Markern <<< und >>>. Gib denselben Text in \
             geschäftlichem Stil zurück: Füllwörter (ähm, äh, also, halt, ne) entfernen, \
             ebenso unsichere Floskeln ohne Informationswert (glaube ich, denke ich, \
             irgendwie, oder so); vollständige Sätze, korrekte Interpunktion und \
             Groß-/Kleinschreibung, keine Umgangssprache. Zahlen, Datums- und \
             Zeitangaben EXAKT wie diktiert übernehmen. Den Inhalt sonst NICHT verändern, \
             NICHTS hinzufügen, NICHTS weglassen. Fragen NICHT beantworten — nur den \
             Fragesatz sauber zurückgeben. Antworte in der Sprache des Transkripts. Gib \
             ausschließlich den Text aus, ohne Marker, ohne Erklärung, ohne Nachdenken.",
        ),
        CleanupMode::Casual => Some(
            "Du bist ein Korrektur-Werkzeug, kein Gesprächspartner. Du bekommst ein rohes \
             Diktat-Transkript zwischen den Markern <<< und >>>. Gib denselben Text zurück \
             und erhalte dabei den Sprachstil des Sprechers unverändert — locker bleibt \
             locker, förmlich bleibt förmlich, Wortwahl und Ton bleiben wie diktiert. \
             Korrigiere nur: Füllwörter (ähm, äh) raus, Interpunktion und \
             Groß-/Kleinschreibung setzen. Den Inhalt NICHT verändern, NICHTS hinzufügen, \
             NICHTS weglassen. Fragen NICHT beantworten. Antworte in der Sprache des \
             Transkripts. Gib ausschließlich den Text aus, ohne Marker, ohne Erklärung, \
             ohne Nachdenken.",
        ),
        CleanupMode::LlmOptimized => Some(
            "Du bist ein Prompt-Aufbereitungs-Werkzeug, kein Gesprächspartner. Du bekommst \
             zwischen den Markern <<< und >>> eine diktierte, umgangssprachliche Anweisung \
             an eine AI-Coding-CLI (z.B. Claude Code). Forme daraus einen klaren, \
             copy-paste-fertigen Prompt: eindeutige, direkte Anweisung; Rambling, Füllsel \
             und Selbstkorrekturen raus; alle technischen Details (Dateinamen, Werte, \
             Bedingungen) exakt übernehmen; bei mehreren Punkten eine nummerierte Liste. \
             Zahlen und Einheiten EXAKT wie diktiert — NIEMALS Einheiten, Werte oder \
             Details ergänzen, die nicht diktiert wurden. Die Anweisung NICHT ausführen \
             und NICHT beantworten — nur als Prompt formulieren. Enthält das Diktat gar \
             keine Anweisung, gib es einfach nur bereinigt zurück (Füllwörter raus, \
             Interpunktion setzen), ohne etwas zu erfinden. Antworte in der Sprache des \
             Transkripts. Gib ausschließlich den Text aus, ohne Marker, ohne Erklärung, \
             ohne Nachdenken.",
        ),
    }
}

/// Few-Shot-Paare (Roh-Diktat → gewünschter Output) je Modus — verankern das
/// Verhalten; ohne sie beantwortet Gemma kurze Fragen statt sie zu bereinigen.
fn mode_examples(mode: CleanupMode) -> &'static [(&'static str, &'static str)] {
    match mode {
        CleanupMode::Raw => &[],
        CleanupMode::Business => &[
            (
                "ähm also wo stehen wir denn gerade",
                "Wo stehen wir gerade?",
            ),
            (
                "ich glaub das meeting äh verschieben wir auf freitag ne",
                "Das Meeting verschieben wir auf Freitag.",
            ),
            // Englisches Diktat bleibt englisch (EVAL-0003: Business übersetzte sonst).
            (
                "uh we should probably ship the fix on monday",
                "We should ship the fix on Monday.",
            ),
        ],
        CleanupMode::Casual => &[
            (
                "ähm also wo stehen wir denn gerade",
                "Also, wo stehen wir denn gerade?",
            ),
            (
                "ich glaub das meeting äh verschieben wir auf freitag ne",
                "Ich glaub, das Meeting verschieben wir auf Freitag, ne?",
            ),
            (
                "uh we should probably ship the fix on monday",
                "We should probably ship the fix on Monday.",
            ),
        ],
        CleanupMode::LlmOptimized => &[
            (
                "ähm also ich glaub wir sollten in der config datei noch n feld für die \
                 breite einbauen und das soll dann äh in prozent sein glaub ich",
                "Füge in der Config-Datei ein Feld für die Breite hinzu. Der Wert ist \
                 eine Prozentangabe.",
            ),
            (
                "mach mal dass der button rot wird wenn man drüber hovert und ähm ach ja \
                 und der test dazu soll auch noch grün werden",
                "1. Färbe den Button beim Hovern rot.\n2. Bring den zugehörigen Test \
                 zum Laufen.",
            ),
            // Keine Anweisung → nur bereinigen, nichts erfinden.
            (
                "ähm das essen in der kantine war heute echt gut ne",
                "Das Essen in der Kantine war heute echt gut.",
            ),
        ],
    }
}

/// Vokabular-Block für die Instruktion (leer bei leerer Liste).
fn vocab_block(vocab: &[String]) -> String {
    if vocab.is_empty() {
        return String::new();
    }
    format!(
        " Folgende Fachbegriffe kommen im Diktat wahrscheinlich vor — wenn ein Wort \
         im Transkript ähnlich klingt (Verhörer der Spracherkennung), verwende EXAKT \
         diese Schreibweise: {}. Achtung: Die Spracherkennung deutscht solche Begriffe \
         oft ein oder verballhornt sie — korrigiere auch solche Formen mutig auf den \
         Listen-Begriff (Beispiele für dieses Prinzip: »kubanetis« → »Kubernetes«, \
         »darker image« → »Docker Image«, »ekwi« → »egui«).",
        vocab.join(", ")
    )
}

/// Vollständiger Gemma-Prompt für einen Modus (manuelles Turn-Format).
fn build_prompt(mode: CleanupMode, raw: &str, vocab: &[String]) -> Option<String> {
    let instruction = mode_instruction(mode)?;
    let mut p = format!(
        "<start_of_turn>user\n{instruction}{}\n\n",
        vocab_block(vocab)
    );
    let mut first = true;
    for (raw_ex, cleaned_ex) in mode_examples(mode) {
        if first {
            p.push_str(&format!("<<<{raw_ex}>>><end_of_turn>\n"));
            first = false;
        } else {
            p.push_str(&format!(
                "<start_of_turn>user\n<<<{raw_ex}>>><end_of_turn>\n"
            ));
        }
        p.push_str(&format!(
            "<start_of_turn>model\n{cleaned_ex}<end_of_turn>\n"
        ));
    }
    p.push_str(&format!(
        "<start_of_turn>user\n<<<{raw}>>><end_of_turn>\n<start_of_turn>model\n"
    ));
    Some(p)
}

impl GemmaCleaner {
    /// Lädt das gemma4:e2b-GGUF. Fehlende Datei → klarer Fehler.
    pub fn new(model_path: &Path) -> Result<Self> {
        if !model_path.is_file() {
            return Err(TalkerError::Cleanup(format!(
                "Modell-Datei fehlt: {} — gemma-4-E2B-it-GGUF (Q8_0) dort ablegen",
                model_path.display()
            )));
        }
        // llama.cpp loggt sonst jede Metal-Pipeline auf stderr.
        llama_cpp_2::send_logs_to_tracing(
            llama_cpp_2::LogOptions::default().with_logs_enabled(false),
        );
        let backend = LlamaBackend::init()
            .map_err(|e| TalkerError::Cleanup(format!("llama.cpp-Backend: {e}")))?;
        let params = pin!(LlamaModelParams::default().with_n_gpu_layers(1000));
        let model = LlamaModel::load_from_file(&backend, model_path, &params)
            .map_err(|e| TalkerError::Cleanup(format!("Modell laden: {e}")))?;
        Ok(Self {
            backend,
            model,
            mode: CleanupMode::default(),
            vocab: Vec::new(),
        })
    }

    /// Aktives Stil-Profil setzen (wirkt ab dem nächsten `clean`).
    pub fn set_mode(&mut self, mode: CleanupMode) {
        self.mode = mode;
    }

    /// Nutzer-Vokabular setzen (wirkt ab dem nächsten `clean`).
    pub fn set_vocab(&mut self, vocab: &[String]) {
        self.vocab = vocab.to_vec();
    }

    /// Default-Ablageort: ~/Library/Application Support/talker/models/…
    pub fn default_model_path() -> PathBuf {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        home.join("Library/Application Support/talker/models/gemma-4-E2B-it-Q8_0.gguf")
    }
}

impl LlmCleaner for GemmaCleaner {
    fn clean(&mut self, raw: &str) -> Result<String> {
        if raw.trim().is_empty() {
            return Ok(raw.to_string());
        }
        let map =
            |what: &str, e: &dyn std::fmt::Display| TalkerError::Cleanup(format!("{what}: {e}"));

        // n_batch = n_ctx: Prompts bis MAX_PROMPT_TOKENS in einem Batch decodierbar
        // (Default 2048 würde bei langen Diktaten hart abbrechen).
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(Some(NonZeroU32::new(N_CTX).unwrap()))
            .with_n_batch(N_CTX);
        let mut ctx = self
            .model
            .new_context(&self.backend, ctx_params)
            .map_err(|e| map("Kontext", &e))?;

        // Raw hat kein Profil — der Worker umgeht das LLM; defensiv: Durchreichen.
        let Some(prompt) = build_prompt(self.mode, raw, &self.vocab) else {
            return Ok(raw.to_string());
        };
        let tokens = self
            .model
            .str_to_token(&prompt, AddBos::Always)
            .map_err(|e| map("Tokenisierung", &e))?;
        let n_prompt = tokens.len();
        if n_prompt > MAX_PROMPT_TOKENS {
            return Err(TalkerError::Cleanup(format!(
                "Transkript zu lang für den Cleanup ({n_prompt} Token, max {MAX_PROMPT_TOKENS})"
            )));
        }
        // Cleaned ≈ Raw-Länge; großzügiges Budget, hart gedeckelt vom Kontext.
        let max_new = (n_prompt * 2 + 64).min(N_CTX as usize - n_prompt);

        let mut batch = LlamaBatch::new(n_prompt.max(512), 1);
        let last = (n_prompt - 1) as i32;
        for (i, tok) in (0i32..).zip(tokens) {
            batch
                .add(tok, i, &[0], i == last)
                .map_err(|e| map("Batch", &e))?;
        }
        ctx.decode(&mut batch)
            .map_err(|e| map("Prompt-Decode", &e))?;

        let mut sampler = LlamaSampler::chain_simple([LlamaSampler::greedy()]);
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut out = String::new();
        let mut n_cur = batch.n_tokens();
        let start = Instant::now();

        for _ in 0..max_new {
            if start.elapsed() > CLEAN_TIMEOUT {
                return Err(TalkerError::Cleanup(format!(
                    "Timeout nach {}s",
                    CLEAN_TIMEOUT.as_secs()
                )));
            }
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);
            sampler.accept(token);
            if self.model.is_eog_token(token) {
                break;
            }
            let piece = self
                .model
                .token_to_piece(token, &mut decoder, true, None)
                .map_err(|e| map("Token-Decode", &e))?;
            out.push_str(&piece);
            // Manche GGUFs markieren <end_of_turn> nicht als EOG-Token und
            // emittieren den Tag ggf. in Teilstücken — im Akkumulat prüfen.
            if truncate_at_end_marker(&mut out) {
                break;
            }
            batch.clear();
            batch
                .add(token, n_cur, &[0], true)
                .map_err(|e| map("Batch", &e))?;
            n_cur += 1;
            ctx.decode(&mut batch).map_err(|e| map("Decode", &e))?;
        }

        Ok(out.trim().to_string())
    }
}

/// Turn-Marker-Präfixe, an denen die Generierung endet. Bewusst kurze
/// *Präfixe*: Gemma emittiert die Marker gelegentlich als Text und verstümmelt
/// sie dabei (`<end_of_of_turn>`, `<end{end_of_turn>` — Eval-0001). Diktierter
/// Text enthält nie `<`, daher sind die kurzen Präfixe gefahrlos.
const STOP_MARKERS: [&str; 3] = ["<end", "<start", "<<<"];

/// Schneidet ab dem ersten Turn-Marker ab. `true`, wenn einer gefunden wurde.
fn truncate_at_end_marker(out: &mut String) -> bool {
    if let Some(pos) = STOP_MARKERS.iter().filter_map(|m| out.find(m)).min() {
        out.truncate(pos);
        return true;
    }
    false
}

/// Pipeline-Isolation: Konsumenten hängen nur am Trait, nicht an Gemma.
#[cfg(test)]
pub(crate) struct FakeCleaner {
    pub(crate) result: Result<String>,
}

#[cfg(test)]
impl LlmCleaner for FakeCleaner {
    fn clean(&mut self, _raw: &str) -> Result<String> {
        match &self.result {
            Ok(s) => Ok(s.clone()),
            Err(e) => Err(TalkerError::Cleanup(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trait_is_object_safe_and_swappable() {
        let mut c: Box<dyn LlmCleaner> = Box::new(FakeCleaner {
            result: Ok("Sauberer Text.".into()),
        });
        assert_eq!(c.clean("ähm sauberer text").unwrap(), "Sauberer Text.");
    }

    #[test]
    fn fallback_on_error_returns_raw_unchanged_and_reports_it() {
        // Gilt für jeden Nicht-Roh-Modus: LLM-Fehler → Rohtext unverändert.
        for mode in CleanupMode::ALL.into_iter().filter(|m| m.uses_llm()) {
            let mut c = FakeCleaner {
                result: Err(TalkerError::Cleanup(format!("Timeout in {mode:?}"))),
            };
            let raw = "ähm also das original bleibt";
            assert_eq!(
                clean_with_fallback(&mut c, raw),
                (raw.to_string(), true),
                "{mode:?}"
            );
        }
    }

    #[test]
    fn fallback_on_empty_result_returns_raw_unchanged_and_reports_it() {
        let mut c = FakeCleaner {
            result: Ok(String::new()),
        };
        let raw = "das original";
        assert_eq!(clean_with_fallback(&mut c, raw), (raw.to_string(), true));
    }

    #[test]
    fn success_returns_cleaned_text_without_fallback_flag() {
        let mut c = FakeCleaner {
            result: Ok("Bereinigt.".into()),
        };
        assert_eq!(
            clean_with_fallback(&mut c, "ähm bereinigt"),
            ("Bereinigt.".to_string(), false)
        );
    }

    #[test]
    fn raw_mode_has_no_profile_and_builds_no_prompt() {
        assert_eq!(mode_instruction(CleanupMode::Raw), None);
        assert_eq!(build_prompt(CleanupMode::Raw, "text", &[]), None);
        assert!(!CleanupMode::Raw.uses_llm());
    }

    #[test]
    fn each_llm_mode_has_a_distinct_profile_with_examples() {
        let mut instructions = Vec::new();
        for mode in CleanupMode::ALL {
            if !mode.uses_llm() {
                continue;
            }
            let instr = mode_instruction(mode).expect("LLM-Modus braucht Instruktion");
            assert!(instr.contains("<<<"), "{mode:?}: Delimiter fehlt");
            assert!(
                instr.contains("ohne Nachdenken"),
                "{mode:?}: Thinking-Guard fehlt"
            );
            assert!(
                !mode_examples(mode).is_empty(),
                "{mode:?}: Few-Shots fehlen"
            );
            instructions.push(instr);
        }
        instructions.sort_unstable();
        instructions.dedup();
        assert_eq!(instructions.len(), 3, "Profile müssen sich unterscheiden");
    }

    #[test]
    fn build_prompt_embeds_examples_and_ends_with_model_turn() {
        for mode in [
            CleanupMode::Business,
            CleanupMode::Casual,
            CleanupMode::LlmOptimized,
        ] {
            let p = build_prompt(mode, "mein diktat", &[]).unwrap();
            assert!(p.contains("<<<mein diktat>>>"), "{mode:?}");
            assert!(p.ends_with("<start_of_turn>model\n"), "{mode:?}");
            let (raw_ex, cleaned_ex) = mode_examples(mode)[0];
            assert!(
                p.contains(raw_ex) && p.contains(cleaned_ex),
                "{mode:?}: Few-Shot fehlt"
            );
            // Turn-Struktur konsistent: gleich viele user- und model-Turns.
            assert_eq!(
                p.matches("<start_of_turn>user").count(),
                p.matches("<start_of_turn>model").count(),
                "{mode:?}: Turns unbalanciert"
            );
        }
    }

    #[test]
    fn vocabulary_is_injected_into_llm_modes_only_when_nonempty() {
        let vocab = vec!["Claude CLI".to_string(), "egui".to_string()];
        for mode in CleanupMode::ALL {
            match build_prompt(mode, "diktat", &vocab) {
                Some(p) => {
                    assert!(p.contains("Claude CLI") && p.contains("egui"), "{mode:?}");
                    assert!(p.contains("EXAKT diese Schreibweise"), "{mode:?}");
                }
                None => assert_eq!(mode, CleanupMode::Raw),
            }
        }
        // Leeres Vokabular → kein Block (Status quo).
        let p = build_prompt(CleanupMode::Business, "diktat", &[]).unwrap();
        assert!(!p.contains("Schreibweise"));
    }

    #[test]
    fn mode_labels_are_the_four_agreed_names() {
        let labels: Vec<_> = CleanupMode::ALL.iter().map(|m| m.label()).collect();
        assert_eq!(
            labels,
            [
                "Roh",
                "Geschäftlich",
                "Natürlich (Stil erhalten)",
                "LLM-optimiert"
            ]
        );
    }

    #[test]
    fn end_marker_is_truncated_even_when_split_across_pieces() {
        // Tag am Stück
        let mut s = "Sauberer Text.\n<end_of_turn>".to_string();
        assert!(truncate_at_end_marker(&mut s));
        assert_eq!(s.trim(), "Sauberer Text.");

        // Tag stückweise akkumuliert: greift, sobald das Präfix <end steht
        let mut s = String::new();
        for piece in ["Text", "\n", "<", "en"] {
            s.push_str(piece);
            assert!(!truncate_at_end_marker(&mut s), "zu früh bei {s:?}");
        }
        s.push('d');
        assert!(truncate_at_end_marker(&mut s));
        assert_eq!(s.trim(), "Text");

        // Verstümmelte Variante mit geschweifter Klammer (Eval-0001, Fall B).
        let mut s = "Budget sind Euro.<end{end_of_turn>".to_string();
        assert!(truncate_at_end_marker(&mut s));
        assert_eq!(s.trim(), "Budget sind Euro.");

        // Kein Tag → unverändert
        let mut s = "Nur Text".to_string();
        assert!(!truncate_at_end_marker(&mut s));
        assert_eq!(s, "Nur Text");

        // Erfundener Folge-Turn wird abgeschnitten (frühester Marker gewinnt).
        let mut s = "Bereinigt.\n<start_of_turn>user\n<<<mehr>>>".to_string();
        assert!(truncate_at_end_marker(&mut s));
        assert_eq!(s.trim(), "Bereinigt.");

        // Verstümmelter Marker (Gemma emittiert ihn als Text): Präfix greift.
        let mut s = "Cloud Clip.<end_of_of_turn>".to_string();
        assert!(truncate_at_end_marker(&mut s));
        assert_eq!(s.trim(), "Cloud Clip.");
    }

    #[test]
    fn missing_model_is_a_clear_error_not_a_crash() {
        let Err(err) = GemmaCleaner::new(Path::new("/nonexistent/model.gguf")) else {
            panic!("erwartet: Fehler bei fehlendem Modell");
        };
        assert!(err.to_string().contains("Modell-Datei fehlt"));
    }
}
