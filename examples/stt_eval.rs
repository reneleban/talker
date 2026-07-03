//! STT-Qualitäts-Testreihe für Parakeet.
//! Liest tests/fixtures/stt/*.wav + *.ref.txt, misst WER, Problembegriff-
//! Treffer und Latenz. Ausgabe: Markdown. Aufruf: make stt-eval

use std::path::Path;
use std::time::Instant;

use talker::stt::{ParakeetTranscriber, Transcriber};

/// Begriffe, deren Erkennung uns besonders interessiert (de/en-Mix-Schmerz),
/// aus tests/fixtures/stt/terms.txt — dieselbe Liste, die später das
/// Custom-Vokabular speist.
fn problem_terms() -> Vec<String> {
    std::fs::read_to_string("tests/fixtures/stt/terms.txt")
        .expect("tests/fixtures/stt/terms.txt fehlt")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

fn normalize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// Wort-Editierdistanz (Levenshtein) → WER = Distanz / Referenzlänge.
fn wer(reference: &str, hypothesis: &str) -> f64 {
    let r = normalize(reference);
    let h = normalize(hypothesis);
    if r.is_empty() {
        return if h.is_empty() { 0.0 } else { 1.0 };
    }
    let mut prev: Vec<usize> = (0..=h.len()).collect();
    let mut curr = vec![0; h.len() + 1];
    for (i, rw) in r.iter().enumerate() {
        curr[0] = i + 1;
        for (j, hw) in h.iter().enumerate() {
            let sub = prev[j] + usize::from(rw != hw);
            curr[j + 1] = sub.min(prev[j + 1] + 1).min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[h.len()] as f64 / r.len() as f64
}

fn read_wav_16k_mono(path: &Path) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).expect("WAV nicht lesbar");
    assert_eq!(reader.spec().sample_rate, 16_000, "{path:?}: 16 kHz nötig");
    assert_eq!(reader.spec().channels, 1, "{path:?}: mono nötig");
    reader
        .samples::<i16>()
        .map(|s| f32::from(s.expect("Sample")) / 32768.0)
        .collect()
}

fn main() {
    let dir = Path::new("tests/fixtures/stt");
    let mut cases: Vec<_> = std::fs::read_dir(dir)
        .expect("tests/fixtures/stt fehlt — scripts/gen_stt_fixtures.sh ausführen")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "wav"))
        .collect();
    cases.sort();
    assert!(!cases.is_empty(), "keine WAV-Fixtures gefunden");

    let mut stt = ParakeetTranscriber::new(&ParakeetTranscriber::default_model_dir())
        .expect("Parakeet-Modell fehlt");

    println!("| Fall | Audio | Latenz | WER | Hypothese |");
    println!("|---|---|---|---|---|");
    let mut wers = Vec::new();
    let mut term_hits: Vec<(String, bool, String)> = Vec::new();

    for wav in &cases {
        let name = wav.file_stem().unwrap().to_string_lossy().to_string();
        let reference = std::fs::read_to_string(wav.with_extension("ref.txt"))
            .unwrap_or_else(|_| panic!("{name}: .ref.txt fehlt"));
        let pcm = read_wav_16k_mono(wav);
        let secs = pcm.len() as f64 / 16_000.0;
        let t0 = Instant::now();
        let hyp = stt.transcribe(&pcm).expect("Transkription fehlgeschlagen");
        let ms = t0.elapsed().as_millis();
        let w = wer(&reference, &hyp);
        wers.push(w);
        // Format-agnostischer Begriffs-Match: „NPM-Install" zählt für „npm install"
        // (nur Alphanumerik, keine Trennzeichen) — sonst zählt Formatierung als Fehler.
        let squash = |s: &str| {
            s.to_lowercase()
                .chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
        };
        let (ref_sq, hyp_sq) = (squash(&reference), squash(&hyp));
        for term in problem_terms() {
            let term_sq = squash(&term);
            if ref_sq.contains(&term_sq) {
                term_hits.push((term.clone(), hyp_sq.contains(&term_sq), name.clone()));
            }
        }
        println!(
            "| {name} | {secs:.1} s | {ms} ms | {:.0} % | {hyp:?} |",
            w * 100.0
        );
    }

    let mean = wers.iter().sum::<f64>() / wers.len() as f64;
    println!(
        "\n**Mittlere WER:** {:.1} % über {} Fälle\n",
        mean * 100.0,
        wers.len()
    );

    println!("| Problembegriff | erkannt | Fall |");
    println!("|---|---|---|");
    for (term, hit, case) in &term_hits {
        println!("| {term} | {} | {case} |", if *hit { "✓" } else { "✗" });
    }
    let hits = term_hits.iter().filter(|(_, h, _)| *h).count();
    println!("\n**Begriff-Trefferquote:** {hits}/{}", term_hits.len());
}
