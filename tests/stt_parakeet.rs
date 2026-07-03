//! Integrationstest: Parakeet-Impl gegen fixe Audio-Samples (de + en).
//! Benötigt das Modell unter dem Default-Pfad (siehe stt::ParakeetTranscriber).

use std::time::Instant;

use talker::stt::{ParakeetTranscriber, Transcriber};

fn read_wav_16k_mono(path: &str) -> Vec<f32> {
    let mut reader = hound::WavReader::open(path).expect("Fixture nicht lesbar");
    let spec = reader.spec();
    assert_eq!(spec.sample_rate, 16_000, "Fixture muss 16 kHz sein");
    assert_eq!(spec.channels, 1, "Fixture muss mono sein");
    reader
        .samples::<i16>()
        .map(|s| f32::from(s.expect("Sample")) / 32768.0)
        .collect()
}

fn transcriber() -> ParakeetTranscriber {
    ParakeetTranscriber::new(&ParakeetTranscriber::default_model_dir())
        .expect("Parakeet-Modell fehlt — siehe Fehlermeldung für Download-Hinweis")
}

#[test]
fn transcribes_german_sample() {
    let pcm = read_wav_16k_mono("tests/fixtures/de_test.wav");
    let mut t = transcriber();
    let start = Instant::now();
    let text = t.transcribe(&pcm).unwrap().to_lowercase();
    println!(
        "de: {:.0} ms Audio → STT {} ms → {text:?}",
        pcm.len() as f64 / 16.0,
        start.elapsed().as_millis()
    );
    for expected in ["test", "spracherkennung", "rechner"] {
        assert!(text.contains(expected), "»{expected}« fehlt in: {text:?}");
    }
}

#[test]
fn transcribes_english_sample() {
    let pcm = read_wav_16k_mono("tests/fixtures/en_test.wav");
    let mut t = transcriber();
    let start = Instant::now();
    let text = t.transcribe(&pcm).unwrap().to_lowercase();
    println!(
        "en: {:.0} ms Audio → STT {} ms → {text:?}",
        pcm.len() as f64 / 16.0,
        start.elapsed().as_millis()
    );
    for expected in ["test", "speech recognition", "computer"] {
        assert!(text.contains(expected), "»{expected}« fehlt in: {text:?}");
    }
}
