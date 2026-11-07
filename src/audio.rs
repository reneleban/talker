//! audio-capture: Mikrofon-Aufnahme (PTT-gesteuert), in-memory, Ausgabe 16 kHz mono PCM.
//!
//! Aufnahme via cpal (CoreAudio); Downmix + Resampling als pure, unit-getestete
//! Funktionen. Kein Platten-I/O, kein Upload.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rubato::audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{Fft, FixedSync, Resampler};

use crate::error::{Result, TalkerError};

/// Zielformat für STT (Nemotron/Whisper): 16 kHz mono.
pub const TARGET_SAMPLE_RATE: u32 = 16_000;
/// Utterances darunter gelten als versehentlicher Tastendruck und werden verworfen.
pub const MIN_UTTERANCE_MS: u64 = 300;

/// Länge eines Live-Chunks in ms — entspricht dem Chunk des Nemotron-Pakets.
const CHUNK_MS: u32 = 560;
/// Samples pro Live-Chunk bei 16 kHz mono.
pub const CHUNK_16K_SAMPLES: usize = (TARGET_SAMPLE_RATE * CHUNK_MS / 1000) as usize;

/// Gemeinsame Uhr (ADR-0012): Nanosekunden auf der Zeitbasis von `mach_absolute_time`,
/// identisch zur Umrechnung der cpal-Callback-Zeitstempel. Tastendruck, Callback-Blöcke
/// und alle Messpunkte der Messzeile liegen auf dieser einen Uhr.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct HostInstant(u64);

/// `ticks * numer / denom` in u128 — exakt die Formel von cpal (`host_time_to_stream_instant`).
fn ticks_to_nanos(ticks: u64, numer: u32, denom: u32) -> u64 {
    if denom == 0 {
        return ticks;
    }
    u64::try_from(u128::from(ticks) * u128::from(numer) / u128::from(denom)).unwrap_or(u64::MAX)
}

impl HostInstant {
    pub fn now() -> Self {
        static TIMEBASE: OnceLock<(u32, u32)> = OnceLock::new();
        let (numer, denom) = *TIMEBASE.get_or_init(|| {
            let mut info = mach2::mach_time::mach_timebase_info { numer: 0, denom: 0 };
            // SAFETY: `info` ist ein gültiger, beschreibbarer Out-Parameter.
            let status = unsafe { mach2::mach_time::mach_timebase_info(&mut info) };
            if status == 0 {
                (info.numer, info.denom)
            } else {
                (1, 1)
            }
        });
        // SAFETY: `mach_absolute_time` hat keine Vorbedingungen.
        let ticks = unsafe { mach2::mach_time::mach_absolute_time() };
        Self(ticks_to_nanos(ticks, numer, denom))
    }

    pub fn from_nanos(ns: u64) -> Self {
        Self(ns)
    }

    pub fn as_nanos(self) -> u64 {
        self.0
    }

    /// Abstand zu `earlier`; liegt `earlier` später, ist er 0.
    pub fn saturating_since(self, earlier: Self) -> Duration {
        Duration::from_nanos(self.0.saturating_sub(earlier.0))
    }

    pub fn saturating_add(self, d: Duration) -> Self {
        let ns = u64::try_from(d.as_nanos()).unwrap_or(u64::MAX);
        Self(self.0.saturating_add(ns))
    }
}

impl From<cpal::StreamInstant> for HostInstant {
    fn from(instant: cpal::StreamInstant) -> Self {
        Self(u64::try_from(instant.as_nanos()).unwrap_or(u64::MAX))
    }
}

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

/// Formt rohe Callback-Blöcke (interleaved, Gerätetakt) zu 560-ms-Chunks in
/// 16 kHz mono — läuft beim Verbraucher, nicht im Audio-Callback (ADR-0009).
pub struct ChunkAssembler {
    channels: u16,
    sample_rate: u32,
    /// Rohsamples (interleaved), die noch keinen vollen Chunk ergeben.
    pending: Vec<f32>,
}

impl ChunkAssembler {
    pub fn new(channels: u16, sample_rate: u32) -> Result<Self> {
        if channels == 0 {
            return Err(TalkerError::Audio("0 Kanäle gemeldet".into()));
        }
        Ok(Self {
            channels,
            sample_rate,
            pending: Vec::new(),
        })
    }

    /// Rohsamples pro vollem Chunk (Frames × Kanäle).
    fn chunk_len(&self) -> usize {
        (self.sample_rate * CHUNK_MS / 1000) as usize * usize::from(self.channels)
    }

    /// Hängt einen Block an und liefert alle dadurch vollständigen Chunks.
    pub fn push(&mut self, block: &[f32]) -> Result<Vec<Vec<f32>>> {
        self.pending.extend_from_slice(block);
        let len = self.chunk_len();
        let mut chunks = Vec::new();
        while len > 0 && self.pending.len() >= len {
            let raw: Vec<f32> = self.pending.drain(..len).collect();
            chunks.push(to_mono_16k(&raw, self.channels, self.sample_rate)?);
        }
        Ok(chunks)
    }

    /// Restliche Samples (kürzer als ein Chunk) als 16 kHz mono; leert den Puffer.
    pub fn flush(&mut self) -> Result<Vec<f32>> {
        let whole = self.pending.len() / usize::from(self.channels) * usize::from(self.channels);
        let raw: Vec<f32> = self.pending.drain(..whole).collect();
        self.pending.clear();
        to_mono_16k(&raw, self.channels, self.sample_rate)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioFormat {
    pub channels: u16,
    pub sample_rate: u32,
}

/// Format und Name der geöffneten Quelle (Gerätename erscheint in der Messzeile).
#[derive(Clone, Debug)]
pub struct SourceInfo {
    pub format: AudioFormat,
    pub device: String,
}

/// Ein Callback-Block im Geräteformat (interleaved) samt Zeitstempel der gemeinsamen Uhr.
pub struct Block {
    pub samples: Vec<f32>,
    pub at: HostInstant,
}

/// Der Live-Kanal trägt Blöcke und zum Schluss genau ein `End`.
pub enum Feed {
    Block(Block),
    End(Box<Ended>),
}

/// Abschluss einer Live-Aufnahme: Loslass-Zeitpunkt und die gesamte Aufnahme.
pub struct Ended {
    pub released_at: HostInstant,
    pub captured: Captured,
}

/// Messpunkte des Aufnahme-Starts (REQ-DIAG-014): erster Block und erstes Signal
/// (erster Block mit einem Sample ≠ 0).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StartMarks {
    pub first_block: Option<HostInstant>,
    pub first_signal: Option<HostInstant>,
}

/// Set-once-Zellen für den Audio-Callback: lockfrei, 0 = nicht gesetzt (gespeichert wird ns + 1).
#[derive(Default)]
struct MarkCells {
    first_block: AtomicU64,
    first_signal: AtomicU64,
}

impl MarkCells {
    fn record(&self, block: &[f32], at: HostInstant) {
        let value = at.as_nanos().saturating_add(1);
        let _ = self
            .first_block
            .compare_exchange(0, value, Ordering::Relaxed, Ordering::Relaxed);
        if self.first_signal.load(Ordering::Relaxed) == 0 && block.iter().any(|&s| s != 0.0) {
            let _ =
                self.first_signal
                    .compare_exchange(0, value, Ordering::Relaxed, Ordering::Relaxed);
        }
    }

    fn snapshot(&self) -> StartMarks {
        let read = |cell: &AtomicU64| match cell.load(Ordering::Relaxed) {
            0 => None,
            v => Some(HostInstant::from_nanos(v - 1)),
        };
        StartMarks {
            first_block: read(&self.first_block),
            first_signal: read(&self.first_signal),
        }
    }
}

/// Rohe Aufnahme einer Utterance im Geräteformat.
pub struct Captured {
    pub raw: Vec<f32>,
    pub info: SourceInfo,
    pub marks: StartMarks,
}

impl Captured {
    /// Aufgenommene Dauer aus der Sample-Zahl (volle Frames / Rate).
    pub fn recorded(&self) -> Duration {
        let channels = u128::from(self.info.format.channels);
        let rate = u128::from(self.info.format.sample_rate);
        if channels == 0 || rate == 0 {
            return Duration::ZERO;
        }
        let frames = self.raw.len() as u128 / channels;
        Duration::from_nanos(u64::try_from(frames * 1_000_000_000 / rate).unwrap_or(u64::MAX))
    }

    /// Rohsamples → 16 kHz mono.
    pub fn to_mono_16k(&self) -> Result<Vec<f32>> {
        to_mono_16k(
            &self.raw,
            self.info.format.channels,
            self.info.format.sample_rate,
        )
    }
}

/// Wird je Block auf dem Thread der Quelle aufgerufen (Audio-Callback: keine Datei-I/O, kein Log).
pub type OnBlock = Box<dyn FnMut(&[f32], HostInstant) + Send>;

/// Geöffnete Quelle: Drop von `stop` beendet die Lieferung — danach kommt kein Block mehr.
pub struct Opened {
    pub info: SourceInfo,
    pub stop: Box<dyn Send>,
}

/// Erzeuger von Audio-Blöcken (ADR-0012): Mikrofon in der App, WAV in Tests.
pub trait AudioSource: Send {
    fn open(self: Box<Self>, on_block: OnBlock) -> Result<Opened>;
}

/// Laufende Aufnahme über einer [`AudioSource`]: Puffer, Pegel und Live-Kanal
/// sind für alle Quellen gleich.
pub struct Recording {
    stop: Box<dyn Send>,
    buf: Arc<Mutex<Vec<f32>>>,
    info: SourceInfo,
    level: LevelHandle,
    marks: Arc<MarkCells>,
    live: Option<Sender<Feed>>,
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

/// Mikrofon über cpal (CoreAudio). `device_name`: Gerät per Name aus der Config,
/// nicht (mehr) vorhanden → Fallback auf das System-Default-Mikrofon.
pub struct MicSource {
    device_name: Option<String>,
}

impl MicSource {
    pub fn new(device_name: Option<String>) -> Self {
        Self { device_name }
    }
}

impl AudioSource for MicSource {
    fn open(self: Box<Self>, mut on_block: OnBlock) -> Result<Opened> {
        let host = cpal::default_host();
        let device = self
            .device_name
            .as_deref()
            .and_then(|wanted| {
                let found = host
                    .input_devices()
                    .ok()?
                    .find(|d| d.description().is_ok_and(|desc| desc.name() == wanted));
                if found.is_none() {
                    log::warn!("Mikrofon »{wanted}« nicht gefunden — nutze System-Default.");
                }
                found
            })
            .or_else(|| host.default_input_device())
            .ok_or_else(|| TalkerError::Audio("kein Mikrofon verfügbar".into()))?;
        let device_label = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| "unbekannt".into());
        let supported = device
            .default_input_config()
            .map_err(|e| TalkerError::Audio(format!("Eingabeformat nicht lesbar: {e}")))?;
        let sample_format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let format = AudioFormat {
            channels: config.channels,
            sample_rate: config.sample_rate,
        };
        let err_fn = |e| log::warn!("Audio-Stream-Fehler: {e}");

        let stream = match sample_format {
            SampleFormat::F32 => device.build_input_stream(
                config,
                move |data: &[f32], info: &cpal::InputCallbackInfo| {
                    on_block(data, info.timestamp().callback.into());
                },
                err_fn,
                None,
            ),
            SampleFormat::I16 => device.build_input_stream(
                config,
                move |data: &[i16], info: &cpal::InputCallbackInfo| {
                    let converted: Vec<f32> =
                        data.iter().map(|&s| f32::from(s) / 32768.0).collect();
                    on_block(&converted, info.timestamp().callback.into());
                },
                err_fn,
                None,
            ),
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

        Ok(Opened {
            info: SourceInfo {
                format,
                device: device_label,
            },
            stop: Box::new(stream),
        })
    }
}

/// Startet die Aufnahme über `source`. `live`: optionaler zusätzlicher Verbraucher
/// (Live-Diktat) — bekommt jeden Block roh als `Feed::Block`, am Ende `Feed::End`.
pub fn start(source: Box<dyn AudioSource>, live: Option<Sender<Feed>>) -> Result<Recording> {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let level = LevelHandle::default();
    let marks = Arc::new(MarkCells::default());
    let on_block: OnBlock = {
        let buf = Arc::clone(&buf);
        let level = level.clone();
        let marks = Arc::clone(&marks);
        let live = live.clone();
        Box::new(move |data, at| {
            marks.record(data, at);
            level.set(rms(data));
            if let Ok(mut b) = buf.lock() {
                b.extend_from_slice(data);
            }
            if let Some(tx) = &live {
                let _ = tx.send(Feed::Block(Block {
                    samples: data.to_vec(),
                    at,
                }));
            }
        })
    };
    let opened = source.open(on_block)?;
    Ok(Recording {
        stop: opened.stop,
        buf,
        info: opened.info,
        level,
        marks,
        live,
    })
}

impl Recording {
    /// Handle auf den Live-Pegel (fürs Aufnahme-Overlay).
    pub fn level(&self) -> LevelHandle {
        self.level.clone()
    }

    /// Format und Gerät der Quelle — Eingabe für [`ChunkAssembler::new`].
    pub fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn into_captured(self) -> (Captured, Option<Sender<Feed>>) {
        drop(self.stop);
        let raw = std::mem::take(&mut *self.buf.lock().unwrap_or_else(|e| e.into_inner()));
        (
            Captured {
                raw,
                info: self.info,
                marks: self.marks.snapshot(),
            },
            self.live,
        )
    }

    /// Beendet eine Live-Aufnahme: erst die Quelle schließen, dann `Feed::End`
    /// mit der gesamten Aufnahme senden (die Blöcke hat der Verbraucher schon).
    pub fn finish_live(self, released_at: HostInstant) {
        let (captured, live) = self.into_captured();
        if let Some(tx) = live {
            let _ = tx.send(Feed::End(Box::new(Ended {
                released_at,
                captured,
            })));
        }
    }

    /// Beendet die Aufnahme und liefert die rohe Utterance.
    pub fn stop(self) -> Captured {
        self.into_captured().0
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

    /// Live-Diktat (ADR-0009): Rohsamples → 560-ms-Chunks in 16 kHz mono.
    #[test]
    fn assembler_emits_a_chunk_per_560ms_and_keeps_the_rest() {
        let mut a = ChunkAssembler::new(1, 16_000).unwrap();
        assert!(a.push(&[0.1; 5_000]).unwrap().is_empty());
        let chunks = a.push(&[0.1; 5_000]).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), CHUNK_16K_SAMPLES);
        // Rest (10_000 - 8_960) kommt beim Flush.
        assert_eq!(a.flush().unwrap().len(), 10_000 - CHUNK_16K_SAMPLES);
        assert!(a.flush().unwrap().is_empty());
    }

    #[test]
    fn assembler_splits_large_input_into_several_chunks() {
        let mut a = ChunkAssembler::new(1, 16_000).unwrap();
        let chunks = a.push(&vec![0.0; CHUNK_16K_SAMPLES * 3 + 10]).unwrap();
        assert_eq!(chunks.len(), 3);
        assert_eq!(a.flush().unwrap().len(), 10);
    }

    #[test]
    fn assembler_downmixes_and_resamples_to_chunk_size() {
        // 48 kHz stereo: 560 ms = 26_880 Frames = 53_760 Samples → ~8_960 Samples @16 kHz.
        let mut a = ChunkAssembler::new(2, 48_000).unwrap();
        let chunks = a.push(&vec![0.2; 53_760]).unwrap();
        assert_eq!(chunks.len(), 1);
        assert!((chunks[0].len() as i64 - CHUNK_16K_SAMPLES as i64).abs() < 16);
    }

    #[test]
    fn assembler_edge_cases_empty_input_and_zero_channels() {
        assert!(ChunkAssembler::new(0, 16_000).is_err());
        let mut a = ChunkAssembler::new(1, 16_000).unwrap();
        assert!(a.push(&[]).unwrap().is_empty());
        assert!(a.flush().unwrap().is_empty());
    }

    /// Ein halber Stereo-Frame am Blockende geht nicht verloren, sondern wartet.
    #[test]
    fn assembler_keeps_incomplete_frames_for_the_next_push() {
        let mut a = ChunkAssembler::new(2, 16_000).unwrap();
        let first = a.push(&vec![0.5; CHUNK_16K_SAMPLES * 2 - 1]).unwrap();
        assert!(first.is_empty());
        let second = a.push(&[0.5]).unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].len(), CHUNK_16K_SAMPLES);
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

    /// REQ-DIAG-013: die gemeinsame Uhr ist monoton.
    #[test]
    fn host_instant_now_is_monotonic() {
        let a = HostInstant::now();
        let b = HostInstant::now();
        assert!(b >= a);
        assert!(a.as_nanos() > 0);
    }

    #[test]
    fn host_instant_arithmetic_saturates_and_roundtrips() {
        let early = HostInstant::from_nanos(1_000);
        let late = HostInstant::from_nanos(4_000);
        assert_eq!(late.saturating_since(early), Duration::from_nanos(3_000));
        assert_eq!(early.saturating_since(late), Duration::ZERO);
        assert_eq!(early.saturating_since(early), Duration::ZERO);
        assert_eq!(
            early.saturating_add(Duration::from_micros(1)),
            HostInstant::from_nanos(2_000)
        );
        assert_eq!(HostInstant::from_nanos(7).as_nanos(), 7);
        let max = HostInstant::from_nanos(u64::MAX);
        assert_eq!(max.saturating_add(Duration::from_secs(1)), max);
        assert_eq!(
            max.saturating_add(Duration::from_secs(u64::MAX)),
            max,
            "übergroße Dauer läuft nicht über"
        );
    }

    /// REQ-DIAG-013: Umrechnung bitgleich zu cpal (Grenzen 0, u32::MAX s, 999_999_999 ns).
    #[test]
    fn stream_instant_converts_to_the_same_nanoseconds() {
        let cases = [
            (0u64, 0u32),
            (0, 1),
            (1, 0),
            (12, 999_999_999),
            (u64::from(u32::MAX), 999_999_999),
        ];
        for (secs, nanos) in cases {
            let instant = cpal::StreamInstant::new(secs, nanos);
            let expected = secs * 1_000_000_000 + u64::from(nanos);
            assert_eq!(HostInstant::from(instant).as_nanos(), expected);
        }
    }

    /// cpal: `m_host_time * numer / denom` in u128 (Apple Silicon: 125/3).
    #[test]
    fn ticks_convert_like_cpal_without_overflow() {
        assert_eq!(ticks_to_nanos(24_000_000, 125, 3), 1_000_000_000);
        assert_eq!(ticks_to_nanos(0, 125, 3), 0);
        assert_eq!(ticks_to_nanos(5, 1, 1), 5);
        assert_eq!(ticks_to_nanos(u64::MAX, 125, 3), u64::MAX);
        assert_eq!(ticks_to_nanos(10, 1, 0), 10);
    }

    /// Test-Quelle: liefert die Blöcke synchron aus `open` (virtuelle Zeitstempel 1 µs je Block).
    struct ScriptedSource {
        blocks: Vec<Vec<f32>>,
        info: SourceInfo,
    }

    impl ScriptedSource {
        fn new(blocks: Vec<Vec<f32>>, channels: u16, sample_rate: u32) -> Box<Self> {
            Box::new(Self {
                blocks,
                info: SourceInfo {
                    format: AudioFormat {
                        channels,
                        sample_rate,
                    },
                    device: "Test-Mikro".into(),
                },
            })
        }
    }

    impl AudioSource for ScriptedSource {
        fn open(self: Box<Self>, mut on_block: OnBlock) -> Result<Opened> {
            for (i, block) in self.blocks.iter().enumerate() {
                on_block(block, HostInstant::from_nanos(1_000 * (i as u64 + 1)));
            }
            Ok(Opened {
                info: self.info,
                stop: Box::new(()),
            })
        }
    }

    struct FailingSource;

    impl AudioSource for FailingSource {
        fn open(self: Box<Self>, _: OnBlock) -> Result<Opened> {
            Err(TalkerError::Audio("kein Mikrofon verfügbar".into()))
        }
    }

    /// REQ-DIAG-023: `stop()` liefert genau die gelieferten Rohsamples samt Format/Gerät.
    #[test]
    fn recording_over_a_source_returns_input_format_and_device() {
        let blocks = vec![vec![0.1, 0.2], vec![0.3, 0.4], vec![0.5, 0.6]];
        let rec = start(ScriptedSource::new(blocks, 2, 48_000), None).unwrap();
        assert_eq!(rec.info().format.channels, 2);
        assert_eq!(rec.info().format.sample_rate, 48_000);
        assert_eq!(rec.info().device, "Test-Mikro");
        let captured = rec.stop();
        assert_eq!(captured.raw, [0.1, 0.2, 0.3, 0.4, 0.5, 0.6]);
        assert_eq!(captured.info.device, "Test-Mikro");
        // 48 kHz stereo → 16 kHz mono: Dauer bleibt erhalten (±1 Frame Resampler-Rand).
        let mono = captured.to_mono_16k().unwrap();
        assert!(mono.len() <= 1);
    }

    #[test]
    fn recording_without_blocks_stops_with_empty_capture() {
        let captured = start(ScriptedSource::new(vec![], 1, 16_000), None)
            .unwrap()
            .stop();
        assert!(captured.raw.is_empty());
        assert!(captured.to_mono_16k().unwrap().is_empty());
    }

    /// Der Pegel folgt dem zuletzt gelieferten Block.
    #[test]
    fn recording_level_reacts_to_the_last_block() {
        let rec = start(
            ScriptedSource::new(vec![vec![0.0; 4], vec![0.5; 4]], 1, 16_000),
            None,
        )
        .unwrap();
        assert!((rec.level().get() - 0.5).abs() < 1e-6);
    }

    /// Live-Kanal: Blöcke in Reihenfolge, genau ein `End` bei `finish_live`, danach nichts mehr.
    #[test]
    fn live_channel_gets_blocks_then_exactly_one_end() {
        let (tx, rx) = std::sync::mpsc::channel();
        let rec = start(
            ScriptedSource::new(vec![vec![1.0], vec![2.0, 3.0]], 1, 16_000),
            Some(tx),
        )
        .unwrap();
        rec.finish_live(HostInstant::from_nanos(9_000));
        let feeds: Vec<Feed> = rx.try_iter().collect();
        assert_eq!(feeds.len(), 3);
        match (&feeds[0], &feeds[1]) {
            (Feed::Block(a), Feed::Block(b)) => {
                assert_eq!(a.samples, [1.0]);
                assert_eq!(a.at, HostInstant::from_nanos(1_000));
                assert_eq!(b.samples, [2.0, 3.0]);
            }
            _ => panic!("erst zwei Blöcke erwartet"),
        }
        match &feeds[2] {
            Feed::End(ended) => {
                assert_eq!(ended.released_at, HostInstant::from_nanos(9_000));
                assert_eq!(ended.captured.raw, [1.0, 2.0, 3.0]);
            }
            Feed::Block(_) => panic!("End erwartet"),
        }
        // Kanal ist danach geschlossen: kein weiterer Block, kein zweites End.
        assert!(rx.recv().is_err());
    }

    /// Fehler aus `open` → `start` gibt `Err`.
    #[test]
    fn failing_source_makes_start_fail() {
        let result = start(Box::new(FailingSource), None);
        assert!(result.is_err_and(|e| e.to_string().contains("kein Mikrofon")));
    }

    /// Batch-Aufnahme (ohne Live-Kanal) sendet nichts und bleibt gleich.
    #[test]
    fn batch_stop_works_with_a_live_channel_too() {
        let (tx, rx) = std::sync::mpsc::channel();
        let rec = start(
            ScriptedSource::new(vec![vec![0.25; 8]], 1, 16_000),
            Some(tx),
        )
        .unwrap();
        assert_eq!(rec.stop().raw.len(), 8);
        assert!(matches!(rx.recv(), Ok(Feed::Block(_))));
        assert!(rx.recv().is_err(), "stop() sendet kein End");
    }

    fn marks_of(blocks: Vec<Vec<f32>>) -> StartMarks {
        start(ScriptedSource::new(blocks, 1, 16_000), None)
            .unwrap()
            .stop()
            .marks
    }

    /// REQ-DIAG-014: führende Null-Blöcke → erster Block < erstes Signal.
    #[test]
    fn leading_zero_blocks_put_first_block_before_first_signal() {
        let marks = marks_of(vec![vec![0.0; 4], vec![0.0; 4], vec![0.0, 0.5], vec![0.9]]);
        assert_eq!(marks.first_block, Some(HostInstant::from_nanos(1_000)));
        assert_eq!(marks.first_signal, Some(HostInstant::from_nanos(3_000)));
        assert!(marks.first_block < marks.first_signal);
    }

    #[test]
    fn only_zero_blocks_have_no_first_signal() {
        let marks = marks_of(vec![vec![0.0; 4], vec![0.0; 4]]);
        assert_eq!(marks.first_block, Some(HostInstant::from_nanos(1_000)));
        assert_eq!(marks.first_signal, None);
    }

    #[test]
    fn no_block_means_no_marks() {
        let marks = marks_of(vec![]);
        assert_eq!(marks.first_block, None);
        assert_eq!(marks.first_signal, None);
    }

    /// Ein einziges Sample ≠ 0 im ersten Block genügt.
    #[test]
    fn single_nonzero_sample_in_the_first_block_is_the_first_signal() {
        let marks = marks_of(vec![vec![0.0, 0.0, 1e-9, 0.0]]);
        assert_eq!(marks.first_signal, marks.first_block);
        assert!(marks.first_signal.is_some());
    }

    /// Marks sind set-once: spätere Blöcke ändern nichts; Zeitstempel 0 ist ein gültiger Wert.
    #[test]
    fn marks_are_set_once_and_accept_a_zero_timestamp() {
        struct ZeroStart;
        impl AudioSource for ZeroStart {
            fn open(self: Box<Self>, mut on_block: OnBlock) -> Result<Opened> {
                on_block(&[0.5], HostInstant::from_nanos(0));
                on_block(&[0.7], HostInstant::from_nanos(50));
                Ok(Opened {
                    info: SourceInfo {
                        format: AudioFormat {
                            channels: 1,
                            sample_rate: 16_000,
                        },
                        device: "x".into(),
                    },
                    stop: Box::new(()),
                })
            }
        }
        let marks = start(Box::new(ZeroStart), None).unwrap().stop().marks;
        assert_eq!(marks.first_block, Some(HostInstant::from_nanos(0)));
        assert_eq!(marks.first_signal, Some(HostInstant::from_nanos(0)));
    }

    /// `recorded()` kommt aus der Sample-Zahl (Frames / Rate), nicht aus der Uhr.
    #[test]
    fn recorded_duration_comes_from_the_sample_count() {
        let captured = |raw: usize, channels: u16, sample_rate: u32| Captured {
            raw: vec![0.0; raw],
            info: SourceInfo {
                format: AudioFormat {
                    channels,
                    sample_rate,
                },
                device: String::new(),
            },
            marks: StartMarks::default(),
        };
        assert_eq!(
            captured(96_000, 2, 48_000).recorded(),
            Duration::from_secs(1)
        );
        assert_eq!(
            captured(44_100, 1, 44_100).recorded(),
            Duration::from_secs(1)
        );
        assert_eq!(captured(0, 1, 48_000).recorded(), Duration::ZERO);
        assert_eq!(captured(10, 0, 48_000).recorded(), Duration::ZERO);
        assert_eq!(captured(10, 1, 0).recorded(), Duration::ZERO);
        // Unvollständiges letztes Frame zählt nicht.
        assert_eq!(captured(3, 2, 1).recorded(), Duration::from_secs(1));
    }
}
