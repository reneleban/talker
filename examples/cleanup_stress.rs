//! Stress-/Qualitäts-Testreihe für den gemma4:e2b-Cleanup.
//! Läuft gegen das echte Modell; Ausgabe ist Markdown.
//! Aufruf: cargo run --release --example cleanup_stress

use std::time::Instant;

use talker::cleanup::{CleanupMode, GemmaCleaner, LlmCleaner};

/// Satz-Pool im Diktat-Stil; {K} wird durch Kontroll-Keywords ersetzt.
const SENTENCES: [&str; 8] = [
    "wir müssen den {K} bis ende der woche fertig machen",
    "der kunde hat gesagt dass das {K} projekt oberste priorität hat",
    "ich schicke dir nachher die unterlagen zum {K} rüber",
    "beim letzten meeting haben wir über {K} gesprochen und nichts entschieden",
    "die {K} läuft seit dienstag stabil auf dem neuen server",
    "kannst du bitte prüfen ob der {K} noch aktuell ist",
    "das budget für {K} liegt bei zwanzigtausend euro",
    "wir verschieben den termin für {K} auf nächsten montag",
];
const KEYWORDS: [&str; 8] = [
    "Quartalsbericht",
    "Datenbankmigration",
    "Angebotsentwurf",
    "Personalplanung",
    "Buildpipeline",
    "Wartungsvertrag",
    "Serverumzug",
    "Kundenworkshop",
];
const FILLERS: [&str; 4] = ["ähm", "äh", "ähm also", "äh ja"];

/// Deterministischer Diktat-Text mit ~n Wörtern; alle 8 Keywords rotieren durch.
fn dictation_text(target_words: usize) -> String {
    let mut words = 0;
    let mut out = String::new();
    let mut i = 0;
    while words < target_words {
        let sentence = SENTENCES[i % SENTENCES.len()].replace("{K}", KEYWORDS[i % KEYWORDS.len()]);
        out.push_str(FILLERS[i % FILLERS.len()]);
        out.push(' ');
        out.push_str(&sentence);
        out.push(' ');
        words = out.split_whitespace().count();
        i += 1;
    }
    out.trim().to_string()
}

fn count_fillers(text: &str) -> usize {
    let lower = text.to_lowercase();
    lower
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| *w == "ähm" || *w == "äh")
        .count()
}

/// Anteil der im Input vorkommenden Keywords, die im Output erhalten sind.
fn keyword_retention(input: &str, output: &str) -> (usize, usize) {
    let out_lower = output.to_lowercase();
    let present: Vec<_> = KEYWORDS
        .iter()
        .filter(|k| input.to_lowercase().contains(&k.to_lowercase()))
        .collect();
    let kept = present
        .iter()
        .filter(|k| out_lower.contains(&k.to_lowercase()))
        .count();
    (kept, present.len())
}

fn has_marker_leak(text: &str) -> bool {
    ["<end_of", "<start_of", "<<<", ">>>"]
        .iter()
        .any(|m| text.contains(m))
}

fn main() {
    let mut cleaner =
        GemmaCleaner::new(&GemmaCleaner::default_model_path()).expect("gemma4:e2b-GGUF fehlt");

    // ── A) Längen-Reihe (Modus Geschäftlich) ────────────────────────────────
    println!("## A) Längen-Reihe (Geschäftlich)\n");
    println!("| Wörter in | Latenz | Wörter out | Ratio | Füllw. in→out | Keywords | Leak |");
    println!("|---|---|---|---|---|---|---|");
    cleaner.set_mode(CleanupMode::Business);
    for target in [10, 50, 100, 200, 400, 800, 1200, 1600] {
        let input = dictation_text(target);
        let n_in = input.split_whitespace().count();
        let f_in = count_fillers(&input);
        let t0 = Instant::now();
        match cleaner.clean(&input) {
            Ok(out) => {
                let ms = t0.elapsed().as_millis();
                let n_out = out.split_whitespace().count();
                let (kept, total) = keyword_retention(&input, &out);
                println!(
                    "| {n_in} | {ms} ms | {n_out} | {:.2} | {f_in}→{} | {kept}/{total} | {} |",
                    n_out as f64 / n_in as f64,
                    count_fillers(&out),
                    if has_marker_leak(&out) { "JA" } else { "–" },
                );
            }
            Err(e) => println!("| {n_in} | FEHLER: {e} | – | – | – | – | – |"),
        }
    }

    // ── B) Robustheits-Fälle (Geschäftlich) ─────────────────────────────────
    println!("\n## B) Robustheit (Geschäftlich)\n");
    let cases: [(&str, &str); 6] = [
        ("leer", ""),
        ("ein Wort", "test"),
        (
            "Zahlen/Datum",
            "ähm der termin ist am dritten märz um vierzehn uhr dreißig und das budget sind zwölftausendfünfhundert euro",
        ),
        (
            "de/en-Mix",
            "ähm wir sollten das feature flag für den dark mode äh erst nach dem code review mergen",
        ),
        (
            "Frage",
            "ähm hast du schon mit dem kunden über den wartungsvertrag gesprochen",
        ),
        (
            "Selbstkorrektur",
            "das meeting ist am äh nee warte am donnerstag nicht am mittwoch",
        ),
    ];
    for (name, input) in cases {
        let t0 = Instant::now();
        match cleaner.clean(input) {
            Ok(out) => println!(
                "- **{name}** ({} ms): `{input}` → `{out}`",
                t0.elapsed().as_millis()
            ),
            Err(e) => println!("- **{name}**: FEHLER {e}"),
        }
    }

    // ── C) Modi-Vergleich (A/B-Grundlage) ───────────────────────────────────
    println!("\n## C) Modi-Vergleich\n");
    let probes: [(&str, &str); 3] = [
        (
            "Büro-Kurznachricht",
            "ähm also ich schaff das heute nicht mehr äh können wir das morgen früh besprechen so um neun oder so",
        ),
        (
            "Lockeres Rambling",
            "boah äh das neue café an der ecke ist echt mega die haben so nen krassen käsekuchen ähm den musst du probieren ne",
        ),
        (
            "Coding-Anweisung",
            "ähm also im config modul da fehlt noch validierung äh die breite muss zwischen fünfzehn und achtzig liegen und wenn nicht ähm dann bitte auf default zurückfallen und nen warning loggen",
        ),
    ];
    for (name, input) in probes {
        println!("### {name}\n\nInput: `{input}`\n");
        for mode in CleanupMode::ALL {
            if !mode.uses_llm() {
                println!("- **{}**: (unverändert)", mode.label());
                continue;
            }
            cleaner.set_mode(mode);
            let t0 = Instant::now();
            match cleaner.clean(input) {
                Ok(out) => {
                    println!(
                        "- **{}** ({} ms): {out:?}",
                        mode.label(),
                        t0.elapsed().as_millis()
                    );
                }
                Err(e) => println!("- **{}**: FEHLER {e}", mode.label()),
            }
        }
        println!();
    }
}
