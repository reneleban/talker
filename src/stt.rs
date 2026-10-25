//! stt: 16 kHz mono PCM → Raw Transcript.
//!
//! Engine hinter dem `Transcriber`-Trait (ADR-0001). Primäre Impl:
//! NVIDIA Nemotron 3.5 ASR Streaming 0.6b (multilingual, de/en) via sherpa-onnx
//! (Crate `sherpa-onnx`), on-device.

use std::path::{Path, PathBuf};

use sherpa_onnx::{
    OnlineModelConfig, OnlineRecognizer, OnlineRecognizerConfig, OnlineTransducerModelConfig,
};

use crate::error::{Result, TalkerError};

/// PCM-Buffer (16 kHz mono) → Raw Transcript.
pub trait Transcriber {
    fn transcribe(&mut self, pcm_16k_mono: &[f32]) -> Result<String>;
}

pub struct NemotronTranscriber {
    inner: OnlineRecognizer,
}

/// Stille vor dem Audio: das Streaming-Modell verschluckt sonst den Satzanfang.
const LEAD_SILENCE_SAMPLES: usize = 6_400; // 0,4 s bei 16 kHz
/// Stille nach dem Audio: füllt den letzten 560-ms-Chunk, sonst fehlen die
/// letzten Wörter (PoC: mit 0,3 s fehlten sie, mit 1,2 s nicht).
const TAIL_SILENCE_SAMPLES: usize = 19_200; // 1,2 s bei 16 kHz
/// Chunk des Pakets: 560 ms bei 16 kHz — der Nachlauf muss ihn füllen.
const CHUNK_SAMPLES: usize = 8_960;
const _: () = assert!(TAIL_SILENCE_SAMPLES >= CHUNK_SAMPLES);

/// Mehr Threads verkürzen die Wartezeit nach dem Loslassen deutlich (Messung auf
/// einem 14-Kern-Mac: 4 → 1,09 s, 8 → 0,73 s); darüber bremsen Effizienz-Kerne.
fn num_threads() -> i32 {
    std::thread::available_parallelism().map_or(4, |n| n.get().min(8)) as i32
}

fn pad_with_silence(pcm: &[f32]) -> Vec<f32> {
    let mut padded = Vec::with_capacity(LEAD_SILENCE_SAMPLES + pcm.len() + TAIL_SILENCE_SAMPLES);
    padded.resize(LEAD_SILENCE_SAMPLES, 0.0);
    padded.extend_from_slice(pcm);
    padded.resize(padded.len() + TAIL_SILENCE_SAMPLES, 0.0);
    padded
}

const MODEL_FILES: [&str; 4] = [
    "encoder.int8.onnx",
    "decoder.int8.onnx",
    "joiner.int8.onnx",
    "tokens.txt",
];

impl NemotronTranscriber {
    /// Lädt das Nemotron-Modell aus `model_dir`. Fehlende Dateien → klarer Fehler.
    pub fn new(model_dir: &Path) -> Result<Self> {
        for file in MODEL_FILES {
            let path = model_dir.join(file);
            if !path.is_file() {
                return Err(TalkerError::Stt(format!(
                    "Modell-Datei fehlt: {} — Nemotron-Modell nach {} entpacken \
                     (sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11)",
                    path.display(),
                    model_dir.display()
                )));
            }
        }
        let path = |file: &str| Some(model_dir.join(file).to_string_lossy().into_owned());
        let config = OnlineRecognizerConfig {
            model_config: OnlineModelConfig {
                transducer: OnlineTransducerModelConfig {
                    encoder: path("encoder.int8.onnx"),
                    decoder: path("decoder.int8.onnx"),
                    joiner: path("joiner.int8.onnx"),
                },
                tokens: path("tokens.txt"),
                num_threads: num_threads(),
                ..Default::default()
            },
            decoding_method: Some("greedy_search".into()),
            ..Default::default()
        };
        let inner = OnlineRecognizer::create(&config)
            .ok_or_else(|| TalkerError::Stt("Modell laden fehlgeschlagen".into()))?;
        Ok(Self { inner })
    }

    /// Default-Ablageort: ~/Library/Application Support/talker/models/<modellname>
    pub fn default_model_dir() -> PathBuf {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        home.join(
            "Library/Application Support/talker/models/sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11",
        )
    }
}

impl Transcriber for NemotronTranscriber {
    fn transcribe(&mut self, pcm_16k_mono: &[f32]) -> Result<String> {
        if pcm_16k_mono.is_empty() {
            return Ok(String::new());
        }
        let stream = self.inner.create_stream();
        // Sprache automatisch erkennen (de/en-Mix im Alltag).
        stream.set_option("language", "auto");
        stream.accept_waveform(
            crate::audio::TARGET_SAMPLE_RATE as i32,
            &pad_with_silence(pcm_16k_mono),
        );
        stream.input_finished();
        while self.inner.is_ready(&stream) {
            self.inner.decode(&stream);
        }
        let text = self
            .inner
            .get_result(&stream)
            .map(|r| r.text)
            .unwrap_or_default();
        Ok(text.trim().to_string())
    }
}

/// Pipeline-Isolation: Konsumenten hängen nur am Trait, nicht an die Engine.
#[cfg(test)]
pub(crate) struct FakeTranscriber {
    pub(crate) reply: &'static str,
}

#[cfg(test)]
impl Transcriber for FakeTranscriber {
    fn transcribe(&mut self, pcm: &[f32]) -> Result<String> {
        if pcm.is_empty() {
            return Ok(String::new());
        }
        Ok(self.reply.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_surrounds_the_audio_with_silence() {
        let pcm = [0.5_f32; 1_600];
        let padded = pad_with_silence(&pcm);
        assert_eq!(
            padded.len(),
            LEAD_SILENCE_SAMPLES + pcm.len() + TAIL_SILENCE_SAMPLES
        );
        assert!(padded[..LEAD_SILENCE_SAMPLES].iter().all(|s| *s == 0.0));
        assert_eq!(
            &padded[LEAD_SILENCE_SAMPLES..LEAD_SILENCE_SAMPLES + pcm.len()],
            &pcm
        );
        assert!(
            padded[LEAD_SILENCE_SAMPLES + pcm.len()..]
                .iter()
                .all(|s| *s == 0.0)
        );
    }

    #[test]
    fn padding_of_empty_audio_is_only_silence() {
        assert_eq!(
            pad_with_silence(&[]).len(),
            LEAD_SILENCE_SAMPLES + TAIL_SILENCE_SAMPLES
        );
    }

    #[test]
    fn trait_is_object_safe_and_swappable() {
        let mut t: Box<dyn Transcriber> = Box::new(FakeTranscriber {
            reply: "hallo welt",
        });
        assert_eq!(t.transcribe(&[0.0; 160]).unwrap(), "hallo welt");
    }

    #[test]
    fn empty_pcm_gives_empty_transcript() {
        let mut t: Box<dyn Transcriber> = Box::new(FakeTranscriber { reply: "x" });
        assert_eq!(t.transcribe(&[]).unwrap(), "");
    }

    #[test]
    fn missing_model_dir_is_a_clear_error_not_a_crash() {
        let Err(err) = NemotronTranscriber::new(Path::new("/nonexistent/model/dir")) else {
            panic!("erwartet: Fehler bei fehlendem Modell");
        };
        let msg = err.to_string();
        assert!(msg.contains("Modell-Datei fehlt"), "unklarer Fehler: {msg}");
    }
}
