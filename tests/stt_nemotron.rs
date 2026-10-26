//! Integrationstest: Nemotron-Impl gegen fixe Audio-Samples (de + en).
//! Benötigt das Modell unter dem Default-Pfad (siehe stt::NemotronTranscriber).

use std::time::Instant;

use talker::stt::{NemotronTranscriber, StreamingTranscriber, Transcriber};

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

fn transcriber() -> NemotronTranscriber {
    NemotronTranscriber::new(&NemotronTranscriber::default_model_dir())
        .expect("Nemotron-Modell fehlt — siehe Fehlermeldung für Download-Hinweis")
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

/// 560-ms-Chunks wie im Live-Betrieb einspeisen.
fn stream(t: &mut NemotronTranscriber, pcm: &[f32]) -> (Vec<talker::stt::StreamUpdate>, String) {
    t.begin();
    let updates = pcm
        .chunks(8_960)
        .map(|c| t.feed(c).unwrap())
        .collect::<Vec<_>>();
    (updates, t.finish().unwrap())
}

/// REQ-STREAM-004/006: Ohne Pause kein Endpoint; der ganze Text kommt aus `finish`.
#[test]
fn streaming_without_pause_delivers_the_text_at_finish() {
    let pcm = read_wav_16k_mono("tests/fixtures/de_test.wav");
    let mut t = transcriber();
    let (updates, rest) = stream(&mut t, &pcm);
    let text = format!(
        "{} {rest}",
        updates
            .iter()
            .filter(|u| u.endpoint)
            .map(|u| u.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    )
    .to_lowercase();
    println!("stream de: {text:?}");
    for expected in ["test", "spracherkennung", "rechner"] {
        assert!(text.contains(expected), "»{expected}« fehlt in: {text:?}");
    }
}

/// REQ-STREAM-004: zwei Sätze mit 1,5 s Pause → ein Endpoint mit dem ersten Satz.
#[test]
fn streaming_pause_ends_the_segment_with_an_endpoint() {
    let sentence = read_wav_16k_mono("tests/fixtures/de_test.wav");
    let mut pcm = sentence.clone();
    pcm.extend(std::iter::repeat_n(0.0, 24_000));
    pcm.extend_from_slice(&sentence);
    let mut t = transcriber();
    let (updates, rest) = stream(&mut t, &pcm);
    let endpoints: Vec<_> = updates.iter().filter(|u| u.endpoint).collect();
    println!("endpoints: {endpoints:?}, rest: {rest:?}");
    assert_eq!(endpoints.len(), 1, "genau ein Endpoint erwartet");
    assert!(endpoints[0].text.to_lowercase().contains("spracherkennung"));
    // Auch nach dem Reset darf der Satzanfang nicht verloren gehen.
    assert!(
        rest.to_lowercase().contains("dies ist ein test"),
        "Satzanfang fehlt: {rest:?}"
    );
}

/// Grenzfälle: leeres Audio und Stille liefern keinen Text und keinen Endpoint.
#[test]
fn streaming_silence_and_empty_input_give_nothing() {
    let mut t = transcriber();
    let (updates, rest) = stream(&mut t, &[]);
    assert!(updates.is_empty() && rest.is_empty());
    let (updates, rest) = stream(&mut t, &vec![0.0; 32_000]);
    assert!(updates.iter().all(|u| !u.endpoint && u.text.is_empty()));
    assert!(rest.is_empty());
}
