//! stt: 16 kHz mono PCM → Raw Transcript.
//!
//! Engine hinter dem `Transcriber`-Trait (ADR-0001). Primäre Impl:
//! Parakeet TDT 0.6b v3 (multilingual, de/en) via sherpa-onnx, on-device.

use std::path::{Path, PathBuf};

use sherpa_rs::transducer::{TransducerConfig, TransducerRecognizer};

use crate::error::{Result, TalkerError};

/// PCM-Buffer (16 kHz mono) → Raw Transcript.
pub trait Transcriber {
    fn transcribe(&mut self, pcm_16k_mono: &[f32]) -> Result<String>;
}

pub struct ParakeetTranscriber {
    inner: TransducerRecognizer,
}

const MODEL_FILES: [&str; 4] = [
    "encoder.int8.onnx",
    "decoder.int8.onnx",
    "joiner.int8.onnx",
    "tokens.txt",
];

impl ParakeetTranscriber {
    /// Lädt das Parakeet-Modell aus `model_dir`. Fehlende Dateien → klarer Fehler.
    pub fn new(model_dir: &Path) -> Result<Self> {
        for file in MODEL_FILES {
            let path = model_dir.join(file);
            if !path.is_file() {
                return Err(TalkerError::Stt(format!(
                    "Modell-Datei fehlt: {} — Parakeet-Modell nach {} entpacken \
                     (sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8)",
                    path.display(),
                    model_dir.display()
                )));
            }
        }
        let path_str = |file: &str| model_dir.join(file).to_string_lossy().into_owned();
        let config = TransducerConfig {
            encoder: path_str("encoder.int8.onnx"),
            decoder: path_str("decoder.int8.onnx"),
            joiner: path_str("joiner.int8.onnx"),
            tokens: path_str("tokens.txt"),
            model_type: "nemo_transducer".into(),
            decoding_method: "greedy_search".into(),
            num_threads: 4,
            sample_rate: crate::audio::TARGET_SAMPLE_RATE as i32,
            feature_dim: 80,
            ..Default::default()
        };
        let inner = TransducerRecognizer::new(config)
            .map_err(|e| TalkerError::Stt(format!("Modell laden fehlgeschlagen: {e}")))?;
        Ok(Self { inner })
    }

    /// Default-Ablageort: ~/Library/Application Support/talker/models/<modellname>
    pub fn default_model_dir() -> PathBuf {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        home.join(
            "Library/Application Support/talker/models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
        )
    }
}

impl Transcriber for ParakeetTranscriber {
    fn transcribe(&mut self, pcm_16k_mono: &[f32]) -> Result<String> {
        if pcm_16k_mono.is_empty() {
            return Ok(String::new());
        }
        let text = self
            .inner
            .transcribe(crate::audio::TARGET_SAMPLE_RATE, pcm_16k_mono);
        Ok(text.trim().to_string())
    }
}

/// Pipeline-Isolation: Konsumenten hängen nur am Trait, nicht an Parakeet.
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
        let Err(err) = ParakeetTranscriber::new(Path::new("/nonexistent/model/dir")) else {
            panic!("erwartet: Fehler bei fehlendem Modell");
        };
        let msg = err.to_string();
        assert!(msg.contains("Modell-Datei fehlt"), "unklarer Fehler: {msg}");
    }
}
