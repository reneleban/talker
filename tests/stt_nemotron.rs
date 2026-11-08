//! Integrationstest: Nemotron-Impl gegen fixe Audio-Samples (de + en).
//! Benötigt das Modell unter dem Default-Pfad (siehe stt::NemotronTranscriber).

use std::time::Instant;

use talker::config::Language;
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

const DE_WORDS: [&str; 3] = ["test", "spracherkennung", "rechner"];
const EN_WORDS: [&str; 3] = ["test", "speech recognition", "computer"];

fn batch_with(language: Language, fixture: &str) -> String {
    let pcm = read_wav_16k_mono(fixture);
    let mut t = transcriber();
    t.set_language(language);
    t.transcribe(&pcm).unwrap().to_lowercase()
}

fn live_with(language: Language, fixture: &str) -> String {
    let pcm = read_wav_16k_mono(fixture);
    let mut t = transcriber();
    t.set_language(language);
    let (updates, rest) = stream(&mut t, &pcm);
    let finals = updates
        .iter()
        .filter(|u| u.endpoint)
        .map(|u| u.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    format!("{finals} {rest}").to_lowercase()
}

fn assert_contains_all(text: &str, words: [&str; 3]) {
    for expected in words {
        assert!(text.contains(expected), "»{expected}« fehlt in: {text:?}");
    }
}

/// AK-7 / REQ-DICT-024: Sprache je Pfad (Batch + Live) und je Wert.
#[test]
fn set_language_de_recognizes_german_in_batch_and_live() {
    assert_contains_all(
        &batch_with(Language::De, "tests/fixtures/de_test.wav"),
        DE_WORDS,
    );
    assert_contains_all(
        &live_with(Language::De, "tests/fixtures/de_test.wav"),
        DE_WORDS,
    );
}

#[test]
fn set_language_auto_and_en_recognize_english_in_batch_and_live() {
    for language in [Language::Auto, Language::En] {
        assert_contains_all(
            &batch_with(language, "tests/fixtures/en_test.wav"),
            EN_WORDS,
        );
        assert_contains_all(&live_with(language, "tests/fixtures/en_test.wav"), EN_WORDS);
    }
}

/// „Darf nicht passieren“: `De` fällt nicht still auf `auto` zurück – ein anderer Code
/// (`en`) auf deutschem Audio liefert nachweislich einen anderen Text als `de`.
#[test]
fn set_language_value_reaches_the_model_not_silently_auto() {
    let de = batch_with(Language::De, "tests/fixtures/de_test.wav");
    let en = batch_with(Language::En, "tests/fixtures/de_test.wav");
    println!("de: {de:?}\nen: {en:?}");
    assert_ne!(de, en, "Sprachcode wirkt nicht auf das Modell");
}

/// Eine laufende Live-Utterance behält die Sprache ihres `begin`.
#[test]
fn live_utterance_keeps_the_language_of_its_begin() {
    let pcm = read_wav_16k_mono("tests/fixtures/de_test.wav");
    let mut t = transcriber();
    t.set_language(Language::De);
    t.begin();
    t.set_language(Language::En);
    let mut text = String::new();
    for c in pcm.chunks(8_960) {
        let u = t.feed(c).unwrap();
        if u.endpoint {
            text.push_str(&u.text);
            text.push(' ');
        }
    }
    text.push_str(&t.finish().unwrap());
    assert_contains_all(&text.to_lowercase(), DE_WORDS);
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

// ---------------------------------------------------------------------------
// Regression „erstes Wort" (Ticket 0038, REQ-DIAG-032/-033): WAV-Quelle → PttSession →
// process_job mit dem echten Nemotron (Cleanup Roh), ohne Injection. Braucht das Modell.
// ---------------------------------------------------------------------------

#[path = "support/first_word.rs"]
mod first_word_support;

use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use talker::audio::{AudioFormat, HostInstant, Pace, SourceDone, WavOptions, WavSource};
use talker::cleanup::{CleanupMode, GemmaCleaner};
use talker::config::{Config, InsertMode};
use talker::diagnostics::{DictationTrace, TraceReport};
use talker::models::{ModelState, ModelsState};
use talker::pipeline::{
    DictationWorker, MicReady, PressOutcome, PttSession, ReleaseOutcome, SttJob, process_job,
};

use first_word_support::{first_word, normalize_words};

/// Ein Modell im Speicher genügt — die Fälle laufen nacheinander (650 MB je Modell).
static MODEL_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug)]
enum FixtureSet {
    /// Mit `say` erzeugte Fixtures (alles außer `fleurs-*`).
    Say,
    /// FLEURS-Clips (`fleurs-de-*`).
    Fleurs,
}

fn fixtures(set: FixtureSet) -> Vec<(String, PathBuf)> {
    let mut found: Vec<(String, PathBuf)> = std::fs::read_dir("tests/fixtures/stt")
        .expect("tests/fixtures/stt fehlt")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "wav"))
        .filter_map(|p| {
            let name = p.file_stem()?.to_string_lossy().into_owned();
            let is_fleurs = name.starts_with("fleurs-");
            match set {
                FixtureSet::Say => !is_fleurs,
                FixtureSet::Fleurs => is_fleurs,
            }
            .then_some((name, p))
        })
        .collect();
    found.sort();
    assert!(!found.is_empty(), "keine Fixtures für {set:?}");
    found
}

fn reference(wav: &Path) -> String {
    std::fs::read_to_string(wav.with_extension("ref.txt")).expect(".ref.txt fehlt")
}

type NemotronWorker = DictationWorker<NemotronTranscriber, GemmaCleaner>;

fn worker() -> NemotronWorker {
    let mut stt = Some(transcriber());
    DictationWorker::new(
        &Config::default(),
        Arc::new(ModelsState::new(ModelState::Ready, ModelState::Missing)),
        Box::new(move |_| {
            stt.take()
                .ok_or_else(|| talker::error::TalkerError::Stt("nur einmal geladen".into()))
        }),
        Box::new(|| (None, false)),
    )
}

/// Ein Diktat über die App-Pipeline; liefert die eingefügten Stücke und die Messpunkte.
fn dictate(
    worker: &mut NemotronWorker,
    pcm: Vec<f32>,
    start_delay_ms: u64,
    insert_mode: InsertMode,
    language: Language,
) -> (Vec<String>, DictationTrace) {
    let cfg = Config {
        insert_mode,
        language,
        cleanup_mode: CleanupMode::Raw,
        ..Config::default()
    };
    let (done_tx, done_rx) = channel::<SourceDone>();
    let done_tx = Mutex::new(done_tx);
    let report: TraceReport = Arc::new(|_| {});
    let mut session = PttSession::new(
        Box::new(move |_| {
            let (source, done) = WavSource::from_samples(
                pcm.clone(),
                WavOptions {
                    format: AudioFormat {
                        channels: 1,
                        sample_rate: 16_000,
                    },
                    block_frames: 480,
                    start_delay: Duration::from_millis(start_delay_ms),
                    pace: Pace::Immediate,
                    epoch: HostInstant::now(),
                },
            );
            let _ = done_tx.lock().unwrap().send(done);
            Box::new(source)
        }),
        report,
    );
    let (ready_tx, ready_rx) = channel();
    let models = ModelsState::new(ModelState::Ready, ModelState::Missing);
    let outcome = session.press(HostInstant::now(), &cfg, &models, move |ready| {
        let _ = ready_tx.send(ready);
    });
    assert!(matches!(outcome, PressOutcome::Starting));
    let live = match ready_rx.recv_timeout(Duration::from_secs(10)).unwrap() {
        MicReady::Recording { live, .. } => live,
        MicReady::Failed(e) => panic!("Quelle ließ sich nicht öffnen: {e}"),
    };
    assert!(
        done_rx
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .wait(Duration::from_secs(60))
    );
    let mut pieces = Vec::new();
    let mut inject = |text: &str| {
        pieces.push(text.to_string());
        Ok(())
    };
    let job = match session.release(HostInstant::now()) {
        ReleaseOutcome::Ready {
            pcm,
            frontmost,
            captured,
            trace,
            ..
        } => SttJob::Utterance {
            pcm,
            frontmost,
            captured,
            trace,
        },
        ReleaseOutcome::LiveEnded => SttJob::Live {
            feed: live.expect("Live-Aufnahme hat einen Feed"),
        },
        other => panic!("Diktat endete vorzeitig: {other:?}"),
    };
    let (_, trace) = process_job(worker, job, &cfg, &mut inject, None);
    (pieces, trace)
}

fn read_pcm(wav: &Path) -> Vec<f32> {
    read_wav_16k_mono(wav.to_str().unwrap())
}

fn since_press(trace: &DictationTrace, at: Option<HostInstant>) -> String {
    at.map_or_else(
        || "—".to_string(),
        |at| format!("{} ms", at.saturating_since(trace.pressed_at).as_millis()),
    )
}

/// REQ-DIAG-032: das erste Referenzwort steht am Anfang des Ergebnisses. Prüft nur „die Pipeline
/// verliert nichts, was die Quelle liefert“; die Startverzögerungen 0/100/300 ms prüft der CI-Test
/// `e2e_no_sample_is_lost_for_any_start_delay_and_format` (AK-20).
/// Schlägt mit einer Tabelle (erwartet / erkannt / →erstes Signal) fehl, wenn nicht.
fn assert_first_word_present(insert_mode: InsertMode, set: FixtureSet) {
    let report: Vec<String> = [Language::De, Language::Auto]
        .into_iter()
        .filter_map(|language| first_word_failures(insert_mode, set, language))
        .collect();
    assert!(report.is_empty(), "{}", report.join("\n\n"));
}

/// `Some(Tabelle)` mit den Fehlfällen, `None` wenn alle Clips das erste Wort haben.
fn first_word_failures(
    insert_mode: InsertMode,
    set: FixtureSet,
    language: Language,
) -> Option<String> {
    let _lock = MODEL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut worker = worker();
    let mut failures = Vec::new();
    for (name, wav) in fixtures(set) {
        let expected = first_word(&reference(&wav)).expect("Referenz ohne Wort");
        let (pieces, trace) = dictate(&mut worker, read_pcm(&wav), 0, insert_mode, language);
        let result = pieces.concat();
        let got = normalize_words(&result).into_iter().next();
        if got.as_deref() != Some(expected.as_str()) {
            failures.push(format!(
                "| {name} | {expected} | {} | {} | {result:?} |",
                got.as_deref().unwrap_or("—"),
                since_press(&trace, trace.first_signal_at),
            ));
        }
    }
    println!(
        "{set:?} · {insert_mode:?} · {language:?}: {} Fehlfälle",
        failures.len()
    );
    (!failures.is_empty()).then(|| {
        format!(
            "erstes Wort fehlt ({insert_mode:?}, {set:?}, {language:?}):\n| Fixture | erwartet | erkannt | →erstes Signal | Ergebnis |\n|---|---|---|---|---|\n{}",
            failures.join("\n")
        )
    })
}

macro_rules! first_word_case {
    ($name:ident, $mode:expr, $set:expr) => {
        #[test]
        fn $name() {
            assert_first_word_present($mode, $set);
        }
    };
}

first_word_case!(first_word_at_end_say, InsertMode::AtEnd, FixtureSet::Say);
first_word_case!(
    first_word_at_end_fleurs,
    InsertMode::AtEnd,
    FixtureSet::Fleurs
);
first_word_case!(
    first_word_per_sentence_say,
    InsertMode::PerSentence,
    FixtureSet::Say
);
first_word_case!(
    first_word_per_sentence_fleurs,
    InsertMode::PerSentence,
    FixtureSet::Fleurs
);

/// REQ-DIAG-033: zwei Sätze mit kurzer Pause, Satzweise → das erste Wort des zweiten Satzes
/// steht am Anfang des Textes aller Stücke nach dem ersten.
#[test]
fn first_word_of_the_second_sentence_survives_the_pause() {
    for language in [Language::De, Language::Auto] {
        assert_second_sentence_first_word(language);
    }
}

fn assert_second_sentence_first_word(language: Language) {
    let _lock = MODEL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut worker = worker();
    let pairs = [
        ("de-business", "de-frage"),
        ("fleurs-de-06", "fleurs-de-01"),
        ("fleurs-de-08", "fleurs-de-05"),
    ];
    let mut failures = Vec::new();
    for (first, second) in pairs {
        let dir = Path::new("tests/fixtures/stt");
        let (a, b) = (
            dir.join(format!("{first}.wav")),
            dir.join(format!("{second}.wav")),
        );
        let mut pcm = read_pcm(&a);
        pcm.extend(std::iter::repeat_n(0.0, 19_200)); // 1,2 s Pause
        pcm.extend(read_pcm(&b));
        let expected = first_word(&reference(&b)).expect("Referenz ohne Wort");
        let (pieces, _) = dictate(&mut worker, pcm, 0, InsertMode::PerSentence, language);
        // REQ-DIAG-033: das erste Wort des zweiten Satzes steht im Text aller Stücke nach dem ersten.
        let last_first = normalize_words(&pieces.get(1..).unwrap_or_default().concat())
            .into_iter()
            .next();
        if pieces.len() < 2 || last_first.as_deref() != Some(expected.as_str()) {
            failures.push(format!(
                "| {first} + {second} | {expected} | {} | {} | {pieces:?} |",
                last_first.as_deref().unwrap_or("—"),
                pieces.len()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "erstes Wort des zweiten Satzes fehlt ({language:?}):\n| Paar | erwartet | erkannt | Stücke | Ergebnis |\n|---|---|---|---|---|\n{}",
        failures.join("\n")
    );
}

/// REQ-DIAG-019 (AK-15): der Tap enthält exakt, was das Modell bekommt — Batch mit
/// Vor-/Nachlauf-Stille, Live zusätzlich mit der Stille nach jedem Reset.
#[test]
fn model_tap_holds_exactly_the_model_input_including_silence() {
    use talker::stt::ModelTap;
    const LEAD: usize = 6_400; // 0,4 s
    const TAIL: usize = 19_200; // 1,2 s
    let sentence = read_wav_16k_mono("tests/fixtures/de_test.wav");

    // Batch: Vorlauf + Audio + Nachlauf, und `take` leert.
    let tap = ModelTap::default();
    let mut t = transcriber().with_model_tap(tap.clone());
    t.transcribe(&sentence).unwrap();
    let batch = tap.take();
    assert_eq!(batch.len(), LEAD + sentence.len() + TAIL);
    assert!(batch[..LEAD].iter().all(|s| *s == 0.0));
    assert_eq!(&batch[LEAD..LEAD + sentence.len()], sentence.as_slice());
    assert!(batch[LEAD + sentence.len()..].iter().all(|s| *s == 0.0));
    assert!(tap.take().is_empty());

    // Leeres Audio: nichts ans Modell, nichts im Tap.
    t.transcribe(&[]).unwrap();
    assert!(tap.take().is_empty());

    // Live mit Pause: Vorlauf, alle Chunks lückenlos (keine Stille nach dem Reset), Nachlauf.
    let mut pcm = sentence.clone();
    pcm.extend(std::iter::repeat_n(0.0, 24_000));
    pcm.extend_from_slice(&sentence);
    let (updates, _) = stream(&mut t, &pcm);
    assert_eq!(updates.iter().filter(|u| u.endpoint).count(), 1);
    assert_tap_gapless(&tap.take(), &pcm);

    // Ohne Tap bleibt alles beim Alten (kein Mitschnitt, gleiche Erkennung).
    let mut plain = transcriber();
    assert!(!plain.transcribe(&sentence).unwrap().is_empty());
}

/// REQ-STREAM-016 (AK-11): Tap = Vorlauf ‖ Eingang ‖ Nachlauf, der Mittelteil bitgleich.
fn assert_tap_gapless(tap: &[f32], input: &[f32]) {
    const LEAD: usize = 6_400;
    const TAIL: usize = 19_200;
    assert_eq!(tap.len(), LEAD + input.len() + TAIL, "Länge");
    assert!(tap[..LEAD].iter().all(|s| *s == 0.0));
    assert!(
        tap[LEAD..LEAD + input.len()] == *input,
        "Mittelteil weicht vom Eingang ab"
    );
    assert!(tap[LEAD + input.len()..].iter().all(|s| *s == 0.0));
}

fn live_tap(pcm: &[f32], chunk: usize) -> (Vec<talker::stt::StreamUpdate>, Vec<f32>) {
    let tap = talker::stt::ModelTap::default();
    let mut t = transcriber().with_model_tap(tap.clone());
    t.begin();
    let updates = pcm.chunks(chunk).map(|c| t.feed(c).unwrap()).collect();
    t.finish().unwrap();
    (updates, tap.take())
}

/// AK-11 Grenzfälle: Endpoint direkt nach dem Vorlauf (Stille am Anfang), zwei Endpoints
/// kurz nacheinander, Endpoint unmittelbar vor dem Loslassen, Pause genau `endpoint_silence_ms`.
#[test]
fn live_tap_stays_gapless_at_segment_ends() {
    let _lock = MODEL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let s = read_wav_16k_mono("tests/fixtures/de_test.wav");
    let silence = |n: usize| std::iter::repeat_n(0.0f32, n);
    let silence_ms = talker::config::DEFAULT_ENDPOINT_SILENCE_MS as usize * 16;

    // Stille direkt nach dem Vorlauf, dann Satz.
    let mut a: Vec<f32> = silence(24_000).collect();
    a.extend_from_slice(&s);
    // Zwei Endpoints kurz nacheinander.
    let mut b = s.clone();
    b.extend(silence(24_000));
    b.extend_from_slice(&s);
    b.extend(silence(24_000));
    b.extend_from_slice(&s);
    // Endpoint unmittelbar vor dem Loslassen: Pause am Ende.
    let mut c = s.clone();
    c.extend(silence(24_000));
    // Pause genau endpoint_silence_ms.
    let mut d = s.clone();
    d.extend(silence(silence_ms));
    d.extend_from_slice(&s);

    for (name, pcm) in [("start", a), ("zwei", b), ("ende", c), ("genau", d)] {
        let (_, tap) = live_tap(&pcm, 8_960);
        println!("{name}: {} Samples", pcm.len());
        assert_tap_gapless(&tap, &pcm);
    }
}

/// AK-11 Grenzfall Zwangs-Endpoint: Utterance über 60 s ohne Pause.
#[test]
fn live_tap_stays_gapless_at_the_forced_endpoint() {
    let _lock = MODEL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let s = read_wav_16k_mono("tests/fixtures/de_test.wav");
    let pcm: Vec<f32> = s.iter().copied().cycle().take(16_000 * 62).collect();
    let (updates, tap) = live_tap(&pcm, 8_960);
    assert_tap_gapless(&tap, &pcm);
    let endpoints = updates.iter().filter(|u| u.endpoint).count();
    assert_eq!(endpoints, 1, "Zwangs-Segment-Ende nach 60 s genau einmal");
}

/// AK-11: Stille vor dem ersten Wort erzeugt kein Segment und kostet kein Wort.
#[test]
fn silence_before_the_first_word_gives_no_segment_and_loses_nothing() {
    let _lock = MODEL_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let s = read_wav_16k_mono("tests/fixtures/de_test.wav");
    let mut pcm = vec![0.0f32; 48_000]; // 3 s Stille
    pcm.extend_from_slice(&s);
    let mut t = transcriber();
    let (updates, rest) = stream(&mut t, &pcm);
    assert!(
        updates.iter().all(|u| !u.endpoint || !u.text.is_empty()),
        "Segment ohne Text"
    );
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
    assert!(
        text.contains("dies ist ein test"),
        "Satzanfang fehlt: {text:?}"
    );
}
