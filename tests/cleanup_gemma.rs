//! Integrationstest: GemmaCleaner gegen das echte gemma4:e2b-GGUF.
//! Benötigt das Modell unter dem Default-Pfad (siehe cleanup::GemmaCleaner).

use std::time::Instant;

use talker::cleanup::{CleanupMode, GemmaCleaner, LlmCleaner};

// Ein Testfall für beide Aspekte: llama.cpp-Backend darf pro Prozess nur
// einmal initialisiert werden, zwei #[test]-Fns würden kollidieren.
#[test]
fn cleans_filler_words_without_thinking_leak_and_passes_empty_through() {
    let mut cleaner = GemmaCleaner::new(&GemmaCleaner::default_model_path())
        .expect("gemma4:e2b-GGUF fehlt — siehe Fehlermeldung für Ablageort");

    // Grenzfall: leerer/Whitespace-Raw geht unverändert durch, ohne LLM-Lauf.
    assert_eq!(cleaner.clean("").unwrap(), "");
    assert_eq!(cleaner.clean("   ").unwrap(), "   ");

    // Grenzfall: Überlanges Transkript → sauberer Fehler (Raw-Fallback greift),
    // KEIN llama.cpp-C-Abort (Eval-0001: GGML_ASSERT bei ~1600 Wörtern).
    let huge = "wort ".repeat(4000);
    let err = cleaner
        .clean(&huge)
        .expect_err("muss Fehler liefern statt crashen");
    assert!(err.to_string().contains("zu lang"), "{err}");

    let raw = "ähm also ich wollte nochmal sagen dass wir ähm das meeting am donnerstag \
               auf freitag verschieben sollten weil äh der kunde da erst zeit hat";
    let start = Instant::now();
    let cleaned = cleaner.clean(raw).unwrap();
    println!("Cleanup {} ms → {cleaned:?}", start.elapsed().as_millis());

    // Sichtbare Bereinigung: Füllwörter raus, Interpunktion/Großschreibung rein.
    let lower = cleaned.to_lowercase();
    assert!(
        !lower.contains("ähm"),
        "Füllwort »ähm« nicht entfernt: {cleaned:?}"
    );
    assert!(
        !lower.contains(" äh "),
        "Füllwort »äh« nicht entfernt: {cleaned:?}"
    );
    assert!(lower.contains("meeting"), "Inhalt verloren: {cleaned:?}");
    assert!(lower.contains("freitag"), "Inhalt verloren: {cleaned:?}");
    assert!(
        cleaned.chars().next().is_some_and(char::is_uppercase),
        "Satzanfang nicht großgeschrieben: {cleaned:?}"
    );

    // Kein Thinking-Leak (Spike-0001: Modell denkt per Default).
    for marker in [
        "Thinking",
        "thinking",
        "<think",
        "Process:",
        "Analyze",
        "<end_of_turn>",
    ] {
        assert!(
            !cleaned.contains(marker),
            "Thinking-Leak »{marker}«: {cleaned:?}"
        );
    }
    // Bereinigt ≈ Rohlänge — ein Vielfaches wäre ein Leak/Halluzination.
    assert!(
        cleaned.len() < raw.len() * 2,
        "Output verdächtig lang ({} vs {} Zeichen): {cleaned:?}",
        cleaned.len(),
        raw.len()
    );

    // Modus »LLM-optimiert«: diktiertes Rambling → copy-paste-fertiger Prompt.
    cleaner.set_mode(CleanupMode::LlmOptimized);
    let dictated = "ähm also mach mal bitte dass äh der login button auch mit enter \
                    funktioniert und ähm ach ja der fehlertext soll rot sein glaub ich";
    let start = Instant::now();
    let prompt_out = cleaner.clean(dictated).unwrap();
    println!(
        "llm-Modus {} ms → {prompt_out:?}",
        start.elapsed().as_millis()
    );
    let pl = prompt_out.to_lowercase();
    assert!(!pl.contains("ähm"), "Füllsel im Prompt: {prompt_out:?}");
    assert!(
        pl.contains("login") && pl.contains("enter"),
        "Detail verloren: {prompt_out:?}"
    );
    assert!(pl.contains("rot"), "Detail verloren: {prompt_out:?}");
    for marker in ["<end_of", "<start_of", "<<<"] {
        assert!(!prompt_out.contains(marker), "Marker-Leak: {prompt_out:?}");
    }
    cleaner.set_mode(CleanupMode::Business);

    // Vokabular korrigiert auch stark eingedeutschte Verhörer (Field-Test 0012:
    // »Tommel«/»Clotzelei«). Produktionsfluss: erst phonetisches Matching
    // (vocab_match, deterministisch), dann LLM mit Vokabular-Block.
    let vocab: Vec<String> = ["Claude CLI".into(), "TOML".into(), "Kubernetes".into()].into();
    cleaner.set_vocab(&vocab);
    let raw = "ähm die config liegt als tommel im repo und die clotzelei ist offen";
    let matched = talker::vocab_match::apply(raw, &vocab);
    let v = cleaner.clean(&matched).unwrap();
    println!("vocab → {v:?}");
    assert!(v.contains("TOML"), "»tommel« nicht korrigiert: {v:?}");
    assert!(
        v.contains("Claude CLI"),
        "»clotzelei« nicht korrigiert: {v:?}"
    );
    cleaner.set_vocab(&[]);

    // Regression (EVAL-0003): englisches Diktat darf NICHT übersetzt werden.
    let en = cleaner
        .clean("uhm we should merge the feature branch after the code review")
        .unwrap();
    println!("en → {en:?}");
    let enl = en.to_lowercase();
    assert!(
        enl.contains("feature branch") && enl.contains("merge"),
        "Inhalt verloren: {en:?}"
    );
    assert!(
        enl.contains("we ") || enl.starts_with("we"),
        "mutmaßlich übersetzt statt bereinigt: {en:?}"
    );

    // Regression: Gemma beantwortete kurze Fragen, statt sie zu bereinigen.
    let q = cleaner
        .clean("ähm wo stehen wir denn gerade im projekt")
        .unwrap();
    println!("Frage → {q:?}");
    let ql = q.to_lowercase();
    assert!(ql.contains("wo stehen wir"), "Frage nicht erhalten: {q:?}");
    assert!(q.contains('?'), "Fragezeichen fehlt: {q:?}");
    for answer_word in ["phase", "implementier", "mitte", "wir sind"] {
        assert!(
            !ql.contains(answer_word),
            "Frage wurde beantwortet statt bereinigt (»{answer_word}«): {q:?}"
        );
    }
}
