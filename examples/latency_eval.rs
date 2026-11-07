//! Latenz-Messreihe (Ticket 0038): alle STT-Fixtures (say + FLEURS) je Am Ende/Satzweise ×
//! Roh/Natürlich, ohne echte Injection, durch dieselbe Pipeline wie die App
//! (`WavSource` → `audio::start` → `process_job`). Ausgabe: Markdown mit p50/p95.
//! Aufruf: `make latency-eval` (Variablen `N` = Läufe je Fall, Default 10; `LATENCY_OUT=<datei>`
//! schreibt die Tabelle zusätzlich in eine Datei). Braucht das Nemotron-Modell; ohne Gemma laufen
//! nur die Roh-Fälle, die Natürlich-Fälle stehen unter „Übersprungen“.

use std::cell::RefCell;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::channel;
use std::time::Duration;

use talker::audio::{
    self, AudioFormat, HostInstant, Pace, TARGET_SAMPLE_RATE, WavOptions, WavSource,
};
use talker::cleanup::{CleanupMode, GemmaCleaner, LlmCleaner};
use talker::config::{Config, InsertMode};
use talker::diagnostics::{DictationTrace, cleanup_name};
use talker::error::TalkerError;
use talker::models::{ModelState, ModelsState};
use talker::pipeline::{CleanerLoader, DictationWorker, LiveFeed, SttJob, process_job};
use talker::stt::{NemotronTranscriber, StreamingTranscriber, Transcriber};

/// Dieselbe Wortnormalisierung wie die Regression „erstes Wort“ (`tests/stt_nemotron.rs`).
#[path = "../tests/support/first_word.rs"]
#[allow(dead_code)]
mod first_word_support;
use first_word_support::normalize_words;

/// Läufe je Fall, wenn `N` nicht gesetzt ist; zusätzlich ein verworfener Aufwärmlauf.
const DEFAULT_RUNS: usize = 10;
/// Blockgröße der WAV-Quelle: 30 ms bei 16 kHz.
const BLOCK_FRAMES: usize = 480;

// ---------- reine Funktionen ----------

/// Nearest-Rank-Perzentil (`p` in 0..=1) auf unsortierten Werten; leer → `None`.
fn percentile(values: &[Duration], p: f64) -> Option<Duration> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort();
    let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    Some(sorted[rank - 1])
}

/// Führt `n + 1` Läufe aus und verwirft den ersten (Aufwärmlauf).
fn collect_runs<T>(n: usize, mut run: impl FnMut() -> T) -> Vec<T> {
    let _warmup = run();
    (0..n).map(|_| run()).collect()
}

/// Wortfehlerrate (Levenshtein über normalisierte Wörter / Referenzlänge).
/// Leere Referenz: 0 bei leerer Hypothese, sonst 1.
fn wer(reference: &str, hypothesis: &str) -> f64 {
    let r = normalize_words(reference);
    let h = normalize_words(hypothesis);
    if r.is_empty() {
        return if h.is_empty() { 0.0 } else { 1.0 };
    }
    let mut prev: Vec<usize> = (0..=h.len()).collect();
    let mut curr = vec![0; h.len() + 1];
    for (i, rw) in r.iter().enumerate() {
        curr[0] = i + 1;
        for (j, hw) in h.iter().enumerate() {
            let sub = prev[j] + usize::from(rw != hw);
            curr[j + 1] = sub.min(prev[j + 1] + 1).min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[h.len()] as f64 / r.len() as f64
}

/// `N` aus der Umgebung: leer/nicht gesetzt → Default; sonst eine positive Zahl.
fn parse_runs(value: Option<&str>) -> Result<usize, String> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(DEFAULT_RUNS),
        Some(v) => v
            .parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| format!("N=\"{v}\" ungültig — erwartet eine positive Zahl")),
    }
}

/// Eine Testdatei: WAV + Referenztext.
#[derive(Debug)]
struct Fixture {
    name: String,
    path: PathBuf,
    reference: String,
}

/// Lädt eine Fixture; defekt (kaputte/leere WAV, fehlende Referenz) → Grund als Text.
fn load_fixture(wav: &Path) -> Result<Fixture, String> {
    let name = wav
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let reader = hound::WavReader::open(wav).map_err(|e| format!("WAV nicht lesbar ({e})"))?;
    if reader.len() == 0 {
        return Err("WAV ist leer".into());
    }
    let reference = std::fs::read_to_string(wav.with_extension("ref.txt"))
        .map_err(|e| format!(".ref.txt fehlt ({e})"))?;
    if reference.trim().is_empty() {
        return Err(".ref.txt ist leer".into());
    }
    Ok(Fixture {
        name,
        path: wav.to_path_buf(),
        reference,
    })
}

/// Alle `*.wav` im Verzeichnis, sortiert; Verzeichnis fehlt → Fehler.
fn list_wavs(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut wavs: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("Fixture-Verzeichnis {} nicht lesbar ({e})", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "wav"))
        .collect();
    wavs.sort();
    Ok(wavs)
}

/// Messwerte eines einzelnen Laufs.
#[derive(Debug, Default, Clone)]
struct RunResult {
    release_to_text: Option<Duration>,
    /// `Loslassen→Text` je Sekunde aufgenommenes Audio (die Latenz am Ende wächst mit der Clip-Länge).
    per_audio_second: Option<Duration>,
    stt: Option<Duration>,
    cleanup: Option<Duration>,
    first_segment: Option<Duration>,
    text: String,
}

/// Latenz bezogen auf eine Sekunde Audio; Audiodauer 0 → `None`.
fn per_audio_second(latency: Duration, recorded: Duration) -> Option<Duration> {
    (!recorded.is_zero()).then(|| latency.div_f64(recorded.as_secs_f64()))
}

impl RunResult {
    fn from_trace(trace: &DictationTrace, text: String) -> Self {
        let release_to_text = match (trace.text_at, trace.released_at) {
            (Some(text_at), Some(released)) => Some(text_at.saturating_since(released)),
            _ => None,
        };
        Self {
            release_to_text,
            per_audio_second: release_to_text
                .zip(trace.recorded)
                .and_then(|(latency, recorded)| per_audio_second(latency, recorded)),
            stt: trace.stt,
            cleanup: trace.cleanup,
            first_segment: trace
                .first_segment_at
                .map(|at| at.saturating_since(trace.pressed_at)),
            text,
        }
    }
}

/// Sammelt die Läufe eines Falls (Einfüge-Verhalten × Cleanup-Modus) über alle Fixtures.
#[derive(Default)]
struct ComboStats {
    release_to_text: Vec<Duration>,
    per_audio_second: Vec<Duration>,
    stt: Vec<Duration>,
    cleanup: Vec<Duration>,
    first_segment: Vec<Duration>,
    /// WER je Fixture (aus dem ersten gemessenen Lauf).
    wers: Vec<f64>,
}

impl ComboStats {
    fn add_run(&mut self, run: &RunResult) {
        self.release_to_text.extend(run.release_to_text);
        self.per_audio_second.extend(run.per_audio_second);
        self.stt.extend(run.stt);
        self.cleanup.extend(run.cleanup);
        self.first_segment.extend(run.first_segment);
    }

    fn mean_wer(&self) -> Option<f64> {
        (!self.wers.is_empty()).then(|| self.wers.iter().sum::<f64>() / self.wers.len() as f64)
    }
}

struct Combo {
    insert: InsertMode,
    cleanup: CleanupMode,
    stats: ComboStats,
}

/// Alles, was die Tabelle braucht.
struct Report {
    combos: Vec<Combo>,
    runs: usize,
    cold_loads: Vec<(&'static str, Duration)>,
    skipped: Vec<(String, String)>,
    commit: String,
    date: String,
    device: String,
}

fn cell(d: Option<Duration>) -> String {
    d.map_or_else(|| "—".to_string(), |d| format!("{} ms", d.as_millis()))
}

fn rate_cell(d: Option<Duration>) -> String {
    d.map_or_else(|| "—".to_string(), |d| format!("{} ms/s", d.as_millis()))
}

fn wer_cell(w: Option<f64>) -> String {
    w.map_or_else(|| "—".to_string(), |w| format!("{:.1} %", w * 100.0))
}

/// Markdown-Tabelle mit p50/p95 je Fall, Kaltlade-Dauer je Modell, Commit, Datum, Gerät.
fn render_table(report: &Report) -> String {
    let mut out = String::new();
    out.push_str("# Latenz-Messreihe\n\n");
    out.push_str(&format!(
        "- Commit: `{}`\n- Datum: {}\n- Gerät: {}\n- Läufe je Fall: {} (+1 verworfener Aufwärmlauf)\n",
        report.commit, report.date, report.device, report.runs
    ));
    for (model, took) in &report.cold_loads {
        out.push_str(&format!(
            "- Kaltlade-Dauer {model}: {:.1} s\n",
            took.as_secs_f64()
        ));
    }
    out.push_str(
        "\n| Einfügen | Cleanup | Loslassen→Text p50 | p95 | je Audio-s p50 | p95 | STT p50 | p95 | Cleanup p50 | p95 | →erstes Segment p50 | p95 | WER |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for combo in &report.combos {
        let s = &combo.stats;
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            combo.insert.label(),
            cleanup_name(combo.cleanup),
            cell(percentile(&s.release_to_text, 0.5)),
            cell(percentile(&s.release_to_text, 0.95)),
            rate_cell(percentile(&s.per_audio_second, 0.5)),
            rate_cell(percentile(&s.per_audio_second, 0.95)),
            cell(percentile(&s.stt, 0.5)),
            cell(percentile(&s.stt, 0.95)),
            cell(percentile(&s.cleanup, 0.5)),
            cell(percentile(&s.cleanup, 0.95)),
            cell(percentile(&s.first_segment, 0.5)),
            cell(percentile(&s.first_segment, 0.95)),
            wer_cell(s.mean_wer()),
        ));
    }
    if !report.skipped.is_empty() {
        out.push_str("\nÜbersprungen:\n\n");
        for (name, reason) in &report.skipped {
            out.push_str(&format!("- {name}: übersprungen: {reason}\n"));
        }
    }
    out
}

/// Tabelle auf stdout und, wenn `out` gesetzt ist, zusätzlich in die Datei.
fn emit(table: &str, out: Option<&Path>, stdout: &mut dyn Write) -> Result<(), String> {
    writeln!(stdout, "{table}").map_err(|e| format!("stdout: {e}"))?;
    if let Some(path) = out {
        std::fs::write(path, table)
            .map_err(|e| format!("LATENCY_OUT {} nicht schreibbar ({e})", path.display()))?;
    }
    Ok(())
}

// ---------- Messlauf ----------

/// Ein Lauf über die App-Pipeline; Live-Fälle im Echtzeittakt, Am Ende sofort.
/// `inject` zählt/merkt nur — es wird nie echt eingefügt.
fn run_once<T, C>(
    worker: &mut DictationWorker<T, C>,
    fixture: &Fixture,
    insert: InsertMode,
    cleanup: CleanupMode,
) -> Result<RunResult, String>
where
    T: Transcriber + StreamingTranscriber,
    C: LlmCleaner,
{
    let cfg = Config {
        insert_mode: insert,
        cleanup_mode: cleanup,
        ..Config::default()
    };
    let live = insert != InsertMode::AtEnd;
    let pressed_at = HostInstant::now();
    let options = WavOptions {
        format: AudioFormat {
            channels: 1,
            sample_rate: TARGET_SAMPLE_RATE,
        },
        block_frames: BLOCK_FRAMES,
        start_delay: Duration::ZERO,
        pace: if live {
            Pace::RealTime
        } else {
            Pace::Immediate
        },
        epoch: pressed_at,
    };
    let (source, done) = WavSource::from_file(&fixture.path, options).map_err(|e| e.to_string())?;
    let (tx, rx) = channel();
    let recording =
        audio::start(Box::new(source), live.then_some(tx)).map_err(|e| e.to_string())?;
    let mut trace = DictationTrace::new(1, pressed_at, insert, cleanup);
    trace.mic_ready_at = Some(HostInstant::now());

    let mut injected = String::new();
    let mut inject = |text: &str| -> Result<(), TalkerError> {
        injected.push_str(text);
        Ok(())
    };
    let wait = Duration::from_secs(600);
    let (_, trace) = if live {
        // Das Loslassen kommt, wenn die Quelle fertig ist; der Worker verarbeitet währenddessen.
        let release = std::thread::spawn(move || {
            done.wait(wait);
            recording.finish_live(HostInstant::now());
        });
        let feed = LiveFeed {
            trace: Box::new(trace),
            frontmost: None,
            blocks: rx,
            format: (1, TARGET_SAMPLE_RATE),
        };
        let result = process_job(worker, SttJob::Live { feed }, &cfg, &mut inject, None);
        release
            .join()
            .map_err(|_| "Loslass-Thread abgestürzt".to_string())?;
        result
    } else {
        if !done.wait(wait) {
            return Err("Quelle wurde nicht fertig".into());
        }
        let released_at = HostInstant::now();
        let captured = recording.stop();
        trace.apply_capture(released_at, &captured);
        let pcm = captured.to_mono_16k().map_err(|e| e.to_string())?;
        let job = SttJob::Utterance {
            pcm,
            frontmost: None,
            captured,
            trace: Box::new(trace),
        };
        process_job(worker, job, &cfg, &mut inject, None)
    };
    Ok(RunResult::from_trace(&trace, injected))
}

/// Alle Fälle über alle Fixtures: Aufwärmlauf verwerfen, `runs` Läufe messen.
fn measure<T, C>(
    worker: &mut DictationWorker<T, C>,
    fixtures: &[Fixture],
    runs: usize,
    cleanups: &[CleanupMode],
    progress: &mut dyn FnMut(&str),
) -> (Vec<Combo>, Vec<(String, String)>)
where
    T: Transcriber + StreamingTranscriber,
    C: LlmCleaner,
{
    let mut combos: Vec<Combo> = [InsertMode::AtEnd, InsertMode::PerSentence]
        .into_iter()
        .flat_map(|insert| {
            cleanups.iter().map(move |&cleanup| Combo {
                insert,
                cleanup,
                stats: ComboStats::default(),
            })
        })
        .collect();
    let mut skipped = Vec::new();
    for fixture in fixtures {
        for combo in &mut combos {
            progress(&format!(
                "{} · {} · {}",
                fixture.name,
                combo.insert.label(),
                cleanup_name(combo.cleanup)
            ));
            let mut failure = None;
            let results = collect_runs(runs, || {
                match run_once(worker, fixture, combo.insert, combo.cleanup) {
                    Ok(run) => run,
                    Err(e) => {
                        failure.get_or_insert(e);
                        RunResult::default()
                    }
                }
            });
            if let Some(reason) = failure {
                skipped.push((fixture.name.clone(), reason));
                continue;
            }
            for run in &results {
                combo.stats.add_run(run);
            }
            if let Some(first) = results.first() {
                combo.stats.wers.push(wer(&fixture.reference, &first.text));
            }
        }
    }
    (combos, skipped)
}

// ---------- Umgebung ----------

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn device_description() -> String {
    let brand = command_output("sysctl", &["-n", "machdep.cpu.brand_string"])
        .unwrap_or_else(|| "unbekannt".into());
    let cores = std::thread::available_parallelism().map_or(0, |n| n.get());
    format!("{brand} ({cores} Kerne)")
}

struct EvalConfig {
    nemotron_dir: PathBuf,
    gemma_path: PathBuf,
    fixtures_dir: PathBuf,
    runs: usize,
    out: Option<PathBuf>,
}

/// Die geladenen Modelle samt Kaltlade-Dauer; Gemma ist optional (ohne Gemma nur Roh-Fälle).
struct Models {
    stt: NemotronTranscriber,
    /// `Err` = Grund, warum die Natürlich-Fälle fehlen (Gemma nicht ladbar).
    cleaner: Result<GemmaCleaner, String>,
    cold_loads: Vec<(&'static str, Duration)>,
}

/// Lädt Gemma; fehlt es, gibt es statt eines Abbruchs den Hinweis für „Übersprungen“.
fn load_cleaner(path: &Path) -> Result<GemmaCleaner, String> {
    GemmaCleaner::new(path)
        .map_err(|e| format!("Gemma nicht verfügbar ({e}) — Natürlich-Fälle übersprungen"))
}

/// Die Cleanup-Modi der Messreihe: Roh immer, Natürlich nur mit Gemma.
fn cleanup_modes(has_cleaner: bool) -> Vec<CleanupMode> {
    let mut modes = vec![CleanupMode::Raw];
    if has_cleaner {
        modes.push(CleanupMode::Casual);
    }
    modes
}

/// Lädt die Modelle (Kaltlade-Dauer gemessen); Nemotron ist Pflicht → Fehler mit Pfad-Hinweis,
/// kein Panic. Gemma fehlt → nur die Roh-Fälle laufen.
fn load_models(cfg: &EvalConfig) -> Result<Models, String> {
    let t0 = std::time::Instant::now();
    let stt = NemotronTranscriber::new(&cfg.nemotron_dir).map_err(|e| e.to_string())?;
    let mut cold_loads = vec![("Nemotron (STT)", t0.elapsed())];
    let t1 = std::time::Instant::now();
    let cleaner = load_cleaner(&cfg.gemma_path);
    if cleaner.is_ok() {
        cold_loads.push(("Gemma (Cleanup)", t1.elapsed()));
    }
    Ok(Models {
        stt,
        cleaner,
        cold_loads,
    })
}

/// Geteiltes Handle auf die einmal geladene Cleaner-Instanz (REQ-DIAG-034): Der Worker verwirft
/// beim Roh-Fall nur sein Handle, die Instanz bleibt für die Natürlich-Fälle erhalten.
struct SharedCleaner<C>(Rc<RefCell<C>>);

impl<C> Clone for SharedCleaner<C> {
    fn clone(&self) -> Self {
        Self(Rc::clone(&self.0))
    }
}

impl<C: LlmCleaner> LlmCleaner for SharedCleaner<C> {
    fn clean(&mut self, raw: &str) -> talker::error::Result<String> {
        self.0.borrow_mut().clean(raw)
    }
    fn set_mode(&mut self, mode: CleanupMode) {
        self.0.borrow_mut().set_mode(mode);
    }
    fn set_vocab(&mut self, vocab: &[String]) {
        self.0.borrow_mut().set_vocab(vocab);
    }
}

/// Loader für den Worker: reicht Handles auf die beim Start geladene Instanz aus und lädt nie nach.
fn shared_cleaner_loader<C: LlmCleaner + 'static>(
    cleaner: Option<C>,
) -> CleanerLoader<SharedCleaner<C>> {
    let shared = cleaner.map(|c| SharedCleaner(Rc::new(RefCell::new(c))));
    Box::new(move || (shared.clone(), false))
}

fn run(cfg: &EvalConfig, stdout: &mut dyn Write) -> Result<(), String> {
    let Models {
        stt,
        cleaner,
        cold_loads,
    } = load_models(cfg)?;
    let has_cleaner = cleaner.is_ok();
    let mut skipped = Vec::new();
    let cleaner = match cleaner {
        Ok(cleaner) => Some(cleaner),
        Err(reason) => {
            skipped.push((cleanup_name(CleanupMode::Casual).to_string(), reason));
            None
        }
    };
    let mut fixtures = Vec::new();
    for wav in list_wavs(&cfg.fixtures_dir)? {
        match load_fixture(&wav) {
            Ok(f) => fixtures.push(f),
            Err(reason) => skipped.push((
                wav.file_stem()
                    .map_or_else(String::new, |s| s.to_string_lossy().into_owned()),
                reason,
            )),
        }
    }
    if fixtures.is_empty() {
        return Err(format!(
            "keine brauchbaren Fixtures in {} — scripts/gen_stt_fixtures.sh bzw. scripts/fetch_fleurs_fixtures.sh ausführen",
            cfg.fixtures_dir.display()
        ));
    }
    // Natürlich braucht den Cleaner schon beim Erzeugen des Workers (Vorladen).
    let preload = Config {
        cleanup_mode: if has_cleaner {
            CleanupMode::Casual
        } else {
            CleanupMode::Raw
        },
        ..Config::default()
    };
    let mut stt_slot = Some(stt);
    let mut worker = DictationWorker::new(
        &preload,
        Arc::new(ModelsState::new(
            ModelState::Ready,
            if has_cleaner {
                ModelState::Ready
            } else {
                ModelState::Missing
            },
        )),
        Box::new(move |_| {
            stt_slot
                .take()
                .ok_or_else(|| TalkerError::Stt("Nemotron nur einmal geladen".into()))
        }),
        shared_cleaner_loader(cleaner),
    );
    let mut progress = |label: &str| eprintln!("… {label}");
    let (combos, run_skipped) = measure(
        &mut worker,
        &fixtures,
        cfg.runs,
        &cleanup_modes(has_cleaner),
        &mut progress,
    );
    skipped.extend(run_skipped);
    let report = Report {
        combos,
        runs: cfg.runs,
        cold_loads,
        skipped,
        commit: command_output("git", &["rev-parse", "--short", "HEAD"])
            .unwrap_or_else(|| "unbekannt".into()),
        date: command_output("date", &["+%Y-%m-%d"]).unwrap_or_else(|| "unbekannt".into()),
        device: device_description(),
    };
    emit(&render_table(&report), cfg.out.as_deref(), stdout)
}

fn main() -> ExitCode {
    let runs = match parse_runs(std::env::var("N").ok().as_deref()) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("latency-eval: {e}");
            return ExitCode::FAILURE;
        }
    };
    let cfg = EvalConfig {
        nemotron_dir: NemotronTranscriber::default_model_dir(),
        gemma_path: GemmaCleaner::default_model_path(),
        fixtures_dir: PathBuf::from("tests/fixtures/stt"),
        runs,
        out: std::env::var_os("LATENCY_OUT")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from),
    };
    match run(&cfg, &mut std::io::stdout()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("latency-eval: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use talker::stt::StreamUpdate;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// REQ-DIAG-025: p50/p95 für N=1, gerade/ungerade, unsortiert, leer.
    #[test]
    fn percentiles_use_nearest_rank() {
        assert_eq!(percentile(&[], 0.5), None);
        assert_eq!(percentile(&[ms(7)], 0.5), Some(ms(7)));
        assert_eq!(percentile(&[ms(7)], 0.95), Some(ms(7)));
        let ten: Vec<_> = (1..=10).rev().map(|n| ms(n * 10)).collect();
        assert_eq!(percentile(&ten, 0.5), Some(ms(50)));
        assert_eq!(percentile(&ten, 0.95), Some(ms(100)));
        let odd = [ms(30), ms(10), ms(20)];
        assert_eq!(percentile(&odd, 0.5), Some(ms(20)));
        assert_eq!(percentile(&odd, 0.95), Some(ms(30)));
        let even = [ms(40), ms(10), ms(30), ms(20)];
        assert_eq!(percentile(&even, 0.5), Some(ms(20)));
        assert_eq!(percentile(&even, 0.0), Some(ms(10)));
    }

    /// REQ-DIAG-025: ein Aufwärmlauf wird verworfen — n gemessene Werte, n + 1 Läufe.
    #[test]
    fn warmup_run_is_discarded() {
        let calls = std::cell::Cell::new(0);
        let values = collect_runs(3, || {
            calls.set(calls.get() + 1);
            calls.get()
        });
        assert_eq!(calls.get(), 4);
        assert_eq!(values, [2, 3, 4]);
        assert_eq!(collect_runs(1, || 9), [9]);
    }

    #[test]
    fn wer_handles_edges_and_normalization() {
        assert_eq!(wer("", ""), 0.0);
        assert_eq!(wer("", "etwas"), 1.0);
        assert_eq!(wer("Hallo, Welt!", "hallo welt"), 0.0);
        assert_eq!(wer("eins zwei", ""), 1.0);
        assert!((wer("a b c d", "a x c d") - 0.25).abs() < 1e-9);
    }

    /// PERF-3: Latenz relativ zur Audiodauer; Dauer 0 → kein Wert (keine Division durch 0).
    #[test]
    fn latency_per_audio_second_scales_with_the_clip_length() {
        assert_eq!(
            per_audio_second(ms(780), Duration::from_secs(3)),
            Some(ms(260))
        );
        assert_eq!(per_audio_second(ms(780), ms(12_000)), Some(ms(65)));
        assert_eq!(
            per_audio_second(Duration::ZERO, Duration::from_secs(3)),
            Some(Duration::ZERO)
        );
        assert_eq!(per_audio_second(ms(780), Duration::ZERO), None);
    }

    /// PERF-1: fehlt Gemma, laufen nur die Roh-Fälle; der Grund steht unter „Übersprungen“.
    #[test]
    fn missing_gemma_leaves_only_the_raw_cases_with_a_note() {
        let reason = load_cleaner(Path::new("/nicht/vorhanden/gemma.gguf"))
            .err()
            .unwrap();
        // PERF-4: der Pfad steht genau einmal im Hinweis.
        assert_eq!(
            reason.matches("/nicht/vorhanden/gemma.gguf").count(),
            1,
            "{reason}"
        );
        assert_eq!(cleanup_modes(false), [CleanupMode::Raw]);
        assert_eq!(cleanup_modes(true), [CleanupMode::Raw, CleanupMode::Casual]);

        let fx = TempFixtures::new("ohne-gemma");
        fx.wav("clip", &[1_000; 9_600], Some("hallo welt"));
        let fixture = load_fixture(&fx.0.join("clip.wav")).unwrap();
        let mut worker = fake_worker();
        let (combos, skipped) = measure(
            &mut worker,
            &[fixture],
            1,
            &cleanup_modes(false),
            &mut |_| {},
        );
        assert_eq!(combos.len(), 2);
        assert!(combos.iter().all(|c| c.cleanup == CleanupMode::Raw));
        assert!(skipped.is_empty());
    }

    #[test]
    fn runs_come_from_n_with_a_default() {
        assert_eq!(parse_runs(None), Ok(10));
        assert_eq!(parse_runs(Some("")), Ok(10));
        assert_eq!(parse_runs(Some(" 3 ")), Ok(3));
        assert_eq!(parse_runs(Some("1")), Ok(1));
        assert!(parse_runs(Some("0")).is_err());
        assert!(parse_runs(Some("viele")).is_err());
        assert!(parse_runs(Some("-2")).is_err());
    }

    fn sample_report() -> Report {
        let mut stats = ComboStats::default();
        stats.add_run(&RunResult {
            release_to_text: Some(ms(780)),
            stt: Some(ms(520)),
            cleanup: Some(ms(240)),
            per_audio_second: Some(ms(260)),
            first_segment: Some(ms(1_400)),
            text: String::new(),
        });
        stats.wers.push(0.125);
        Report {
            combos: vec![
                Combo {
                    insert: InsertMode::PerSentence,
                    cleanup: CleanupMode::Casual,
                    stats,
                },
                Combo {
                    insert: InsertMode::AtEnd,
                    cleanup: CleanupMode::Raw,
                    stats: ComboStats::default(),
                },
            ],
            runs: 10,
            cold_loads: vec![
                ("Nemotron (STT)", ms(1_900)),
                ("Gemma (Cleanup)", ms(3_200)),
            ],
            skipped: vec![("kaputt".into(), "WAV ist leer".into())],
            commit: "abc1234".into(),
            date: "2026-11-07".into(),
            device: "Apple M3 (8 Kerne)".into(),
        }
    }

    /// REQ-DIAG-025/-026: Spalten, Kaltlade, Commit, Datum, Gerät, Übersprungenes.
    #[test]
    fn table_contains_all_columns_and_header_facts() {
        let table = render_table(&sample_report());
        for needle in [
            "Loslassen→Text p50 | p95 | je Audio-s p50 | p95",
            "STT p50 | p95",
            "Cleanup p50 | p95",
            "→erstes Segment p50 | p95",
            "WER",
            "Kaltlade-Dauer Nemotron (STT): 1.9 s",
            "Kaltlade-Dauer Gemma (Cleanup): 3.2 s",
            "`abc1234`",
            "2026-11-07",
            "Apple M3 (8 Kerne)",
            "Läufe je Fall: 10",
            "| Satzweise | Natürlich | 780 ms | 780 ms | 260 ms/s | 260 ms/s | 520 ms | 520 ms | 240 ms | 240 ms | 1400 ms | 1400 ms | 12.5 % |",
            "| Am Ende | Roh | — | — | — | — | — | — | — | — | — | — | — |",
            "kaputt: übersprungen: WAV ist leer",
        ] {
            assert!(table.contains(needle), "Tabelle ohne: {needle}\n{table}");
        }
    }

    /// REQ-DIAG-027: `LATENCY_OUT` schreibt zusätzlich in die Datei, stdout bleibt.
    #[test]
    fn output_file_is_written_in_addition_to_stdout() {
        let dir = std::env::temp_dir().join(format!("talker-latency-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("messung.md");
        let mut stdout = Vec::new();
        emit("# Tabelle", Some(&file), &mut stdout).unwrap();
        assert!(String::from_utf8(stdout).unwrap().contains("# Tabelle"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "# Tabelle");
        let mut stdout = Vec::new();
        emit("# nur stdout", None, &mut stdout).unwrap();
        assert!(!stdout.is_empty());
        let bad = dir.join("fehlt/ordner/messung.md");
        assert!(
            emit("x", Some(&bad), &mut Vec::new())
                .unwrap_err()
                .contains("LATENCY_OUT")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// REQ-DIAG-028: Modell fehlt → Fehler mit Pfad-Hinweis (kein Panic), `main` → Exit ≠ 0.
    #[test]
    fn missing_model_is_an_error_with_the_path() {
        let cfg = EvalConfig {
            nemotron_dir: PathBuf::from("/nicht/vorhanden/nemotron"),
            gemma_path: PathBuf::from("/nicht/vorhanden/gemma.gguf"),
            fixtures_dir: PathBuf::from("tests/fixtures/stt"),
            runs: 1,
            out: None,
        };
        let err = run(&cfg, &mut Vec::new()).unwrap_err();
        assert!(err.contains("/nicht/vorhanden/nemotron"), "{err}");
    }

    struct TempFixtures(PathBuf);
    impl TempFixtures {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("talker-latency-fx-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn wav(&self, name: &str, samples: &[i16], reference: Option<&str>) -> PathBuf {
            let path = self.0.join(format!("{name}.wav"));
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut writer = hound::WavWriter::create(&path, spec).unwrap();
            for &s in samples {
                writer.write_sample(s).unwrap();
            }
            writer.finalize().unwrap();
            if let Some(text) = reference {
                std::fs::write(path.with_extension("ref.txt"), text).unwrap();
            }
            path
        }
    }
    impl Drop for TempFixtures {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// REQ-DIAG-029: defekte Fixture → Grund, Lauf geht weiter.
    #[test]
    fn broken_fixtures_are_skipped_with_a_reason() {
        let fx = TempFixtures::new("defekt");
        let ok = fx.wav("gut", &[100; 1_600], Some("hallo welt"));
        let empty = fx.wav("leer", &[], Some("egal"));
        let no_ref = fx.wav("ohne-ref", &[100; 1_600], None);
        let blank_ref = fx.wav("leere-ref", &[100; 1_600], Some("  "));
        let broken = fx.0.join("kaputt.wav");
        std::fs::write(&broken, "kein wav").unwrap();
        assert_eq!(load_fixture(&ok).unwrap().reference, "hallo welt");
        assert!(load_fixture(&empty).unwrap_err().contains("leer"));
        assert!(
            load_fixture(&no_ref)
                .unwrap_err()
                .contains(".ref.txt fehlt")
        );
        assert!(load_fixture(&blank_ref).unwrap_err().contains("leer"));
        assert!(load_fixture(&broken).unwrap_err().contains("nicht lesbar"));
        let listed: Vec<_> = list_wavs(&fx.0).unwrap();
        assert_eq!(listed.len(), 5);
        assert!(list_wavs(&fx.0.join("gibt-es-nicht")).is_err());
    }

    /// STT-/Cleanup-Fake für den Messlauf in der CI.
    struct EvalStt {
        updates: VecDeque<StreamUpdate>,
        reply: &'static str,
    }

    impl Transcriber for EvalStt {
        fn transcribe(&mut self, _: &[f32]) -> talker::error::Result<String> {
            Ok(self.reply.to_string())
        }
    }

    impl StreamingTranscriber for EvalStt {
        fn begin(&mut self) {}
        fn feed(&mut self, _: &[f32]) -> talker::error::Result<StreamUpdate> {
            Ok(self.updates.pop_front().unwrap_or(StreamUpdate {
                text: String::new(),
                endpoint: false,
            }))
        }
        fn finish(&mut self) -> talker::error::Result<String> {
            Ok(self.reply.to_string())
        }
    }

    struct BracketCleaner;
    impl LlmCleaner for BracketCleaner {
        fn clean(&mut self, raw: &str) -> talker::error::Result<String> {
            Ok(format!("{raw}!"))
        }
    }

    fn fake_worker() -> DictationWorker<EvalStt, BracketCleaner> {
        fake_worker_with(Box::new(|| (Some(BracketCleaner), false)))
    }

    fn fake_worker_with<C: LlmCleaner>(
        load_cleaner: CleanerLoader<C>,
    ) -> DictationWorker<EvalStt, C> {
        let cfg = Config::default();
        DictationWorker::new(
            &cfg,
            Arc::new(ModelsState::new(ModelState::Ready, ModelState::Ready)),
            Box::new(|_| {
                Ok(EvalStt {
                    updates: [("a", false), ("Erster Satz.", true)]
                        .iter()
                        .map(|(text, endpoint)| StreamUpdate {
                            text: (*text).to_string(),
                            endpoint: *endpoint,
                        })
                        .collect(),
                    reply: "hallo welt",
                })
            }),
            load_cleaner,
        )
    }

    /// Messlauf in der CI: Am Ende und Satzweise liefern Messwerte und Text, ohne echte Injection.
    #[test]
    fn measurement_loop_runs_with_fakes_and_never_injects_for_real() {
        let fx = TempFixtures::new("lauf");
        // 1,2 s Audio, damit Live mindestens zwei 560-ms-Chunks bekommt.
        fx.wav("clip", &[1_000; 19_200], Some("hallo welt"));
        let fixture = load_fixture(&fx.0.join("clip.wav")).unwrap();
        let mut worker = fake_worker();
        let batch = run_once(&mut worker, &fixture, InsertMode::AtEnd, CleanupMode::Raw).unwrap();
        assert_eq!(batch.text, "hallo welt");
        assert!(batch.release_to_text.is_some() && batch.stt.is_some());
        assert!(batch.per_audio_second.is_some());
        assert_eq!(batch.cleanup, None, "Roh hat keinen Cleanup");
        assert_eq!(batch.first_segment, None, "Am Ende hat kein Segment");

        let mut worker = fake_worker();
        let live = run_once(
            &mut worker,
            &fixture,
            InsertMode::PerSentence,
            CleanupMode::Raw,
        )
        .unwrap();
        assert!(live.text.contains("Erster Satz."), "{}", live.text);
        assert!(live.first_segment.is_some());

        let mut worker = fake_worker();
        let cleaned = run_once(
            &mut worker,
            &fixture,
            InsertMode::AtEnd,
            CleanupMode::Casual,
        )
        .unwrap();
        assert_eq!(cleaned.text, "hallo welt!");
        assert!(cleaned.cleanup.is_some());
    }

    /// AK-22 (PERF-5), REQ-DIAG-034 AK-2: Natürlich-Fälle haben Cleanup-Werte, auch nachdem Roh
    /// das Cleaner-Handle verworfen hat.
    #[test]
    fn casual_cases_get_cleanup_after_raw_dropped_the_cleaner() {
        let fx = TempFixtures::new("natuerlich");
        fx.wav("clip", &[1_000; 9_600], Some("hallo welt"));
        let fixture = load_fixture(&fx.0.join("clip.wav")).unwrap();
        let mut worker = fake_worker_with(shared_cleaner_loader(Some(BracketCleaner)));
        let (combos, skipped) = measure(
            &mut worker,
            &[fixture],
            2,
            &cleanup_modes(true),
            &mut |_: &str| {},
        );
        assert!(skipped.is_empty(), "{skipped:?}");
        for combo in combos.iter().filter(|c| c.cleanup == CleanupMode::Casual) {
            assert_eq!(
                combo.stats.cleanup.len(),
                2,
                "{} · Natürlich ohne Cleanup-Werte",
                combo.insert.label()
            );
        }
    }

    /// Zählt Instanzen eines Fake-Cleaners (REQ-DIAG-034: Gemma höchstens einmal, nie zwei zugleich).
    #[derive(Default)]
    struct Census {
        constructed: Cell<u32>,
        alive: Cell<u32>,
        max_alive: Cell<u32>,
        last_mode: Cell<Option<CleanupMode>>,
    }

    struct CountedCleaner(Rc<Census>);

    impl CountedCleaner {
        fn new(census: &Rc<Census>) -> Self {
            census.constructed.set(census.constructed.get() + 1);
            census.alive.set(census.alive.get() + 1);
            census
                .max_alive
                .set(census.max_alive.get().max(census.alive.get()));
            Self(Rc::clone(census))
        }
    }

    impl Drop for CountedCleaner {
        fn drop(&mut self) {
            self.0.alive.set(self.0.alive.get() - 1);
        }
    }

    impl LlmCleaner for CountedCleaner {
        fn clean(&mut self, raw: &str) -> talker::error::Result<String> {
            Ok(format!("{raw}!"))
        }
        fn set_mode(&mut self, mode: CleanupMode) {
            self.0.last_mode.set(Some(mode));
        }
    }

    /// Misst `fixtures` × beide Modi mit einem einmal erzeugten Cleaner; liefert Kombinationen und Zählung.
    fn measure_with_one_cleaner(
        fixtures: usize,
        runs: usize,
    ) -> (Vec<Combo>, Rc<Census>, TempFixtures) {
        let fx = TempFixtures::new(&format!("einmal-{fixtures}-{runs}"));
        let loaded: Vec<Fixture> = (0..fixtures)
            .map(|i| {
                let name = format!("clip{i}");
                fx.wav(&name, &[1_000; 9_600], Some("hallo welt"));
                load_fixture(&fx.0.join(format!("{name}.wav"))).unwrap()
            })
            .collect();
        let census = Rc::new(Census::default());
        let mut worker =
            fake_worker_with(shared_cleaner_loader(Some(CountedCleaner::new(&census))));
        let (combos, skipped) = measure(
            &mut worker,
            &loaded,
            runs,
            &cleanup_modes(true),
            &mut |_: &str| {},
        );
        assert!(skipped.is_empty(), "{skipped:?}");
        (combos, census, fx)
    }

    /// REQ-DIAG-034 AK-1/AK-2/AK-3 inkl. Grenzfälle (N=1, eine Fixture): nie zwei Instanzen zugleich,
    /// jeder Natürlich-Fall mit Cleanup; `constructed` ist nur Nebenbedingung.
    #[test]
    fn cleaner_is_never_duplicated_across_fixtures_runs_and_modes() {
        for (fixtures, runs) in [(2, 2), (2, 1), (1, 2)] {
            let (combos, census, _fx) = measure_with_one_cleaner(fixtures, runs);
            let ctx = format!("{fixtures} Fixtures × {runs} Läufe");
            assert_eq!(census.max_alive.get(), 1, "nie zwei Instanzen: {ctx}");
            assert_eq!(census.constructed.get(), 1, "kein Nachladen: {ctx}");
            assert_eq!(combos.len(), 4, "{ctx}");
            for combo in combos.iter().filter(|c| c.cleanup == CleanupMode::Casual) {
                assert_eq!(
                    combo.stats.cleanup.len(),
                    fixtures * runs,
                    "{ctx}: {}",
                    combo.insert.label()
                );
            }
            assert_eq!(
                census.last_mode.get(),
                Some(CleanupMode::Casual),
                "set_mode kommt am geteilten Cleaner an: {ctx}"
            );
        }
    }

    /// REQ-DIAG-034 AK-3: Die Instanz überlebt die Roh-Fälle (kein Drop mitten in der Reihe) und wird
    /// mit dem Worker freigegeben (kein Leck des Handles).
    #[test]
    fn cleaner_instance_survives_raw_cases_and_is_freed_with_the_worker() {
        let fx = TempFixtures::new("lebensdauer");
        fx.wav("clip", &[1_000; 9_600], Some("hallo welt"));
        let fixture = load_fixture(&fx.0.join("clip.wav")).unwrap();
        let census = Rc::new(Census::default());
        let mut worker =
            fake_worker_with(shared_cleaner_loader(Some(CountedCleaner::new(&census))));
        measure(
            &mut worker,
            &[fixture],
            2,
            &cleanup_modes(true),
            &mut |_: &str| {},
        );
        assert_eq!(
            census.alive.get(),
            1,
            "Roh-Fall darf die Instanz nicht zerstören"
        );
        drop(worker);
        assert_eq!(
            census.alive.get(),
            0,
            "Instanz wird mit dem Worker freigegeben"
        );
        assert_eq!(census.constructed.get(), 1);
    }

    /// REQ-DIAG-034 Grenzfall nur Roh (Absicherung nach T1, kein Rot-Grün-Zyklus).
    #[test]
    fn raw_only_series_has_no_casual_case_and_no_cleaner() {
        let fx = TempFixtures::new("nur-roh");
        fx.wav("clip", &[1_000; 9_600], Some("hallo welt"));
        let fixture = load_fixture(&fx.0.join("clip.wav")).unwrap();
        let mut worker = fake_worker_with(shared_cleaner_loader(None::<BracketCleaner>));
        let (combos, skipped) = measure(
            &mut worker,
            &[fixture],
            2,
            &cleanup_modes(false),
            &mut |_: &str| {},
        );
        assert!(skipped.is_empty(), "{skipped:?}");
        assert_eq!(combos.len(), 2);
        assert!(combos.iter().all(|c| c.cleanup == CleanupMode::Raw));
    }

    /// REQ-DIAG-028: Ohne Gemma liefert der Loader nichts und meldet keinen Fehlschlag-Cache.
    #[test]
    fn loader_without_gemma_yields_none_every_time() {
        let mut loader = shared_cleaner_loader(None::<BracketCleaner>);
        assert!(matches!(loader(), (None, false)));
        assert!(matches!(loader(), (None, false)));
    }

    /// REQ-DIAG-028: Der Übersprungen-Grund nennt den Pfad genau einmal.
    #[test]
    fn missing_gemma_reason_names_the_path_exactly_once() {
        let reason = load_cleaner(Path::new("/nicht/vorhanden/gemma.gguf"))
            .err()
            .expect("Gemma fehlt");
        assert_eq!(reason.matches("/nicht/vorhanden/gemma.gguf").count(), 1);
    }

    /// Gesamtlauf mit Fakes: 4 Fälle, Aufwärmlauf verworfen, defekte Quelle übersprungen, WER gebildet.
    #[test]
    fn measure_covers_all_four_cases_and_skips_unreadable_sources() {
        let fx = TempFixtures::new("messen");
        fx.wav("clip", &[1_000; 9_600], Some("hallo welt"));
        let good = load_fixture(&fx.0.join("clip.wav")).unwrap();
        let vanished = Fixture {
            name: "weg".into(),
            path: fx.0.join("weg.wav"),
            reference: "x".into(),
        };
        let mut worker = fake_worker();
        let calls = AtomicU32::new(0);
        let mut progress = |_: &str| {
            calls.fetch_add(1, Ordering::Relaxed);
        };
        let (combos, skipped) = measure(
            &mut worker,
            &[good, vanished],
            1,
            &cleanup_modes(true),
            &mut progress,
        );
        assert_eq!(combos.len(), 4);
        assert_eq!(
            skipped.len(),
            4,
            "die verschwundene Fixture fehlt in allen vier Fällen"
        );
        assert_eq!(skipped[0].0, "weg");
        assert!(
            combos.iter().all(|c| c.stats.release_to_text.len() == 1),
            "1 gemessener Lauf, Aufwärmlauf verworfen"
        );
        assert_eq!(calls.load(Ordering::Relaxed), 8);
        let at_end_raw = &combos[0].stats;
        assert_eq!(at_end_raw.wers, [0.0]);
        let table = render_table(&Report {
            combos,
            runs: 1,
            cold_loads: vec![],
            skipped,
            commit: "x".into(),
            date: "d".into(),
            device: "g".into(),
        });
        assert!(table.contains("| Am Ende | Roh |"));
        assert!(table.contains("weg: übersprungen:"));
    }
}
