//! audio-capture: Mikrofon-Aufnahme (PTT-gesteuert), in-memory, Ausgabe 16 kHz mono PCM.
//!
//! Aufnahme via cpal (CoreAudio); Downmix + Resampling als pure, unit-getestete
//! Funktionen. Kein Platten-I/O, kein Upload.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream};
use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{Fft, FixedSync, Resampler};

use crate::error::{Result, TalkerError};

/// Zielformat für STT (Parakeet/Whisper): 16 kHz mono.
pub const TARGET_SAMPLE_RATE: u32 = 16_000;
/// Utterances darunter gelten als versehentlicher Tastendruck und werden verworfen.
pub const MIN_UTTERANCE_MS: u64 = 300;

/// Live-Pegel der laufenden Aufnahme (RMS des letzten Callback-Frames,
/// als f32-Bits in einem Atomic — lockfrei lesbar aus dem UI-Thread).
#[derive(Clone, Default)]
pub struct LevelHandle(Arc<AtomicU32>);

impl LevelHandle {
    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    pub(crate) fn set(&self, rms: f32) {
        self.0.store(rms.to_bits(), Ordering::Relaxed);
    }
}

/// RMS eines Sample-Frames (0.0 bei leerem Input).
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Laufende Aufnahme. `stop()` beendet sie und liefert 16 kHz mono PCM.
pub struct Recording {
    stream: Stream,
    buf: Arc<Mutex<Vec<f32>>>,
    channels: u16,
    sample_rate: u32,
    level: LevelHandle,
}

/// Namen aller Eingabegeräte (für die Mikrofon-Auswahl in den Settings).
pub fn input_device_names() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|devices| {
            devices
                .filter_map(|d| d.description().ok().map(|desc| desc.name().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Startet die Mikrofon-Aufnahme. `device_name`: Gerät per Name aus der Config,
/// nicht (mehr) vorhanden → Fallback auf das System-Default-Mikrofon.
pub fn start(device_name: Option<&str>) -> Result<Recording> {
    let host = cpal::default_host();
    let device = device_name
        .and_then(|wanted| {
            let found = host
                .input_devices()
                .ok()?
                .find(|d| d.description().is_ok_and(|desc| desc.name() == wanted));
            if found.is_none() {
                eprintln!("talker: Mikrofon »{wanted}« nicht gefunden — nutze System-Default.");
            }
            found
        })
        .or_else(|| host.default_input_device())
        .ok_or_else(|| TalkerError::Audio("kein Mikrofon verfügbar".into()))?;
    let supported = device
        .default_input_config()
        .map_err(|e| TalkerError::Audio(format!("Eingabeformat nicht lesbar: {e}")))?;
    let sample_format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let channels = config.channels;
    let sample_rate = config.sample_rate;

    let buf = Arc::new(Mutex::new(Vec::new()));
    let level = LevelHandle::default();
    let err_fn = |e| eprintln!("talker: Audio-Stream-Fehler: {e}");

    let stream = match sample_format {
        SampleFormat::F32 => {
            let buf = Arc::clone(&buf);
            let level = level.clone();
            device.build_input_stream(
                config,
                move |data: &[f32], _| {
                    level.set(rms(data));
                    if let Ok(mut b) = buf.lock() {
                        b.extend_from_slice(data);
                    }
                },
                err_fn,
                None,
            )
        }
        SampleFormat::I16 => {
            let buf = Arc::clone(&buf);
            let level = level.clone();
            device.build_input_stream(
                config,
                move |data: &[i16], _| {
                    if let Ok(mut b) = buf.lock() {
                        let start = b.len();
                        b.extend(data.iter().map(|&s| f32::from(s) / 32768.0));
                        level.set(rms(&b[start..]));
                    }
                },
                err_fn,
                None,
            )
        }
        other => {
            return Err(TalkerError::Audio(format!(
                "nicht unterstütztes Sample-Format: {other}"
            )));
        }
    }
    .map_err(|e| TalkerError::Audio(format!("Aufnahme-Stream fehlgeschlagen: {e}")))?;

    stream
        .play()
        .map_err(|e| TalkerError::Audio(format!("Aufnahme-Start fehlgeschlagen: {e}")))?;

    Ok(Recording {
        stream,
        buf,
        channels,
        sample_rate,
        level,
    })
}

impl Recording {
    /// Handle auf den Live-Pegel (fürs Aufnahme-Overlay).
    pub fn level(&self) -> LevelHandle {
        self.level.clone()
    }

    /// Beendet die Aufnahme und liefert die Utterance als 16 kHz mono PCM.
    pub fn stop(self) -> Result<Vec<f32>> {
        drop(self.stream);
        let raw = std::mem::take(
            &mut *self
                .buf
                .lock()
                .map_err(|_| TalkerError::Audio("Aufnahme-Puffer vergiftet".into()))?,
        );
        to_mono_16k(&raw, self.channels, self.sample_rate)
    }
}

/// Dauer eines 16-kHz-mono-Puffers in Millisekunden.
pub fn duration_ms(samples: &[f32]) -> u64 {
    (samples.len() as u64 * 1000) / u64::from(TARGET_SAMPLE_RATE)
}

/// Interleaved Mehrkanal-PCM → 16 kHz mono. Leerer Input → leerer Output.
pub fn to_mono_16k(interleaved: &[f32], channels: u16, sample_rate: u32) -> Result<Vec<f32>> {
    if channels == 0 {
        return Err(TalkerError::Audio("0 Kanäle gemeldet".into()));
    }
    let mono = downmix(interleaved, channels);
    if sample_rate == TARGET_SAMPLE_RATE || mono.is_empty() {
        return Ok(mono);
    }
    resample(&mono, sample_rate, TARGET_SAMPLE_RATE)
}

fn downmix(interleaved: &[f32], channels: u16) -> Vec<f32> {
    let n = usize::from(channels);
    if n == 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(n)
        .map(|frame| frame.iter().sum::<f32>() / n as f32)
        .collect()
}

fn resample(mono: &[f32], from: u32, to: u32) -> Result<Vec<f32>> {
    let map_err = |e: &dyn std::fmt::Display| TalkerError::Audio(format!("Resampling: {e}"));
    let mut resampler = Fft::<f32>::new(from as usize, to as usize, 1024, 2, 1, FixedSync::Input)
        .map_err(|e| map_err(&e))?;

    let input_len = mono.len();
    let input_data = vec![mono.to_vec()];
    let input = SequentialSliceOfVecs::new(&input_data, 1, input_len).map_err(|e| map_err(&e))?;

    let out_len = resampler.process_all_needed_output_len(input_len);
    let mut output_data = vec![vec![0.0f32; out_len]];
    let mut output =
        SequentialSliceOfVecs::new_mut(&mut output_data, 1, out_len).map_err(|e| map_err(&e))?;

    let (_, n_out) = resampler
        .process_all_into_buffer(&input, &mut output, input_len, None)
        .map_err(|e| map_err(&e))?;

    let mut result = output_data.into_iter().next().unwrap_or_default();
    result.truncate(n_out);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, sample_rate: u32, duration_s: f32) -> Vec<f32> {
        let n = (sample_rate as f32 * duration_s) as usize;
        (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / sample_rate as f32).sin())
            .collect()
    }

    #[test]
    fn resample_48k_mono_to_16k_has_third_of_samples() {
        let input = sine(440.0, 48_000, 1.0);
        let out = to_mono_16k(&input, 1, 48_000).unwrap();
        let expected = input.len() / 3;
        assert!(
            (out.len() as i64 - expected as i64).unsigned_abs() < 16,
            "erwartet ~{expected}, bekommen {}",
            out.len()
        );
        assert!(
            out.iter().any(|&s| s.abs() > 0.1),
            "Signal darf nicht leer sein"
        );
    }

    #[test]
    fn stereo_is_downmixed_by_averaging() {
        // L=1.0, R=0.0 → mono 0.5; bereits 16 kHz → kein Resampling.
        let interleaved: Vec<f32> = [1.0f32, 0.0].repeat(1600);
        let out = to_mono_16k(&interleaved, 2, 16_000).unwrap();
        assert_eq!(out.len(), 1600);
        assert!(out.iter().all(|&s| (s - 0.5).abs() < f32::EPSILON));
    }

    #[test]
    fn empty_input_gives_empty_output() {
        assert!(to_mono_16k(&[], 1, 48_000).unwrap().is_empty());
        assert!(to_mono_16k(&[], 2, 16_000).unwrap().is_empty());
    }

    #[test]
    fn very_short_input_does_not_crash() {
        // 10 ms @ 44,1 kHz — weit unter der Resampler-Chunk-Größe.
        let input = sine(440.0, 44_100, 0.01);
        let out = to_mono_16k(&input, 1, 44_100).unwrap();
        let expected = (input.len() as f64 * 16_000.0 / 44_100.0) as usize;
        assert!(
            (out.len() as i64 - expected as i64).unsigned_abs() < 16,
            "erwartet ~{expected}, bekommen {}",
            out.len()
        );
    }

    #[test]
    fn sixteen_khz_mono_passes_through_unchanged() {
        let input = sine(440.0, 16_000, 0.5);
        let out = to_mono_16k(&input, 1, 16_000).unwrap();
        assert_eq!(out, input);
    }

    #[test]
    fn zero_channels_is_an_error() {
        assert!(to_mono_16k(&[0.0], 0, 16_000).is_err());
    }

    #[test]
    fn resample_22050_to_16k_non_integer_ratio() {
        let input = sine(440.0, 22_050, 1.0);
        let out = to_mono_16k(&input, 1, 22_050).unwrap();
        let expected = (input.len() as f64 * 16_000.0 / 22_050.0) as usize;
        assert!(
            (out.len() as i64 - expected as i64).unsigned_abs() < 16,
            "erwartet ~{expected}, bekommen {}",
            out.len()
        );
        assert!(
            out.iter().any(|&s| s.abs() > 0.1),
            "Signal darf nicht leer sein"
        );
    }

    #[test]
    fn resample_8k_to_16k_upsamples_to_double_length() {
        let input = sine(440.0, 8_000, 1.0);
        let out = to_mono_16k(&input, 1, 8_000).unwrap();
        let expected = input.len() * 2;
        assert!(
            (out.len() as i64 - expected as i64).unsigned_abs() < 16,
            "erwartet ~{expected}, bekommen {}",
            out.len()
        );
        assert!(
            out.iter().any(|&s| s.abs() > 0.1),
            "Signal darf nicht leer sein"
        );
    }

    #[test]
    fn four_channel_input_is_downmixed_by_averaging() {
        // Frame [1.0, 0.0, 0.5, 0.5] → Mittelwert 0.5; bereits 16 kHz.
        let interleaved: Vec<f32> = [1.0f32, 0.0, 0.5, 0.5].repeat(1600);
        let out = to_mono_16k(&interleaved, 4, 16_000).unwrap();
        assert_eq!(out.len(), 1600);
        assert!(out.iter().all(|&s| (s - 0.5).abs() < f32::EPSILON));
    }

    #[test]
    fn zero_sample_rate_is_a_clear_error_not_a_panic() {
        let err = to_mono_16k(&[0.1; 100], 1, 0).unwrap_err();
        assert!(
            matches!(err, TalkerError::Audio(ref msg) if msg.contains("Resampling")),
            "erwartet Audio-Fehler, bekam: {err}"
        );
    }

    #[test]
    fn rms_of_empty_is_zero_and_sine_is_amp_over_sqrt2() {
        assert_eq!(rms(&[]), 0.0);
        assert_eq!(rms(&[0.0; 480]), 0.0);
        let s = sine(440.0, 16_000, 0.5);
        let expected = 1.0 / std::f32::consts::SQRT_2;
        assert!((rms(&s) - expected).abs() < 0.01, "rms = {}", rms(&s));
    }

    #[test]
    fn level_handle_roundtrip() {
        let h = LevelHandle::default();
        assert_eq!(h.get(), 0.0);
        h.set(0.42);
        assert!((h.get() - 0.42).abs() < f32::EPSILON);
    }

    #[test]
    fn duration_of_16k_buffer() {
        assert_eq!(duration_ms(&vec![0.0; 16_000]), 1000);
        assert_eq!(duration_ms(&vec![0.0; 1600]), 100);
        assert_eq!(duration_ms(&[]), 0);
    }
}
