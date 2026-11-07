//! Logging (ADR-0011): `log`-Fassade, eigener Datei-Logger mit Rotation.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::time::{SystemTime, UNIX_EPOCH};

use log::{Level, LevelFilter, Log, Metadata, Record};

use crate::config::LogLevel;

/// Ziel für fertig formatierte Logzeilen (ohne Zeilenende).
pub trait LogSink: Send {
    fn write_line(&mut self, line: &str) -> io::Result<()>;
}

/// `talker.log` mit Größenrotation: `talker.log` → `talker.1.log` … `talker.<keep>.log`,
/// die älteste Datei wird gelöscht. Keine Löschung nach Alter (REQ-DIAG-002).
pub struct RotatingFile {
    dir: PathBuf,
    file: File,
    size: u64,
    max_bytes: u64,
    keep: usize,
}

const LOG_NAME: &str = "talker";

fn log_path(dir: &Path, index: usize) -> PathBuf {
    if index == 0 {
        dir.join(format!("{LOG_NAME}.log"))
    } else {
        dir.join(format!("{LOG_NAME}.{index}.log"))
    }
}

impl RotatingFile {
    /// Öffnet `talker.log` im Anhängemodus; die Größe kommt aus der Dateilänge.
    pub fn open(dir: &Path, max_bytes: u64, keep: usize) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path(dir, 0))?;
        let size = file.metadata()?.len();
        Ok(Self {
            dir: dir.to_path_buf(),
            file,
            size,
            max_bytes,
            keep,
        })
    }

    fn rotate(&mut self) -> io::Result<()> {
        if self.keep > 0 {
            match std::fs::remove_file(log_path(&self.dir, self.keep)) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            for index in (0..self.keep).rev() {
                match std::fs::rename(log_path(&self.dir, index), log_path(&self.dir, index + 1)) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
        }
        self.file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(log_path(&self.dir, 0))?;
        self.size = 0;
        Ok(())
    }
}

impl LogSink for RotatingFile {
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        let bytes = line.len() as u64 + 1;
        if self.size > 0 && self.size + bytes > self.max_bytes {
            self.rotate()?;
        }
        self.file.write_all(line.as_bytes())?;
        self.file.write_all(b"\n")?;
        self.size += bytes;
        Ok(())
    }
}

/// Zeitstempel in UTC mit Suffix `Z` (Fallback, wenn die Ortszeit nicht ermittelbar ist).
fn format_utc(secs: i64, millis: u32) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Zivilkalender aus Tagen seit 1970-01-01 (Howard Hinnant, `civil_from_days`).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

/// Zeitstempel in Ortszeit (`libc::localtime_r`, thread-sicher); sonst UTC.
fn format_timestamp(now: SystemTime) -> String {
    let (secs, millis) = match now.duration_since(UNIX_EPOCH) {
        Ok(d) => (d.as_secs() as i64, d.subsec_millis()),
        Err(_) => (0, 0),
    };
    let t = secs as libc::time_t;
    // SAFETY: `tm` ist ein reiner C-Struct (nur Zahlen/Zeiger), Nullen sind gültig;
    // `localtime_r` schreibt nur in den übergebenen Puffer und liest `t`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    if !ok {
        return format_utc(secs, millis);
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{millis:03}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

/// Ziele unter `talker` folgen dem gewählten Grad; fremde Crates höchstens `warn`.
fn allows(target: &str, level: Level, max: LevelFilter) -> bool {
    let own = target == "talker" || target.starts_with("talker::");
    let cap = if own { max } else { max.min(LevelFilter::Warn) };
    level <= cap
}

fn format_line(now: SystemTime, level: Level, target: &str, text: &std::fmt::Arguments) -> String {
    let module = target.strip_prefix("talker::").unwrap_or(target);
    format!("{} {level:<5} {module}: {text}", format_timestamp(now))
}

/// `log::Log`-Impl: formatiert im aufrufenden Thread, übergibt per `try_send` an den
/// Writer-Thread. Wartet nie, paniciert nie; volle Warteschlange → verwerfen + zählen.
pub struct Logger {
    tx: SyncSender<String>,
    dropped: std::sync::Arc<AtomicU64>,
}

impl Logger {
    pub fn spawn(sink: Box<dyn LogSink>, capacity: usize) -> Self {
        let (tx, rx) = sync_channel(capacity);
        let dropped = std::sync::Arc::new(AtomicU64::new(0));
        let counter = dropped.clone();
        // Scheitert der Thread-Start, ist `rx` weg: alle Zeilen werden dann verworfen.
        let _ = std::thread::Builder::new()
            .name("talker-log".into())
            .spawn(move || write_loop(rx, sink, &counter));
        Self { tx, dropped }
    }

    fn submit(&self, line: String) {
        match self.tx.try_send(line) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

fn write_loop(rx: Receiver<String>, mut sink: Box<dyn LogSink>, dropped: &AtomicU64) {
    let mut reported_error = false;
    while let Ok(line) = rx.recv() {
        let lost = dropped.swap(0, Ordering::Relaxed);
        let notice = (lost > 0).then(|| {
            format!(
                "{} WARN  logging: {lost} Logzeilen verworfen (Puffer voll)",
                format_timestamp(SystemTime::now())
            )
        });
        for text in notice.iter().chain(std::iter::once(&line)) {
            if let Err(e) = sink.write_line(text)
                && !reported_error
            {
                reported_error = true;
                eprintln!("talker: Logdatei nicht schreibbar ({e})");
            }
        }
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        allows(metadata.target(), metadata.level(), log::max_level())
    }

    fn log(&self, record: &Record) {
        // Den gewählten Grad prüfen schon die `log`-Makros (`max_level`); hier bleibt
        // nur die Obergrenze für fremde Ziele.
        if allows(record.target(), record.level(), LevelFilter::Trace) {
            self.submit(format_line(
                SystemTime::now(),
                record.level(),
                record.target(),
                record.args(),
            ));
        }
    }

    fn flush(&self) {}
}

/// Ergebnis von [`init`]: Datei-Log oder stderr (Verzeichnis/Datei nicht nutzbar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogInit {
    File,
    Stderr,
}

const MAX_BYTES: u64 = 1_000_000;
const KEEP: usize = 4;
const QUEUE_CAPACITY: usize = 1024;

/// Fallback-Sink: Terminal-Start sieht die Zeilen weiter (REQ-DIAG-010).
struct StderrSink;

impl LogSink for StderrSink {
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        eprintln!("{line}");
        Ok(())
    }
}

/// `TALKER_LOG` gegen den Config-Wert auflösen: gültig übersteuert, leer gilt als nicht
/// gesetzt, ungültig → Config-Wert plus Warntext (REQ-DIAG-006/-007).
pub fn resolve_level(env: Option<&str>, cfg: LogLevel) -> (LogLevel, Option<String>) {
    match env {
        None => (cfg, None),
        Some(raw) if raw.trim().is_empty() => (cfg, None),
        Some(raw) => match LogLevel::parse_env(raw) {
            Some(level) => (level, None),
            None => (
                cfg,
                Some(format!(
                    "TALKER_LOG=\"{raw}\" ungültig (erlaubt: error, warn, info, debug) — nutze log_level aus der Config"
                )),
            ),
        },
    }
}

/// Der Grad, den `apply_config_level` setzen würde; `None` bei gültigem Env-Override.
fn level_to_apply(env_override: Option<LogLevel>, cfg: LogLevel) -> Option<LogLevel> {
    env_override.is_none().then_some(cfg)
}

fn to_filter(level: LogLevel) -> LevelFilter {
    match level {
        LogLevel::Error => LevelFilter::Error,
        LogLevel::Warn => LevelFilter::Warn,
        LogLevel::Info => LevelFilter::Info,
        LogLevel::Debug => LevelFilter::Debug,
    }
}

static ENV_OVERRIDE: OnceLock<Option<LogLevel>> = OnceLock::new();
static ENV_WARNING: OnceLock<Option<String>> = OnceLock::new();

/// Gültiges `TALKER_LOG` dieses Prozesses (für die Settings-Sperre).
pub fn env_override() -> Option<LogLevel> {
    ENV_OVERRIDE.get().copied().flatten()
}

/// Warnt (über den Logger), falls `TALKER_LOG` ungültig war; nach `Config::load()` aufrufen.
pub fn warn_if_env_invalid() {
    if let Some(Some(text)) = ENV_WARNING.get() {
        log::warn!(target: "talker::logging", "{text}");
    }
}

/// Setzt den Grad aus der Config; No-op, solange `TALKER_LOG` gültig übersteuert.
pub fn apply_config_level(level: LogLevel) {
    apply_level(env_override(), level);
}

/// Wie [`apply_config_level`] mit explizitem Override; liefert den gesetzten Filter.
pub(crate) fn apply_level(env_override: Option<LogLevel>, cfg: LogLevel) -> Option<LevelFilter> {
    let filter = to_filter(level_to_apply(env_override, cfg)?);
    log::set_max_level(filter);
    Some(filter)
}

/// `~/Library/Logs/talker`.
pub fn default_log_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join("Library/Logs/talker")
}

/// `~/Library/Logs/talker/talker.log`.
pub fn log_file_path() -> PathBuf {
    default_log_dir().join("talker.log")
}

fn build_logger(dir: &Path) -> (Logger, LogInit) {
    match RotatingFile::open(dir, MAX_BYTES, KEEP) {
        Ok(file) => (Logger::spawn(Box::new(file), QUEUE_CAPACITY), LogInit::File),
        Err(e) => {
            eprintln!(
                "talker: Logdatei in {} nicht nutzbar ({e}) — logge auf stderr",
                dir.display()
            );
            (
                Logger::spawn(Box::new(StderrSink), QUEUE_CAPACITY),
                LogInit::Stderr,
            )
        }
    }
}

/// Installiert den Logger (einmal, vor `Config::load()`). Grad zunächst `TALKER_LOG`
/// bzw. Info; die Config setzt danach `apply_config_level`.
pub fn init(log_dir: &Path) -> LogInit {
    let env = std::env::var("TALKER_LOG").ok();
    let (level, warning) = resolve_level(env.as_deref(), LogLevel::Info);
    let _ = ENV_OVERRIDE.set(
        env.as_deref()
            .filter(|_| warning.is_none())
            .and_then(LogLevel::parse_env),
    );
    let _ = ENV_WARNING.set(warning);
    let (logger, result) = build_logger(log_dir);
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(to_filter(level));
    }
    result
}

/// Argumente für `open`: Datei vorhanden → `-R <datei>` (im Finder markieren), sonst Ordner.
fn reveal_args(dir: &Path, file: &Path) -> Vec<OsString> {
    if file.exists() {
        vec!["-R".into(), file.as_os_str().to_owned()]
    } else {
        vec![dir.as_os_str().to_owned()]
    }
}

/// Zeigt `talker.log` im Finder (REQ-DIAG-008).
pub fn reveal_in_finder() {
    let args = reveal_args(&default_log_dir(), &log_file_path());
    if let Err(e) = std::process::Command::new("open").args(args).spawn() {
        log::warn!(target: "talker::logging", "Finder konnte nicht geöffnet werden: {e}");
    }
}

#[cfg(test)]
thread_local! {
    static CAPTURED: std::cell::RefCell<Option<Vec<(Level, String)>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
struct CaptureLogger;

#[cfg(test)]
impl Log for CaptureLogger {
    fn enabled(&self, _: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        CAPTURED.with(|c| {
            if let Some(lines) = c.borrow_mut().as_mut() {
                lines.push((
                    record.level(),
                    format!("{}: {}", record.target(), record.args()),
                ));
            }
        });
    }

    fn flush(&self) {}
}

/// Test-Hilfe: sammelt alle Logzeilen (`<ziel>: <text>`), die `f` auf **diesem** Thread
/// erzeugt — parallele Tests stören sich nicht. Schreibt nichts in eine Datei.
#[cfg(test)]
pub fn capture<R>(f: impl FnOnce() -> R) -> (R, Vec<(Level, String)>) {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        let _ = log::set_logger(&CaptureLogger);
    });
    log::set_max_level(LevelFilter::Debug);
    CAPTURED.with(|c| *c.borrow_mut() = Some(Vec::new()));
    let result = f();
    let lines = CAPTURED.with(|c| c.borrow_mut().take()).unwrap_or_default();
    (result, lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Frisches, test-eigenes Verzeichnis unterm System-Tempdir (kein tempfile-Dep).
    pub(super) struct TestDir(pub PathBuf);
    impl TestDir {
        pub fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "talker-log-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    /// Zeile mit genau `total` Byte inkl. Zeilenende.
    fn line(total: usize, fill: char) -> String {
        fill.to_string().repeat(total - 1)
    }

    /// REQ-DIAG-002: genau `max_bytes` → keine Rotation.
    #[test]
    fn file_of_exactly_max_bytes_is_not_rotated() {
        let t = TestDir::new();
        let mut f = RotatingFile::open(&t.0, 100, 4).unwrap();
        f.write_line(&line(60, 'a')).unwrap();
        f.write_line(&line(40, 'b')).unwrap();
        assert_eq!(names(&t.0), ["talker.log"]);
        assert_eq!(
            std::fs::metadata(t.0.join("talker.log")).unwrap().len(),
            100
        );
    }

    /// REQ-DIAG-002: +1 Byte → Rotation vor dem Schreiben der Zeile.
    #[test]
    fn line_that_would_cross_the_limit_rotates_first() {
        let t = TestDir::new();
        let mut f = RotatingFile::open(&t.0, 100, 4).unwrap();
        f.write_line(&line(60, 'a')).unwrap();
        f.write_line(&line(41, 'b')).unwrap();
        assert_eq!(names(&t.0), ["talker.1.log", "talker.log"]);
        assert_eq!(
            std::fs::metadata(t.0.join("talker.1.log")).unwrap().len(),
            60
        );
        assert_eq!(std::fs::metadata(t.0.join("talker.log")).unwrap().len(), 41);
        assert!(
            std::fs::read_to_string(t.0.join("talker.log"))
                .unwrap()
                .starts_with('b')
        );
    }

    /// REQ-DIAG-002: nie mehr als 5 Dateien; die 5. Rotation löscht `talker.4.log`.
    #[test]
    fn fifth_rotation_drops_the_oldest_and_keeps_five_files() {
        let t = TestDir::new();
        let mut f = RotatingFile::open(&t.0, 10, 4).unwrap();
        for c in ['a', 'b', 'c', 'd', 'e', 'f'] {
            f.write_line(&line(10, c)).unwrap();
        }
        assert_eq!(
            names(&t.0),
            [
                "talker.1.log",
                "talker.2.log",
                "talker.3.log",
                "talker.4.log",
                "talker.log"
            ]
        );
        let first = |n: &str| {
            std::fs::read_to_string(t.0.join(n))
                .unwrap()
                .chars()
                .next()
                .unwrap()
        };
        assert_eq!(first("talker.log"), 'f');
        assert_eq!(first("talker.1.log"), 'e');
        assert_eq!(first("talker.4.log"), 'b');
    }

    /// AK-2: mit den Produktionswerten (1 MB, 4 ältere) bleibt die Summe nach vielen Zeilen ≤ 5 MB
    /// und es gibt nie mehr als 5 Dateien.
    #[test]
    fn production_limits_keep_at_most_five_files_and_five_megabytes() {
        assert_eq!((MAX_BYTES, KEEP), (1_000_000, 4));
        let t = TestDir::new();
        let mut f = RotatingFile::open(&t.0, MAX_BYTES, KEEP).unwrap();
        let text = "x".repeat(9_999);
        for _ in 0..800 {
            f.write_line(&text).unwrap();
        }
        let files = names(&t.0);
        assert_eq!(files.len(), 5, "{files:?}");
        let total: u64 = files
            .iter()
            .map(|n| std::fs::metadata(t.0.join(n)).unwrap().len())
            .sum();
        assert!(total <= 5_000_000, "gesamt {total}");
        assert!(std::fs::metadata(t.0.join("talker.log")).unwrap().len() <= MAX_BYTES);
    }

    /// REQ-DIAG-002 „darf nicht passieren": keine Löschung nach Alter.
    #[test]
    fn old_files_are_not_deleted_by_age() {
        let t = TestDir::new();
        let old = t.0.join("talker.1.log");
        std::fs::write(&old, "alt\n").unwrap();
        let long_ago = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000);
        File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        let mut f = RotatingFile::open(&t.0, 100, 4).unwrap();
        f.write_line("neu").unwrap();
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "alt\n");
        assert_eq!(
            std::fs::metadata(&old).unwrap().modified().unwrap(),
            long_ago
        );
    }

    /// Vorhandene Datei wird angehängt, Größe stammt aus der Dateilänge.
    #[test]
    fn existing_file_is_appended_and_size_is_taken_from_disk() {
        let t = TestDir::new();
        std::fs::write(t.0.join("talker.log"), line(90, 'x') + "\n").unwrap();
        let mut f = RotatingFile::open(&t.0, 100, 4).unwrap();
        f.write_line(&line(11, 'y')).unwrap();
        assert_eq!(names(&t.0), ["talker.1.log", "talker.log"]);
        assert_eq!(
            std::fs::metadata(t.0.join("talker.1.log")).unwrap().len(),
            90
        );
    }

    /// Eine einzelne Zeile über dem Limit steht allein in einer frischen Datei.
    #[test]
    fn single_oversized_line_is_written_without_endless_rotation() {
        let t = TestDir::new();
        let mut f = RotatingFile::open(&t.0, 10, 4).unwrap();
        f.write_line(&line(50, 'z')).unwrap();
        assert_eq!(names(&t.0), ["talker.log"]);
        f.write_line("x").unwrap();
        assert_eq!(names(&t.0), ["talker.1.log", "talker.log"]);
    }

    /// REQ-DIAG-001/-010: nicht anlegbares/schreibgeschütztes Verzeichnis → Err.
    #[test]
    fn open_in_read_only_dir_is_an_error() {
        use std::os::unix::fs::PermissionsExt;
        let t = TestDir::new();
        std::fs::set_permissions(&t.0, std::fs::Permissions::from_mode(0o500)).unwrap();
        let result = RotatingFile::open(&t.0.join("logs"), 100, 4);
        std::fs::set_permissions(&t.0, std::fs::Permissions::from_mode(0o700)).unwrap();
        if std::fs::write(t.0.join("probe"), "").is_ok() {
            // root ignoriert Rechte — dann ist der Test nicht aussagekräftig
            return;
        }
        assert!(result.is_err());
    }

    use std::sync::mpsc;
    use std::time::Duration;

    fn record_to(logger: &Logger, level: Level, target: &str, text: &str) {
        logger.log(
            &Record::builder()
                .level(level)
                .target(target)
                .args(format_args!("{text}"))
                .build(),
        );
    }

    struct ChannelSink(mpsc::Sender<String>);
    impl LogSink for ChannelSink {
        fn write_line(&mut self, line: &str) -> io::Result<()> {
            let _ = self.0.send(line.to_string());
            Ok(())
        }
    }

    /// Sink, der beim ersten Schreiben blockiert, bis `release` gesendet wird.
    struct BlockingSink {
        started: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
        lines: mpsc::Sender<String>,
    }
    impl LogSink for BlockingSink {
        fn write_line(&mut self, line: &str) -> io::Result<()> {
            let _ = self.started.send(());
            let _ = self.release.recv();
            let _ = self.lines.send(line.to_string());
            Ok(())
        }
    }

    /// REQ-DIAG-011: blockierendes Backend → `log()` kehrt sofort zurück, Verluste gezählt.
    #[test]
    fn blocking_sink_never_blocks_callers_and_drops_are_counted() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (lines_tx, lines_rx) = mpsc::channel();
        let capacity = 8;
        let logger = Logger::spawn(
            Box::new(BlockingSink {
                started: started_tx,
                release: release_rx,
                lines: lines_tx,
            }),
            capacity,
        );
        record_to(&logger, Level::Warn, "talker::x", "erste");
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let t0 = std::time::Instant::now();
        for i in 0..capacity + 100 {
            record_to(&logger, Level::Warn, "talker::x", &format!("zeile {i}"));
        }
        assert!(t0.elapsed() < Duration::from_secs(1));
        assert_eq!(logger.dropped.load(Ordering::Relaxed), 100);
        // Backend wieder frei → gepufferte Zeilen laufen durch, danach der Verlust-Hinweis.
        drop(release_tx);
        let first = lines_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(first.contains("erste"));
        let mut all = String::new();
        while let Ok(l) = lines_rx.recv_timeout(Duration::from_secs(1)) {
            all.push_str(&l);
            all.push('\n');
        }
        assert!(all.contains("zeile 0"));
        assert!(all.contains("100 Logzeilen verworfen"));
    }

    /// REQ-DIAG-011: Sink-Fehler → kein Panic, Folgezeilen laufen weiter.
    #[test]
    fn failing_sink_does_not_panic_and_later_lines_are_still_attempted() {
        struct Failing(mpsc::Sender<String>);
        impl LogSink for Failing {
            fn write_line(&mut self, line: &str) -> io::Result<()> {
                let _ = self.0.send(line.to_string());
                Err(io::Error::other("Platte voll"))
            }
        }
        let (tx, rx) = mpsc::channel();
        let logger = Logger::spawn(Box::new(Failing(tx)), 16);
        for i in 0..3 {
            record_to(&logger, Level::Error, "talker::x", &format!("z{i}"));
        }
        for i in 0..3 {
            let l = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(l.ends_with(&format!("z{i}")));
        }
    }

    /// Filter: fremde Crates höchstens `warn`, `talker::*` folgt dem Grad.
    #[test]
    fn filter_caps_foreign_targets_at_warn_and_own_targets_follow_the_level() {
        use Level::*;
        use LevelFilter as F;
        let table = [
            // (target, level, max, erlaubt)
            ("ureq", Info, F::Info, false),
            ("egui", Debug, F::Debug, false),
            ("ureq", Warn, F::Info, true),
            ("rustls::x", Error, F::Error, true),
            ("rustls::x", Warn, F::Error, false),
            ("talker::pipeline", Info, F::Info, true),
            ("talker::pipeline", Debug, F::Info, false),
            ("talker::pipeline", Debug, F::Debug, true),
            ("talker", Info, F::Info, true),
            ("talker::messung", Warn, F::Error, false),
            ("talker::messung", Error, F::Error, true),
            ("talkerx", Info, F::Info, false),
            ("talker::x", Error, F::Off, false),
        ];
        for (target, level, max, expected) in table {
            assert_eq!(
                allows(target, level, max),
                expected,
                "{target} {level} {max}"
            );
        }
    }

    /// Zeilenformat: Zeitstempel, Level, Ziel ohne `talker::`, Text.
    #[test]
    fn line_format_has_timestamp_level_module_and_text() {
        let (tx, rx) = mpsc::channel();
        let logger = Logger::spawn(Box::new(ChannelSink(tx)), 4);
        record_to(&logger, Level::Warn, "talker::models", "hallo");
        let l = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (ts, rest) = l.split_at(23);
        let b = ts.as_bytes();
        assert!(b[..4].iter().all(u8::is_ascii_digit) && b[4] == b'-' && b[7] == b'-');
        assert!(b[10] == b' ' && b[13] == b':' && b[16] == b':' && b[19] == b'.');
        assert_eq!(rest, " WARN  models: hallo");
        assert_eq!(
            format_line(UNIX_EPOCH, Level::Info, "ureq", &format_args!("x"))
                .split_at(23)
                .1,
            " INFO  ureq: x"
        );
    }

    #[test]
    fn utc_fallback_formats_known_instants() {
        assert_eq!(format_utc(0, 0), "1970-01-01 00:00:00.000Z");
        assert_eq!(format_utc(951_782_400, 5), "2000-02-29 00:00:00.005Z");
        assert_eq!(format_utc(1_794_023_052, 123), "2026-11-07 03:44:12.123Z");
        assert_eq!(format_utc(-1, 999), "1969-12-31 23:59:59.999Z");
    }

    #[test]
    fn dropping_the_logger_ends_the_writer_thread() {
        let (tx, rx) = mpsc::channel();
        let logger = Logger::spawn(Box::new(ChannelSink(tx)), 4);
        record_to(&logger, Level::Error, "talker::x", "a");
        drop(logger);
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok());
        // Sink (und damit `tx`) wird mit dem Thread freigegeben → Kanal schließt.
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
    }

    /// `capture` sammelt je Thread; Zeilen anderer Threads und außerhalb bleiben draußen.
    #[test]
    fn capture_collects_per_thread() {
        log::info!(target: "talker::x", "außerhalb");
        let (value, lines) = capture(|| {
            std::thread::spawn(|| log::info!(target: "talker::x", "fremder thread"))
                .join()
                .unwrap();
            log::info!(target: "talker::x", "drin");
            log::debug!(target: "talker::y", "detail");
            7
        });
        assert_eq!(value, 7);
        assert_eq!(
            lines,
            [
                (Level::Info, "talker::x: drin".to_string()),
                (Level::Debug, "talker::y: detail".to_string())
            ]
        );
        let (_, empty) = capture(|| ());
        assert!(empty.is_empty());
    }

    /// REQ-DIAG-006/-007: `TALKER_LOG` gegen Config.
    #[test]
    fn env_level_resolution_table() {
        use LogLevel::*;
        assert_eq!(resolve_level(None, Warn), (Warn, None));
        assert_eq!(resolve_level(Some("debug"), Warn), (Debug, None));
        assert_eq!(resolve_level(Some(" ERROR "), Info), (Error, None));
        assert_eq!(resolve_level(Some(""), Warn), (Warn, None));
        assert_eq!(resolve_level(Some("   "), Warn), (Warn, None));
        let (level, warning) = resolve_level(Some("laut"), Warn);
        assert_eq!(level, Warn);
        assert!(warning.unwrap().contains("TALKER_LOG"));
    }

    /// REQ-DIAG-006: Config-Änderung wirkt nur ohne gültigen Env-Override.
    #[test]
    fn config_level_is_ignored_while_env_overrides() {
        assert_eq!(level_to_apply(None, LogLevel::Debug), Some(LogLevel::Debug));
        assert_eq!(level_to_apply(Some(LogLevel::Error), LogLevel::Debug), None);
        assert_eq!(to_filter(LogLevel::Error), LevelFilter::Error);
        assert_eq!(to_filter(LogLevel::Debug), LevelFilter::Debug);
    }

    #[test]
    fn default_paths_end_in_library_logs_talker() {
        assert!(default_log_dir().ends_with("Library/Logs/talker"));
        assert!(log_file_path().ends_with("Library/Logs/talker/talker.log"));
    }

    /// REQ-DIAG-010: Verzeichnis nicht nutzbar → stderr-Fallback, kein Panic.
    #[test]
    fn unusable_log_dir_falls_back_to_stderr() {
        let t = TestDir::new();
        let (_, ok) = build_logger(&t.0.join("logs"));
        assert_eq!(ok, LogInit::File);
        // Datei statt Verzeichnis: nicht anlegbar, auch als root.
        let blocker = t.0.join("datei");
        std::fs::write(&blocker, "x").unwrap();
        let (_, fallback) = build_logger(&blocker.join("logs"));
        assert_eq!(fallback, LogInit::Stderr);
    }

    /// REQ-DIAG-008: Datei da → markieren, sonst Ordner öffnen.
    #[test]
    fn reveal_selects_the_file_or_opens_the_folder() {
        let t = TestDir::new();
        let file = t.0.join("talker.log");
        assert_eq!(reveal_args(&t.0, &file), [t.0.as_os_str()]);
        std::fs::write(&file, "x").unwrap();
        assert_eq!(
            reveal_args(&t.0, &file),
            [OsString::from("-R"), file.as_os_str().to_owned()]
        );
    }

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// REQ-DIAG-010: kein `eprintln!` in `src/` außer dem stderr-Fallback in `logging.rs`.
    #[test]
    fn no_eprintln_outside_the_stderr_fallback() {
        let mut files = Vec::new();
        rust_files(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        assert!(!files.is_empty());
        let offenders: Vec<_> = files
            .iter()
            .filter(|f| {
                let name = f.file_name().unwrap().to_string_lossy();
                name != "logging.rs"
            })
            .filter(|f| std::fs::read_to_string(f).unwrap().contains("eprintln!"))
            .collect();
        assert!(offenders.is_empty(), "eprintln! in {offenders:?}");
    }
}
