//! A/B/C-Vergleich der Pipelines auf den STT-Fixtures:
//!   A: nur Parakeet (Raw Transcript)
//!   B: Parakeet + Gemma-Cleanup (Geschäftlich; Anweisungs-Fälle zusätzlich LLM-Modus)
//!   C: nur Gemma (Audio direkt via llama-mtmd-cli, Thinking nicht abschaltbar)
//! Aufruf: make pipeline-ab

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use talker::cleanup::{CleanupMode, GemmaCleaner, LlmCleaner};
use talker::stt::{ParakeetTranscriber, Transcriber};

fn normalize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

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

fn squash(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn read_wav_16k_mono(path: &Path) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).expect("WAV nicht lesbar");
    reader
        .samples::<i16>()
        .map(|s| f32::from(s.expect("Sample")) / 32768.0)
        .collect()
}

/// mmproj-Datei im HF-Cache suchen (vom Spike-Download).
fn find_mmproj() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let snaps = PathBuf::from(home)
        .join(".cache/huggingface/hub/models--ggml-org--gemma-4-E2B-it-GGUF/snapshots");
    for snap in std::fs::read_dir(snaps).ok()? {
        let dir = snap.ok()?.path();
        for f in std::fs::read_dir(&dir).ok()? {
            let p = f.ok()?.path();
            if p.file_name()?.to_string_lossy().starts_with("mmproj") {
                return Some(p);
            }
        }
    }
    None
}

/// Pipeline C: Audio direkt in gemma4:e2b (llama-mtmd-cli). Das Modell denkt
/// unabschaltbar; das Transkript ist die letzte nicht-leere Zeile.
fn gemma_audio_transcribe(wav: &Path, mmproj: &Path) -> Option<(String, u128)> {
    let model = GemmaCleaner::default_model_path();
    let t0 = Instant::now();
    let out = Command::new("/opt/homebrew/bin/llama-mtmd-cli")
        .args(["-m"]).arg(&model)
        .arg("--mmproj").arg(mmproj)
        .arg("--jinja")
        .arg("--audio").arg(wav)
        .args(["-p", "Transkribiere das Audio wortwörtlich auf Deutsch. Gib nur den transkribierten Text aus, keine Erklärung.", "-n", "512"])
        .output()
        .ok()?;
    let ms = t0.elapsed().as_millis();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.lines().rev().find(|l| !l.trim().is_empty())?;
    // Thinking-Reste vor einem Separator abschneiden.
    let text = line.rsplit("|>").next().unwrap_or(line).trim().to_string();
    Some((text, ms))
}

fn main() {
    let dir = Path::new("tests/fixtures/stt");
    let mut cases: Vec<_> = std::fs::read_dir(dir)
        .expect("Fixtures fehlen")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "wav"))
        .collect();
    cases.sort();

    let terms: Vec<String> = std::fs::read_to_string(dir.join("terms.txt"))
        .expect("terms.txt")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();

    let mut stt = ParakeetTranscriber::new(&ParakeetTranscriber::default_model_dir())
        .expect("Parakeet fehlt");
    let mut cleaner = GemmaCleaner::new(&GemmaCleaner::default_model_path()).expect("Gemma fehlt");
    cleaner.set_mode(CleanupMode::Business);
    // terms.txt = Nutzer-Vokabular (Ticket-0012): misst den Dictionary-Effekt.
    cleaner.set_vocab(&terms);
    let mmproj = find_mmproj();
    if mmproj.is_none() {
        println!("> Hinweis: mmproj nicht gefunden — Pipeline C wird übersprungen.\n");
    }

    println!(
        "| Fall | WER A (Parakeet) | WER B (P+Gemma) | WER C (Gemma-Audio) | Lat. A | Lat. B | Lat. C* |"
    );
    println!("|---|---|---|---|---|---|---|");
    let mut sums = [(0.0, 0usize), (0.0, 0), (0.0, 0)];
    let mut hits = [0usize, 0, 0];
    let mut totals = [0usize, 0, 0];
    let mut anweisungen: Vec<(String, String)> = Vec::new();

    for wav in &cases {
        let name = wav.file_stem().unwrap().to_string_lossy().to_string();
        let reference = std::fs::read_to_string(wav.with_extension("ref.txt")).unwrap();
        let pcm = read_wav_16k_mono(wav);

        let t0 = Instant::now();
        let raw = stt.transcribe(&pcm).unwrap();
        let ms_a = t0.elapsed().as_millis();

        let t1 = Instant::now();
        // Wie im Worker: erst deterministisches Vokabular-Matching, dann LLM.
        let matched = talker::vocab_match::apply(&raw, &terms);
        let cleaned = cleaner.clean(&matched).unwrap_or_else(|_| matched.clone());
        let ms_b = ms_a + t1.elapsed().as_millis();

        let c = mmproj
            .as_deref()
            .and_then(|mp| gemma_audio_transcribe(wav, mp));

        let wa = wer(&reference, &raw);
        let wb = wer(&reference, &cleaned);
        sums[0].0 += wa;
        sums[0].1 += 1;
        sums[1].0 += wb;
        sums[1].1 += 1;
        let (wc_str, lc_str) = match &c {
            Some((text, ms)) => {
                let wc = wer(&reference, text);
                sums[2].0 += wc;
                sums[2].1 += 1;
                let (ref_sq, c_sq) = (squash(&reference), squash(text));
                for term in &terms {
                    if ref_sq.contains(&squash(term)) {
                        totals[2] += 1;
                        hits[2] += usize::from(c_sq.contains(&squash(term)));
                    }
                }
                (format!("{:.0} %", wc * 100.0), format!("{ms} ms"))
            }
            None => ("–".into(), "–".into()),
        };
        let ref_sq = squash(&reference);
        for (i, hyp) in [(0usize, &raw), (1, &cleaned)] {
            let sq = squash(hyp);
            for term in &terms {
                if ref_sq.contains(&squash(term)) {
                    totals[i] += 1;
                    hits[i] += usize::from(sq.contains(&squash(term)));
                }
            }
        }
        println!(
            "| {name} | {:.0} % | {:.0} % | {wc_str} | {ms_a} ms | {ms_b} ms | {lc_str} |",
            wa * 100.0,
            wb * 100.0
        );

        // Anweisungs-Fälle zusätzlich durch den LLM-Modus (qualitativ).
        if name.starts_with("dev-anweisung") {
            cleaner.set_mode(CleanupMode::LlmOptimized);
            let prompt = cleaner.clean(&raw).unwrap_or_default();
            cleaner.set_mode(CleanupMode::Business);
            anweisungen.push((name, prompt));
        }
    }

    println!();
    for (label, i) in [("A Parakeet", 0), ("B P+Gemma", 1), ("C Gemma-Audio", 2)] {
        if sums[i].1 > 0 {
            println!(
                "**{label}:** mittlere WER {:.1} %, Begriffe {}/{}",
                sums[i].0 / sums[i].1 as f64 * 100.0,
                hits[i],
                totals[i]
            );
        }
    }
    println!(
        "\n*Lat. C enthält Modell-Load + nicht abschaltbares Thinking (llama-mtmd-cli, Prozess pro Datei).*"
    );

    println!("\n## Anweisungs-Fälle → LLM-optimierter Modus (Pipeline B)\n");
    for (name, prompt) in anweisungen {
        println!("- **{name}**: {prompt:?}");
    }
}
