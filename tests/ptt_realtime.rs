//! AK-2 / REQ-DICT-022 (Ticket 0039, CR-4): Charakterisierung der PTT-Pipeline im Echtzeittakt.
//! `WavSource` mit `Pace::RealTime` und 300 ms Anlaufzeit → kein Sample geht verloren, auch nicht
//! die führenden Null-Blöcke vor dem ersten Signal — weder im Batch-Pfad (PCM beim Modell) noch
//! im Live-Pfad (Chunks im Live-Kanal). Fakes statt Modelle, läuft in CI.

use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use talker::audio::{AudioFormat, Feed, HostInstant, Pace, SourceDone, WavOptions, WavSource};
use talker::cleanup::{CleanupMode, LlmCleaner};
use talker::config::{Config, InsertMode};
use talker::diagnostics::TraceReport;
use talker::error::Result;
use talker::models::{ModelState, ModelsState};
use talker::pipeline::{
    DictationWorker, LiveFeed, MicReady, PressOutcome, PttSession, ReleaseOutcome, SttJob,
    process_job,
};
use talker::stt::{StreamUpdate, StreamingTranscriber, Transcriber};

const BLOCK_FRAMES: usize = 480;

/// STT-Fake: hält den Modell-Eingang der Batch-Utterance fest.
struct RecordingStt {
    received: Arc<Mutex<Vec<f32>>>,
}

impl Transcriber for RecordingStt {
    fn transcribe(&mut self, pcm: &[f32]) -> Result<String> {
        *self.received.lock().unwrap() = pcm.to_vec();
        Ok("hallo".into())
    }
}

impl StreamingTranscriber for RecordingStt {
    fn begin(&mut self) {}
    fn feed(&mut self, _: &[f32]) -> Result<StreamUpdate> {
        Ok(StreamUpdate {
            text: String::new(),
            endpoint: false,
        })
    }
    fn finish(&mut self) -> Result<String> {
        Ok(String::new())
    }
}

struct NoCleanup;

impl LlmCleaner for NoCleanup {
    fn clean(&mut self, raw: &str) -> Result<String> {
        Ok(raw.to_string())
    }
}

/// Startet ein Diktat im Echtzeittakt und wartet, bis die Quelle alles geliefert hat.
fn press_and_wait(
    session: &mut PttSession,
    cfg: &Config,
    done_rx: &Receiver<SourceDone>,
) -> Option<LiveFeed> {
    let (ready_tx, ready_rx) = channel();
    let models = ModelsState::new(ModelState::Ready, ModelState::Missing);
    let outcome = session.press(HostInstant::now(), cfg, &models, move |ready| {
        let _ = ready_tx.send(ready);
    });
    assert!(matches!(outcome, PressOutcome::Starting));
    let live = match ready_rx.recv_timeout(Duration::from_secs(10)).unwrap() {
        MicReady::Recording { live, .. } => live,
        MicReady::Failed(e) => panic!("Quelle ließ sich nicht öffnen: {e}"),
    };
    let done = done_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(done.wait(Duration::from_secs(30)));
    live
}

fn session(input: Vec<f32>, done_tx: std::sync::mpsc::Sender<SourceDone>) -> PttSession {
    let done_tx = Mutex::new(done_tx);
    let report: TraceReport = Arc::new(|_| {});
    PttSession::new(
        Box::new(move |_| {
            let (source, done) = WavSource::from_samples(
                input.clone(),
                WavOptions {
                    format: AudioFormat {
                        channels: 1,
                        sample_rate: 16_000,
                    },
                    block_frames: BLOCK_FRAMES,
                    start_delay: Duration::from_millis(300),
                    pace: Pace::RealTime,
                    epoch: HostInstant::now(),
                },
            );
            let _ = done_tx.lock().unwrap().send(done);
            Box::new(source)
        }),
        report,
    )
}

fn cfg(insert_mode: InsertMode) -> Config {
    Config {
        insert_mode,
        cleanup_mode: CleanupMode::Raw,
        ..Config::default()
    }
}

/// Batch: das PCM, das beim Modell ankommt.
fn batch_model_input(input: &[f32]) -> Vec<f32> {
    let cfg = cfg(InsertMode::AtEnd);
    let (done_tx, done_rx) = channel();
    let mut session = session(input.to_vec(), done_tx);
    let received = Arc::new(Mutex::new(Vec::new()));
    let mut worker = DictationWorker::new(
        &cfg,
        Arc::new(ModelsState::new(ModelState::Ready, ModelState::Missing)),
        Box::new({
            let received = Arc::clone(&received);
            move |_| {
                Ok(RecordingStt {
                    received: Arc::clone(&received),
                })
            }
        }),
        Box::new(|| (Some(NoCleanup), false)),
    );
    assert!(press_and_wait(&mut session, &cfg, &done_rx).is_none());
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
        other => panic!("Diktat endete vorzeitig: {other:?}"),
    };
    process_job(&mut worker, job, &cfg, &mut |_| Ok(()), None);
    received.lock().unwrap().clone()
}

/// Live: die Samples aller Chunks im Live-Kanal, in Reihenfolge, bis `Feed::End`.
fn live_channel_samples(input: &[f32]) -> Vec<f32> {
    let cfg = cfg(InsertMode::PerSentence);
    let (done_tx, done_rx) = channel();
    let mut session = session(input.to_vec(), done_tx);
    let live = press_and_wait(&mut session, &cfg, &done_rx).expect("Live-Aufnahme hat einen Feed");
    assert!(matches!(
        session.release(HostInstant::now()),
        ReleaseOutcome::LiveEnded
    ));
    let mut samples = Vec::new();
    let mut ended = false;
    while let Ok(feed) = live.blocks.recv_timeout(Duration::from_secs(5)) {
        match feed {
            Feed::Block(block) => {
                assert!(!ended, "Block nach Feed::End");
                samples.extend(block.samples);
            }
            Feed::End(_) => ended = true,
        }
        if ended {
            break;
        }
    }
    assert!(ended, "Feed::End fehlt");
    samples
}

/// 300 ms Null-Blöcke, dann Signal.
fn leading_zeros_then_ramp() -> Vec<f32> {
    let mut input = vec![0.0; 4_800];
    input.extend((0..11_200).map(|i| 0.01 + (i % 1_000) as f32 / 10_000.0));
    input
}

fn only_zeros() -> Vec<f32> {
    vec![0.0; 16_000]
}

/// Genau ein Sample ≠ 0, mitten im ersten Block.
fn single_sample_in_first_block() -> Vec<f32> {
    let mut input = vec![0.0; 16_000];
    input[100] = 0.5;
    input
}

#[test]
fn batch_realtime_keeps_leading_zero_blocks() {
    let input = leading_zeros_then_ramp();
    assert_eq!(batch_model_input(&input), input);
}

#[test]
fn live_realtime_keeps_leading_zero_blocks() {
    let input = leading_zeros_then_ramp();
    assert_eq!(live_channel_samples(&input), input);
}

#[test]
fn batch_realtime_keeps_all_zero_input() {
    let input = only_zeros();
    assert_eq!(batch_model_input(&input), input);
}

#[test]
fn live_realtime_keeps_all_zero_input() {
    let input = only_zeros();
    assert_eq!(live_channel_samples(&input), input);
}

#[test]
fn batch_realtime_keeps_single_sample_in_first_block() {
    let input = single_sample_in_first_block();
    assert_eq!(batch_model_input(&input), input);
}

#[test]
fn live_realtime_keeps_single_sample_in_first_block() {
    let input = single_sample_in_first_block();
    assert_eq!(live_channel_samples(&input), input);
}
