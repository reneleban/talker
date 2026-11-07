//! FLEURS-Fixtures (AK-25/-26): Vollständigkeit, Format und Attribution.

use std::path::{Path, PathBuf};

fn repo_file(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

/// REQ-DIAG-030: jedes `fleurs-de-*.wav` hat eine Referenz, ist 16 kHz mono, nicht leer, < 30 s.
#[test]
fn fleurs_fixtures_are_complete_16k_mono_clips_with_references() {
    let dir = repo_file("tests/fixtures/stt");
    let mut wavs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("fleurs-de-") && n.ends_with(".wav"))
        })
        .collect();
    wavs.sort();
    assert_eq!(wavs.len(), 10, "ca. 10 Clips: {wavs:?}");
    for wav in wavs {
        let reader = hound::WavReader::open(&wav).unwrap();
        let spec = reader.spec();
        assert_eq!((spec.channels, spec.sample_rate), (1, 16_000), "{wav:?}");
        let seconds = reader.len() as f32 / 16_000.0;
        assert!(seconds > 0.0 && seconds < 30.0, "{wav:?}: {seconds} s");
        let reference = std::fs::read_to_string(wav.with_extension("ref.txt")).unwrap();
        assert!(!reference.trim().is_empty(), "{wav:?}");
        assert!(!reference.ends_with('\n'), "Referenz ohne Zeilenende");
    }
}

/// REQ-DIAG-031: Attribution nennt Urheber, Titel, Quelle, Paper, Lizenz mit Link, Änderungen.
#[test]
fn attribution_names_creator_title_sources_license_and_changes() {
    let text = std::fs::read_to_string(repo_file("tests/fixtures/stt/ATTRIBUTION.md")).unwrap();
    for needle in [
        "Alexis Conneau",
        "Google",
        "FLEURS: Few-shot Learning Evaluation of Universal Representations of Speech",
        "https://huggingface.co/datasets/google/fleurs",
        "https://arxiv.org/abs/2205.12446",
        "CC-BY-4.0",
        "https://creativecommons.org/licenses/by/4.0/",
        "Änderungen",
    ] {
        assert!(text.contains(needle), "ATTRIBUTION.md ohne: {needle}");
    }
    let clip_ids = text.matches("fleurs-de-").count();
    assert!(clip_ids >= 10);
}

/// REQ-DIAG-031: die Lizenz-Doku weist die Fixtures als nicht-MIT aus.
#[test]
fn licensing_doc_marks_the_fleurs_fixtures_as_not_mit() {
    let text = std::fs::read_to_string(repo_file("docs/licensing.md")).unwrap();
    let section = text
        .split("## Testdaten: FLEURS-Fixtures, NICHT MIT")
        .nth(1)
        .expect("Abschnitt zu den FLEURS-Fixtures fehlt");
    assert!(section.contains("CC-BY-4.0") && section.contains("ATTRIBUTION.md"));
    assert!(section.contains("nicht unter der MIT-Lizenz"));
}

/// SEC-5: die eingecheckte Prüfsummenliste deckt genau die 20 FLEURS-Dateien ab und stimmt mit
/// ihnen überein; das Fetch-Skript prüft nach dem Download gegen dieselbe Liste.
#[test]
fn checksum_list_matches_the_committed_fleurs_files() {
    let dir = repo_file("tests/fixtures/stt");
    let list = std::fs::read_to_string(dir.join("fleurs-de.sha256")).unwrap();
    let mut listed = Vec::new();
    for line in list.lines() {
        let (hash, name) = line.split_once("  ").expect("Format `<sha256>  <datei>`");
        let actual = talker::models::sha256_file(&dir.join(name)).unwrap();
        assert_eq!(hash, actual, "{name}");
        listed.push(name.to_string());
    }
    let mut expected: Vec<String> = (1..=10)
        .flat_map(|n| {
            [
                format!("fleurs-de-{n:02}.wav"),
                format!("fleurs-de-{n:02}.ref.txt"),
            ]
        })
        .collect();
    expected.sort();
    listed.sort();
    assert_eq!(listed, expected);
}
