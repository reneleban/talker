//! audio-capture: Mikrofon-Aufnahme (PTT-gesteuert), in-memory, Ausgabe 16 kHz mono PCM.
//!
//! Aufnahme via cpal (CoreAudio); Downmix + Resampling als pure, unit-getestete
//! Funktionen. Aufnahme ohne Platten-I/O und ohne Upload; `WavSource` liest Test-/Mess-WAVs.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
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

/// Lesender Griff auf das erste Signal einer laufenden Aufnahme (REQ-DICT-016):
/// `arrived()` ist wahr, sobald ein Block mit einem Sample ≠ 0 eingetroffen ist.
/// Nur ein Atomic-Load — der Audio-Callback meldet nichts, der Verbraucher fragt.
#[derive(Clone)]
pub struct SignalHandle(Arc<MarkCells>);

impl SignalHandle {
    pub fn arrived(&self) -> bool {
        self.0.first_signal.load(Ordering::Relaxed) != 0
    }

    /// Handgesteuertes Handle für Tests anderer Module (Signal noch nicht da).
    #[cfg(test)]
    pub(crate) fn pending() -> Self {
        Self(Arc::new(MarkCells::default()))
    }

    /// Setzt das Signal von Hand (nur Tests).
    #[cfg(test)]
    pub(crate) fn fire(&self) {
        self.0.first_signal.store(1, Ordering::Relaxed);
    }
}

/// Rohe Aufnahme einer Utterance im Geräteformat.
#[derive(Debug)]
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
pub type OnBlock = Arc<dyn Fn(&[f32], HostInstant) + Send + Sync>;

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

/// Anfangskapazität des I16-Scratch-Puffers (typische Callback-Größe bei 48 kHz stereo).
const I16_SCRATCH_SAMPLES: usize = 4_096;

/// I16 → f32 in `[-1, 1)`; ersetzt den Inhalt von `out` und behält dessen Kapazität.
fn i16_to_f32(data: &[i16], out: &mut Vec<f32>) {
    out.clear();
    out.extend(data.iter().map(|&s| f32::from(s) / 32768.0));
}

/// Format des Eingabegeräts; Teil des Gerätecaches (Änderung → neu ermitteln).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MicFormat {
    channels: u16,
    sample_rate: u32,
    sample_format: SampleFormat,
}

/// Gemerkt wird nur Geräte-Referenz und Format — nie ein Stream (ADR-0014 §2).
#[derive(Clone)]
struct CachedMic<D> {
    wanted: Option<String>,
    device: D,
    uid: String,
    format: MicFormat,
    label: String,
}

/// Gerätecache über Tastendrücke hinweg; gesperrt wird er nur kurz auf dem Start-Thread.
pub struct MicCache<D = cpal::Device>(Arc<Mutex<Option<CachedMic<D>>>>);

impl<D> Clone for MicCache<D> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<D> Default for MicCache<D> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }
}

impl<D: Clone> MicCache<D> {
    fn get(&self) -> Option<CachedMic<D>> {
        self.0.lock().ok()?.clone()
    }

    fn set(&self, entry: CachedMic<D>) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = Some(entry);
        }
    }

    fn clear(&self) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = None;
        }
    }
}

/// Zugriff auf das Eingabegerät; in Tests ein zählender Fake.
pub trait MicBackend: Send {
    type Device: Clone + Send;
    /// Standard-Eingabegerät (keine Aufzählung).
    fn default_device(&self) -> Option<Self::Device>;
    /// Gerät per Name (Aufzählung).
    fn find_by_name(&self, name: &str) -> Option<Self::Device>;
    /// Stabile Kennung; `Err` = Gerät weg.
    fn uid(&self, device: &Self::Device) -> Result<String>;
    fn label(&self, device: &Self::Device) -> String;
    fn format(&self, device: &Self::Device) -> Result<MicFormat>;
    /// Stream bauen und starten; Drop des Ergebnisses beendet ihn.
    fn open(
        &self,
        device: &Self::Device,
        format: MicFormat,
        on_block: OnBlock,
    ) -> Result<Box<dyn Send>>;
}

pub struct CpalBackend;

impl MicBackend for CpalBackend {
    type Device = cpal::Device;

    fn default_device(&self) -> Option<cpal::Device> {
        cpal::default_host().default_input_device()
    }

    fn find_by_name(&self, name: &str) -> Option<cpal::Device> {
        cpal::default_host()
            .input_devices()
            .ok()?
            .find(|d| d.description().is_ok_and(|desc| desc.name() == name))
    }

    fn uid(&self, device: &cpal::Device) -> Result<String> {
        device
            .id()
            .map(|id| id.to_string())
            .map_err(|e| TalkerError::Audio(format!("Geräte-ID nicht lesbar: {e}")))
    }

    fn label(&self, device: &cpal::Device) -> String {
        device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| "unbekannt".into())
    }

    fn format(&self, device: &cpal::Device) -> Result<MicFormat> {
        let supported = device
            .default_input_config()
            .map_err(|e| TalkerError::Audio(format!("Eingabeformat nicht lesbar: {e}")))?;
        Ok(MicFormat {
            channels: supported.channels(),
            sample_rate: supported.sample_rate(),
            sample_format: supported.sample_format(),
        })
    }

    fn open(
        &self,
        device: &cpal::Device,
        format: MicFormat,
        on_block: OnBlock,
    ) -> Result<Box<dyn Send>> {
        let config = cpal::StreamConfig {
            channels: format.channels,
            sample_rate: format.sample_rate,
            buffer_size: cpal::BufferSize::Default,
        };
        let err_fn = |e| log::warn!("Audio-Stream-Fehler: {e}");

        let stream = match format.sample_format {
            SampleFormat::F32 => device.build_input_stream(
                config,
                move |data: &[f32], info: &cpal::InputCallbackInfo| {
                    on_block(data, info.timestamp().callback.into());
                },
                err_fn,
                None,
            ),
            SampleFormat::I16 => {
                // Scratch-Puffer: keine Allokation je Callback (nur beim ersten größeren Block).
                let mut scratch: Vec<f32> = Vec::with_capacity(I16_SCRATCH_SAMPLES);
                device.build_input_stream(
                    config,
                    move |data: &[i16], info: &cpal::InputCallbackInfo| {
                        i16_to_f32(data, &mut scratch);
                        on_block(&scratch, info.timestamp().callback.into());
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
        Ok(Box::new(stream))
    }
}

/// Mikrofon über cpal (CoreAudio). `device_name`: Gerät per Name aus der Config,
/// nicht (mehr) vorhanden → Fallback auf das System-Default-Mikrofon.
pub struct MicSource<B: MicBackend = CpalBackend> {
    device_name: Option<String>,
    cache: MicCache<B::Device>,
    backend: B,
}

impl MicSource {
    pub fn new(device_name: Option<String>, cache: MicCache) -> Self {
        Self::with_backend(device_name, cache, CpalBackend)
    }
}

impl<B: MicBackend> MicSource<B> {
    fn with_backend(device_name: Option<String>, cache: MicCache<B::Device>, backend: B) -> Self {
        Self {
            device_name,
            cache,
            backend,
        }
    }

    /// Cache-Treffer, wenn Wunsch, Gerät und Format unverändert sind (günstige Prüfabfrage,
    /// keine Aufzählung); sonst volle Neuermittlung.
    fn cached(&self) -> Option<(B::Device, MicFormat, String)> {
        let c = self.cache.get()?;
        (c.wanted == self.device_name && self.still_current(&c))
            .then_some((c.device, c.format, c.label))
    }

    fn still_current(&self, cached: &CachedMic<B::Device>) -> bool {
        let same_device = match self.device_name {
            None => self
                .backend
                .default_device()
                .and_then(|d| self.backend.uid(&d).ok())
                .is_some_and(|uid| uid == cached.uid),
            Some(_) => self
                .backend
                .uid(&cached.device)
                .is_ok_and(|uid| uid == cached.uid),
        };
        same_device
            && self
                .backend
                .format(&cached.device)
                .is_ok_and(|f| f == cached.format)
    }

    /// Öffnet den Stream; bei Fehler wird der Cache geleert.
    fn start(
        &self,
        (device, format, label): (B::Device, MicFormat, String),
        on_block: OnBlock,
    ) -> Result<Opened> {
        match self.backend.open(&device, format, on_block) {
            Ok(stop) => Ok(Opened {
                info: SourceInfo {
                    format: AudioFormat {
                        channels: format.channels,
                        sample_rate: format.sample_rate,
                    },
                    device: label,
                },
                stop,
            }),
            Err(e) => {
                self.cache.clear();
                Err(e)
            }
        }
    }

    fn discover(&self) -> Result<(B::Device, MicFormat, String)> {
        let named = self.device_name.as_deref().and_then(|wanted| {
            let found = self.backend.find_by_name(wanted);
            if found.is_none() {
                log::warn!("Mikrofon »{wanted}« nicht gefunden — nutze System-Default.");
            }
            found
        });
        // Ausweichen aufs Standardgerät wird nicht gemerkt: jeder Druck sucht wieder per Name.
        let cacheable = self.device_name.is_none() || named.is_some();
        let device = named
            .or_else(|| self.backend.default_device())
            .ok_or_else(|| TalkerError::Audio("kein Mikrofon verfügbar".into()))?;
        let format = self.backend.format(&device)?;
        let label = self.backend.label(&device);
        if cacheable && let Ok(uid) = self.backend.uid(&device) {
            self.cache.set(CachedMic {
                wanted: self.device_name.clone(),
                device: device.clone(),
                uid,
                format,
                label: label.clone(),
            });
        }
        Ok((device, format, label))
    }
}

impl<B: MicBackend> AudioSource for MicSource<B> {
    fn open(self: Box<Self>, on_block: OnBlock) -> Result<Opened> {
        let (hit, found) = match self.cached() {
            Some(c) => (true, c),
            None => (false, self.discover()?),
        };
        // Scheitert der Start, bleibt kein Eintrag stehen; mit Cache-Daten gibt es genau einen Neuversuch.
        match self.start(found, Arc::clone(&on_block)) {
            Err(e) if hit => {
                log::warn!("Mikrofon-Start mit gemerktem Gerät gescheitert ({e}) — neu ermitteln.");
                self.start(self.discover()?, on_block)
            }
            r => r,
        }
    }
}

/// Takt der [`WavSource`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pace {
    /// Blöcke im Echtzeittakt, Zeitstempel = `HostInstant::now()` bei der Lieferung.
    RealTime,
    /// Alle Blöcke sofort, Zeitstempel virtuell ab `epoch + start_delay`.
    Immediate,
}

/// Einstellungen der [`WavSource`]: Format, wie es ein Gerät liefern würde.
#[derive(Clone, Copy, Debug)]
pub struct WavOptions {
    pub format: AudioFormat,
    /// Frames je Block (mind. 1).
    pub block_frames: usize,
    /// Zeit bis zum ersten Block (Mikrofon-Anlaufzeit).
    pub start_delay: Duration,
    pub pace: Pace,
    /// Zeitnullpunkt der virtuellen Zeitstempel bei `Pace::Immediate`.
    pub epoch: HostInstant,
}

/// Meldet, dass die [`WavSource`] ihren letzten Block geliefert hat (oder beendet wurde).
#[derive(Clone)]
pub struct SourceDone(Arc<(Mutex<bool>, Condvar)>);

impl SourceDone {
    fn new() -> Self {
        Self(Arc::new((Mutex::new(false), Condvar::new())))
    }

    fn signal(&self) {
        let (flag, cv) = &*self.0;
        *flag.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cv.notify_all();
    }

    /// Wartet höchstens `timeout`; `true`, wenn die Lieferung beendet ist.
    pub fn wait(&self, timeout: Duration) -> bool {
        let (flag, cv) = &*self.0;
        let guard = flag.lock().unwrap_or_else(|e| e.into_inner());
        let (guard, _) = cv
            .wait_timeout_while(guard, timeout, |done| !*done)
            .unwrap_or_else(|e| e.into_inner());
        *guard
    }
}

/// Test- und Messquelle: liefert eine WAV-Aufnahme als Blöcke (ADR-0012).
pub struct WavSource {
    mono_16k: Vec<f32>,
    opts: WavOptions,
    name: String,
    done: SourceDone,
}

impl WavSource {
    /// Liest `path` (beliebige Rate/Kanäle/Bittiefe) und wandelt auf 16 kHz mono.
    pub fn from_file(path: &Path, opts: WavOptions) -> Result<(Self, SourceDone)> {
        let fail = |what: &str| TalkerError::Audio(format!("WAV {}: {what}", path.display()));
        let mut reader = hound::WavReader::open(path).map_err(|e| fail(&e.to_string()))?;
        let spec = reader.spec();
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader
                .samples::<f32>()
                .collect::<std::result::Result<_, _>>()
                .map_err(|e| fail(&e.to_string()))?,
            hound::SampleFormat::Int => {
                let scale = 2f32.powi(i32::from(spec.bits_per_sample) - 1);
                reader
                    .samples::<i32>()
                    .map(|s| s.map(|v| v as f32 / scale))
                    .collect::<std::result::Result<_, _>>()
                    .map_err(|e| fail(&e.to_string()))?
            }
        };
        let mono = to_mono_16k(&samples, spec.channels, spec.sample_rate)
            .map_err(|e| fail(&e.to_string()))?;
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(Self::build(mono, opts, name))
    }

    /// Aus 16-kHz-mono-Samples, z. B. mit führenden Null-Blöcken.
    pub fn from_samples(mono_16k: Vec<f32>, opts: WavOptions) -> (Self, SourceDone) {
        Self::build(mono_16k, opts, "Samples".into())
    }

    fn build(mono_16k: Vec<f32>, opts: WavOptions, name: String) -> (Self, SourceDone) {
        let done = SourceDone::new();
        (
            Self {
                mono_16k,
                opts,
                name,
                done: done.clone(),
            },
            done,
        )
    }
}

/// Beendet den Lieferthread beim Drop und wartet auf ihn — danach kommt kein Block mehr.
struct WavGuard {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for WavGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            handle.thread().unpark();
            let _ = handle.join();
        }
    }
}

impl AudioSource for WavSource {
    fn open(self: Box<Self>, on_block: OnBlock) -> Result<Opened> {
        let WavSource {
            mono_16k,
            opts,
            name,
            done,
        } = *self;
        let channels = opts.format.channels.max(1);
        let rate = opts.format.sample_rate;
        if rate == 0 {
            return Err(TalkerError::Audio("WAV-Quelle: Abtastrate 0".into()));
        }
        let mono = if rate == TARGET_SAMPLE_RATE || mono_16k.is_empty() {
            mono_16k
        } else {
            resample(&mono_16k, TARGET_SAMPLE_RATE, rate)?
        };
        let block_frames = opts.block_frames.max(1);
        let info = SourceInfo {
            format: AudioFormat {
                channels,
                sample_rate: rate,
            },
            device: format!("WAV: {name}"),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("talker-wav-source".into())
            .spawn(move || {
                let started = std::time::Instant::now();
                let mut frames_done: u64 = 0;
                for frames in mono.chunks(block_frames) {
                    frames_done += frames.len() as u64;
                    // Der Block gilt als geliefert, sobald seine letzte Frame aufgenommen ist.
                    let offset = opts.start_delay
                        + Duration::from_nanos(frames_done * 1_000_000_000 / u64::from(rate));
                    let at = match opts.pace {
                        Pace::Immediate => opts.epoch.saturating_add(offset),
                        Pace::RealTime => {
                            while started.elapsed() < offset {
                                if thread_stop.load(Ordering::SeqCst) {
                                    done.signal();
                                    return;
                                }
                                std::thread::park_timeout(
                                    offset
                                        .saturating_sub(started.elapsed())
                                        .min(Duration::from_millis(20)),
                                );
                            }
                            HostInstant::now()
                        }
                    };
                    if thread_stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let interleaved: Vec<f32> = frames
                        .iter()
                        .flat_map(|&s| std::iter::repeat_n(s, usize::from(channels)))
                        .collect();
                    on_block(&interleaved, at);
                }
                done.signal();
            })
            .map_err(|e| TalkerError::Audio(format!("WAV-Quelle: Thread-Start: {e}")))?;
        Ok(Opened {
            info,
            stop: Box::new(WavGuard {
                stop,
                handle: Some(handle),
            }),
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
        Arc::new(move |data, at| {
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

    /// Griff auf das erste Signal (fürs Aufnahme-Overlay).
    pub fn signal(&self) -> SignalHandle {
        SignalHandle(Arc::clone(&self.marks))
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

pub(crate) fn resample(mono: &[f32], from: u32, to: u32) -> Result<Vec<f32>> {
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

    /// PERF-2: I16-Konvertierung in einen wiederverwendeten Puffer — Werte stimmen, die
    /// Kapazität bleibt, kein neuer Speicher bei gleich großen oder kleineren Blöcken.
    #[test]
    fn i16_conversion_reuses_the_scratch_buffer() {
        let mut out = Vec::with_capacity(I16_SCRATCH_SAMPLES);
        i16_to_f32(&[0, 16_384, -32_768, 32_767], &mut out);
        assert_eq!(out, [0.0, 0.5, -1.0, 32_767.0 / 32_768.0]);
        let ptr = out.as_ptr();
        i16_to_f32(&[8_192; 4_000], &mut out);
        assert_eq!(out.len(), 4_000);
        assert_eq!(out.as_ptr(), ptr, "keine neue Allokation");
        i16_to_f32(&[], &mut out);
        assert!(out.is_empty() && out.capacity() >= I16_SCRATCH_SAMPLES);
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
        fn open(self: Box<Self>, on_block: OnBlock) -> Result<Opened> {
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
            fn open(self: Box<Self>, on_block: OnBlock) -> Result<Opened> {
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

    // ---- WavSource ----

    fn wav_opts(channels: u16, rate: u32, block_frames: usize, delay_ms: u64) -> WavOptions {
        WavOptions {
            format: AudioFormat {
                channels,
                sample_rate: rate,
            },
            block_frames,
            start_delay: Duration::from_millis(delay_ms),
            pace: Pace::Immediate,
            epoch: HostInstant::from_nanos(1_000_000),
        }
    }

    /// Öffnet die Quelle und sammelt alle Blöcke (Samples, Zeitstempel) bis `SourceDone`.
    fn collect(source: WavSource, done: &SourceDone) -> (Vec<Vec<f32>>, Vec<HostInstant>) {
        let got = Arc::new(Mutex::new((Vec::new(), Vec::new())));
        let sink = Arc::clone(&got);
        let opened = Box::new(source)
            .open(Arc::new(move |data, at| {
                let mut g = sink.lock().unwrap();
                g.0.push(data.to_vec());
                g.1.push(at);
            }))
            .unwrap();
        assert!(done.wait(Duration::from_secs(10)), "SourceDone blieb aus");
        drop(opened);
        let g = got.lock().unwrap();
        (g.0.clone(), g.1.clone())
    }

    fn ramp(n: usize) -> Vec<f32> {
        (0..n).map(|i| (i as f32 + 1.0) / 100_000.0).collect()
    }

    /// REQ-DIAG-024: bei 0/100/300 ms Startverzögerung geht kein Sample verloren (bitgleich).
    #[test]
    fn immediate_source_delivers_every_sample_for_each_start_delay() {
        let input = ramp(16_000);
        for delay_ms in [0, 100, 300] {
            let (source, done) =
                WavSource::from_samples(input.clone(), wav_opts(1, 16_000, 480, delay_ms));
            let (blocks, times) = collect(source, &done);
            let flat: Vec<f32> = blocks.concat();
            assert_eq!(flat, input, "Verzögerung {delay_ms} ms");
            assert!(
                times[0]
                    >= HostInstant::from_nanos(1_000_000)
                        .saturating_add(Duration::from_millis(delay_ms))
            );
        }
    }

    /// 44,1/48 kHz, 1/2 Kanäle: Frames·Kanäle konsistent, Dauer ±1 Block, Kanäle identisch.
    #[test]
    fn immediate_source_adapts_rate_and_channels() {
        let input = ramp(16_000);
        for (channels, rate) in [(1u16, 44_100u32), (2, 44_100), (1, 48_000), (2, 48_000)] {
            let block_frames = 1_024;
            let (source, done) =
                WavSource::from_samples(input.clone(), wav_opts(channels, rate, block_frames, 0));
            let (blocks, _) = collect(source, &done);
            let flat: Vec<f32> = blocks.concat();
            assert_eq!(flat.len() % usize::from(channels), 0);
            let frames = flat.len() / usize::from(channels);
            let expected = input.len() * rate as usize / 16_000;
            assert!(
                frames.abs_diff(expected) <= block_frames,
                "{channels} Kanäle {rate} Hz: {frames} statt ~{expected}"
            );
            if channels == 2 {
                assert!(flat.chunks_exact(2).all(|f| f[0] == f[1]));
            }
        }
    }

    #[test]
    fn block_size_edge_cases_and_empty_input() {
        let input = ramp(10);
        let (source, done) = WavSource::from_samples(input.clone(), wav_opts(1, 16_000, 1, 0));
        assert_eq!(collect(source, &done).0.len(), 10);
        let (source, done) =
            WavSource::from_samples(input.clone(), wav_opts(1, 16_000, 1_000_000, 0));
        let (blocks, _) = collect(source, &done);
        assert_eq!(blocks, std::slice::from_ref(&input));
        let (source, done) = WavSource::from_samples(input, wav_opts(1, 16_000, 0, 0));
        assert_eq!(
            collect(source, &done).0.len(),
            10,
            "Blockgröße 0 gilt als 1"
        );
        let (source, done) = WavSource::from_samples(vec![], wav_opts(1, 16_000, 480, 0));
        assert!(collect(source, &done).0.is_empty());
    }

    /// Virtuelle Zeitstempel sind streng monoton und beginnen frühestens bei epoch + Startverzögerung.
    #[test]
    fn immediate_timestamps_are_strictly_increasing_and_start_after_the_delay() {
        let (source, done) = WavSource::from_samples(ramp(16_000), wav_opts(1, 16_000, 160, 100));
        let (_, times) = collect(source, &done);
        assert_eq!(times.len(), 100);
        assert!(times.windows(2).all(|w| w[0] < w[1]));
        let epoch = HostInstant::from_nanos(1_000_000);
        assert!(times[0] >= epoch.saturating_add(Duration::from_millis(100)));
        assert_eq!(
            *times.last().unwrap(),
            epoch.saturating_add(Duration::from_millis(1_100))
        );
    }

    /// Im Echtzeittakt dauert die Lieferung mindestens ungefähr so lange wie der Clip.
    #[test]
    fn real_time_pace_takes_at_least_about_the_clip_duration() {
        let mut opts = wav_opts(1, 16_000, 160, 0);
        opts.pace = Pace::RealTime;
        let (source, done) = WavSource::from_samples(ramp(3_200), opts);
        let t0 = std::time::Instant::now();
        let (blocks, times) = collect(source, &done);
        assert!(
            t0.elapsed() >= Duration::from_millis(150),
            "{:?}",
            t0.elapsed()
        );
        assert_eq!(blocks.concat().len(), 3_200);
        assert!(times.windows(2).all(|w| w[0] <= w[1]));
    }

    /// `SourceDone` meldet sich nicht, solange die Quelle nie geöffnet wurde.
    #[test]
    fn source_done_stays_false_until_delivery_ends() {
        let (_source, done) = WavSource::from_samples(ramp(10), wav_opts(1, 16_000, 5, 0));
        assert!(!done.wait(Duration::from_millis(10)));
    }

    /// Drop von `Opened.stop` beendet die Lieferung: danach kommt kein Block mehr.
    #[test]
    fn dropping_stop_ends_delivery_without_late_blocks() {
        let mut opts = wav_opts(1, 16_000, 160, 0);
        opts.pace = Pace::RealTime;
        let (source, _done) = WavSource::from_samples(ramp(16_000 * 30), opts);
        let count = Arc::new(AtomicU32::new(0));
        let seen = Arc::clone(&count);
        let (tx, rx) = std::sync::mpsc::channel();
        let opened = Box::new(source)
            .open(Arc::new(move |_, _| {
                seen.fetch_add(1, Ordering::SeqCst);
                let _ = tx.send(());
            }))
            .unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let t0 = std::time::Instant::now();
        drop(opened);
        assert!(
            t0.elapsed() < Duration::from_secs(2),
            "Drop wartete auf den Clip"
        );
        let after = count.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(count.load(Ordering::SeqCst), after);
    }

    fn temp_wav(
        name: &str,
        bytes: Option<&[u8]>,
        spec: hound::WavSpec,
        samples: &[f32],
    ) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("talker-wav-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.wav"));
        match bytes {
            Some(b) => std::fs::write(&path, b).unwrap(),
            None => {
                let mut w = hound::WavWriter::create(&path, spec).unwrap();
                for &s in samples {
                    w.write_sample((s * 32767.0) as i16).unwrap();
                }
                w.finalize().unwrap();
            }
        }
        path
    }

    #[test]
    fn from_file_reads_a_wav_and_names_the_device() {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let samples = vec![0.0, 0.5, -0.5, 0.25];
        let path = temp_wav("ok", None, spec, &samples);
        let (source, done) = WavSource::from_file(&path, wav_opts(1, 16_000, 2, 0)).unwrap();
        let info_name = source.name.clone();
        let (blocks, _) = collect(source, &done);
        let flat = blocks.concat();
        assert_eq!(info_name, "ok");
        assert_eq!(flat.len(), 4);
        assert!((flat[1] - 0.5).abs() < 1e-3 && (flat[2] + 0.5).abs() < 1e-3);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Defekte oder fehlende WAV → Err mit Grund (Pfad im Text).
    #[test]
    fn from_file_reports_broken_and_missing_files() {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let broken = temp_wav("kaputt", Some(b"kein wav"), spec, &[]);
        let err = WavSource::from_file(&broken, wav_opts(1, 16_000, 160, 0))
            .err()
            .unwrap();
        assert!(err.to_string().contains("kaputt.wav"));
        let missing = broken.with_file_name("fehlt.wav");
        assert!(WavSource::from_file(&missing, wav_opts(1, 16_000, 160, 0)).is_err());
        let _ = std::fs::remove_dir_all(broken.parent().unwrap());
    }

    #[test]
    fn zero_sample_rate_fails_at_open_not_with_a_panic() {
        let (source, _done) = WavSource::from_samples(ramp(10), wav_opts(1, 0, 160, 0));
        assert!(Box::new(source).open(Arc::new(|_, _| {})).is_err());
    }

    /// Führende Null-Blöcke über `Recording` → erster Block < erstes Signal (AK-10 mit WavSource).
    #[test]
    fn wav_source_with_leading_zeros_marks_first_signal_after_first_block() {
        let mut input = vec![0.0; 3_200];
        input.extend(ramp(1_600));
        let (source, done) = WavSource::from_samples(input, wav_opts(1, 16_000, 160, 0));
        let rec = start(Box::new(source), None).unwrap();
        assert!(done.wait(Duration::from_secs(10)));
        let captured = rec.stop();
        assert!(captured.marks.first_block < captured.marks.first_signal);
        assert_eq!(captured.raw.len(), 4_800);
    }

    /// Quelle, die Blöcke erst auf Kommando liefert und jeden Block quittiert.
    struct GatedSource {
        blocks: std::sync::mpsc::Receiver<Vec<f32>>,
        acks: std::sync::mpsc::Sender<()>,
    }

    impl AudioSource for GatedSource {
        fn open(self: Box<Self>, on_block: OnBlock) -> Result<Opened> {
            std::thread::spawn(move || {
                while let Ok(block) = self.blocks.recv() {
                    on_block(&block, HostInstant::from_nanos(1));
                    let _ = self.acks.send(());
                }
            });
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

    fn gated() -> (
        Recording,
        std::sync::mpsc::Sender<Vec<f32>>,
        std::sync::mpsc::Receiver<()>,
    ) {
        let (block_tx, block_rx) = std::sync::mpsc::channel();
        let (ack_tx, ack_rx) = std::sync::mpsc::channel();
        let source = GatedSource {
            blocks: block_rx,
            acks: ack_tx,
        };
        (start(Box::new(source), None).unwrap(), block_tx, ack_rx)
    }

    /// REQ-DICT-016: erst der Block mit einem Sample ≠ 0 setzt `arrived()`; Clones teilen den Zustand.
    #[test]
    fn signal_arrives_with_the_first_nonzero_block_and_clones_share_it() {
        let (rec, blocks, acks) = gated();
        let signal = rec.signal();
        let clone = signal.clone();
        assert!(!signal.arrived(), "vor dem ersten Block");
        for _ in 0..2 {
            blocks.send(vec![0.0; 4]).unwrap();
            acks.recv().unwrap();
            assert!(!signal.arrived() && !clone.arrived(), "Null-Block");
        }
        blocks.send(vec![0.0, 1e-9]).unwrap();
        acks.recv().unwrap();
        assert!(signal.arrived() && clone.arrived());
        blocks.send(vec![0.0; 4]).unwrap();
        acks.recv().unwrap();
        assert!(signal.arrived(), "bleibt wahr");
    }

    #[test]
    fn signal_is_true_from_the_first_block_with_a_single_nonzero_sample() {
        let (rec, blocks, acks) = gated();
        blocks.send(vec![0.0, 0.0, 1e-9, 0.0]).unwrap();
        acks.recv().unwrap();
        assert!(rec.signal().arrived());
    }

    #[test]
    fn signal_stays_false_with_only_zero_blocks() {
        let (rec, blocks, acks) = gated();
        blocks.send(vec![0.0; 4]).unwrap();
        acks.recv().unwrap();
        blocks.send(Vec::new()).unwrap();
        acks.recv().unwrap();
        assert!(!rec.signal().arrived());
    }

    // ---- Gerätecache (MicSource) ----

    #[derive(Default)]
    struct World {
        default_uid: String,
        named: Vec<(String, String)>,
        formats: Vec<(String, MicFormat)>,
        gone: Vec<String>,
        find_by_name: usize,
        default_device: usize,
        uid: usize,
        format: usize,
        opens: usize,
        fail_opens: usize,
        streams: usize,
    }

    #[derive(Clone)]
    struct FakeBackend(Arc<Mutex<World>>);

    struct StreamGuard(Arc<Mutex<World>>);
    impl Drop for StreamGuard {
        fn drop(&mut self) {
            self.0.lock().unwrap().streams -= 1;
        }
    }

    fn fmt(rate: u32, channels: u16) -> MicFormat {
        MicFormat {
            channels,
            sample_rate: rate,
            sample_format: SampleFormat::F32,
        }
    }

    impl MicBackend for FakeBackend {
        type Device = String;
        fn default_device(&self) -> Option<String> {
            let mut w = self.0.lock().unwrap();
            w.default_device += 1;
            Some(w.default_uid.clone())
        }
        fn find_by_name(&self, name: &str) -> Option<String> {
            let mut w = self.0.lock().unwrap();
            w.find_by_name += 1;
            w.named
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, u)| u.clone())
        }
        fn uid(&self, d: &String) -> Result<String> {
            let mut w = self.0.lock().unwrap();
            w.uid += 1;
            if w.gone.contains(d) {
                return Err(TalkerError::Audio("weg".into()));
            }
            Ok(d.clone())
        }
        fn label(&self, d: &String) -> String {
            format!("Mic {d}")
        }
        fn format(&self, d: &String) -> Result<MicFormat> {
            let mut w = self.0.lock().unwrap();
            w.format += 1;
            Ok(w.formats
                .iter()
                .find(|(u, _)| u == d)
                .map_or(fmt(48_000, 1), |(_, f)| *f))
        }
        fn open(&self, _: &String, _: MicFormat, _: OnBlock) -> Result<Box<dyn Send>> {
            let mut w = self.0.lock().unwrap();
            w.opens += 1;
            if w.fail_opens > 0 {
                w.fail_opens -= 1;
                return Err(TalkerError::Audio("open scheitert".into()));
            }
            w.streams += 1;
            Ok(Box::new(StreamGuard(Arc::clone(&self.0))))
        }
    }

    fn world() -> (FakeBackend, MicCache<String>) {
        let w = World {
            default_uid: "built-in".into(),
            named: vec![("USB".into(), "usb".into())],
            ..World::default()
        };
        (FakeBackend(Arc::new(Mutex::new(w))), MicCache::default())
    }

    /// Ein Tastendruck: öffnen, Opened zurückgeben.
    fn press(b: &FakeBackend, cache: &MicCache<String>, wanted: Option<&str>) -> Opened {
        let src = MicSource::with_backend(wanted.map(String::from), cache.clone(), b.clone());
        Box::new(src).open(Arc::new(|_, _| {})).unwrap()
    }

    fn reset_counts(b: &FakeBackend) {
        let mut w = b.0.lock().unwrap();
        w.find_by_name = 0;
        w.default_device = 0;
        w.uid = 0;
        w.format = 0;
    }

    #[test]
    fn ak4_second_press_default_uses_cache_without_enumeration() {
        let (b, cache) = world();
        drop(press(&b, &cache, None));
        assert_eq!(b.0.lock().unwrap().streams, 0, "kein Stream nach Loslassen");
        reset_counts(&b);
        let o = press(&b, &cache, None);
        let w = b.0.lock().unwrap();
        assert_eq!(w.find_by_name, 0);
        assert!(w.default_device <= 1 && w.uid <= 1 && w.format <= 1);
        assert_eq!(o.info.device, "Mic built-in");
        assert_eq!(o.info.format.sample_rate, 48_000);
    }

    #[test]
    fn ak4_second_press_named_uses_cache_without_enumeration() {
        let (b, cache) = world();
        drop(press(&b, &cache, Some("USB")));
        reset_counts(&b);
        let o = press(&b, &cache, Some("USB"));
        let w = b.0.lock().unwrap();
        assert_eq!(w.find_by_name, 0);
        assert_eq!(w.default_device, 0);
        assert!(w.uid <= 1 && w.format <= 1);
        assert_eq!(o.info.device, "Mic usb");
    }

    #[test]
    fn ak4_no_stream_stays_open_between_recordings() {
        let (b, cache) = world();
        let o = press(&b, &cache, None);
        assert_eq!(b.0.lock().unwrap().streams, 1);
        drop(o);
        assert_eq!(b.0.lock().unwrap().streams, 0);
    }

    #[test]
    fn ak5a_changed_mic_device_uses_new_device() {
        let (b, cache) = world();
        drop(press(&b, &cache, None));
        let o = press(&b, &cache, Some("USB"));
        assert_eq!(o.info.device, "Mic usb");
    }

    #[test]
    fn ak5b_changed_default_device_is_picked_up() {
        let (b, cache) = world();
        drop(press(&b, &cache, None));
        b.0.lock().unwrap().default_uid = "airpods".into();
        let o = press(&b, &cache, None);
        assert_eq!(o.info.device, "Mic airpods");
    }

    #[test]
    fn ak5c_changed_format_is_picked_up() {
        let (b, cache) = world();
        drop(press(&b, &cache, None));
        b.0.lock()
            .unwrap()
            .formats
            .push(("built-in".into(), fmt(44_100, 2)));
        let o = press(&b, &cache, None);
        assert_eq!(o.info.format.sample_rate, 44_100);
        assert_eq!(o.info.format.channels, 2);
    }

    #[test]
    fn unplugged_named_device_is_rediscovered() {
        let (b, cache) = world();
        drop(press(&b, &cache, Some("USB")));
        {
            let mut w = b.0.lock().unwrap();
            w.gone.push("usb".into());
            w.named.clear();
        }
        reset_counts(&b);
        let o = press(&b, &cache, Some("USB"));
        assert_eq!(b.0.lock().unwrap().find_by_name, 1);
        assert_eq!(o.info.device, "Mic built-in");
    }

    #[test]
    fn missing_named_device_fallback_is_not_cached() {
        let (b, cache) = world();
        drop(press(&b, &cache, Some("Nope")));
        reset_counts(&b);
        drop(press(&b, &cache, Some("Nope")));
        assert_eq!(
            b.0.lock().unwrap().find_by_name,
            1,
            "jeder Druck sucht per Name"
        );
    }

    #[test]
    fn open_error_is_returned_and_leaves_no_stream() {
        let (b, cache) = world();
        b.0.lock().unwrap().fail_opens = 1;
        let src = MicSource::with_backend(None, cache, b.clone());
        assert!(Box::new(src).open(Arc::new(|_, _| {})).is_err());
        assert_eq!(b.0.lock().unwrap().streams, 0);
    }

    #[test]
    fn ak6_failed_open_with_cache_retries_once_with_rediscovery() {
        let (b, cache) = world();
        drop(press(&b, &cache, Some("USB")));
        reset_counts(&b);
        {
            let mut w = b.0.lock().unwrap();
            w.opens = 0;
            w.fail_opens = 1;
        }
        let o = press(&b, &cache, Some("USB"));
        let w = b.0.lock().unwrap();
        assert_eq!(w.opens, 2, "genau ein Neuversuch");
        assert_eq!(w.find_by_name, 1, "genau eine Neuermittlung");
        assert_eq!(w.streams, 1);
        assert_eq!(o.info.device, "Mic usb");
    }

    #[test]
    fn ak6_both_opens_fail_returns_err_without_third_attempt() {
        let (b, cache) = world();
        drop(press(&b, &cache, None));
        {
            let mut w = b.0.lock().unwrap();
            w.opens = 0;
            w.fail_opens = 5;
        }
        let src = MicSource::with_backend(None, cache, b.clone());
        assert!(Box::new(src).open(Arc::new(|_, _| {})).is_err());
        let w = b.0.lock().unwrap();
        assert_eq!(w.opens, 2, "höchstens 2 Versuche");
        assert_eq!(w.streams, 0);
    }

    #[test]
    fn ak6_first_press_without_cache_does_not_retry() {
        let (b, cache) = world();
        b.0.lock().unwrap().fail_opens = 5;
        let src = MicSource::with_backend(None, cache, b.clone());
        assert!(Box::new(src).open(Arc::new(|_, _| {})).is_err());
        assert_eq!(b.0.lock().unwrap().opens, 1);
    }

    #[test]
    fn ak6_failed_open_does_not_keep_failing_device_cached() {
        let (b, cache) = world();
        b.0.lock().unwrap().fail_opens = 1;
        let src = MicSource::with_backend(None, cache.clone(), b.clone());
        assert!(Box::new(src).open(Arc::new(|_, _| {})).is_err());
        reset_counts(&b);
        drop(press(&b, &cache, None));
        // Ohne Cache-Eintrag: volle Ermittlung, danach kein Neuversuch nötig.
        assert_eq!(b.0.lock().unwrap().opens, 2);
        assert!(cache.get().is_some());
    }
}
