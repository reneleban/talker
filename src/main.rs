//! talker — lokaler macOS-Diktier-Assistent.
//! Pipeline: PTT-Hotkey → audio-capture → stt → cleanup → injection.
//! eframe besitzt den Main-Loop (Settings-Fenster, ADR-0002); Tray + Event-Tap
//! hängen am selben Main-RunLoop.

use std::process::ExitCode;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use talker::diagnostics::{DictationResult, TraceReport};
use talker::indicator::Indicator;
use talker::{
    audio, cleanup, clipboard, config, hotkey, injection, models, overlay, permissions, pipeline,
    stt, tray, ui,
};

/// Lädt das Cleanup-Modell; das zweite Element meldet den Fehlschlag
/// (Fehler-Cache: bis zum Moduswechsel nicht erneut versuchen).
fn try_load_cleaner() -> (Option<cleanup::GemmaCleaner>, bool) {
    let t = Instant::now();
    match cleanup::GemmaCleaner::new(&cleanup::GemmaCleaner::default_model_path()) {
        Ok(c) => {
            log::info!(
                "Cleanup-Modell geladen in {:.1}s.",
                t.elapsed().as_secs_f32()
            );
            (Some(c), false)
        }
        Err(e) => {
            log::warn!("Cleanup nicht verfügbar — {e}");
            (None, true)
        }
    }
}

/// STT + Cleanup + Injection laufen abseits des Main-Threads: der Dictation
/// Worker (pipeline.rs) besitzt die Modelle und deren Lebenszyklus; dieser
/// Thread drainiert nur den Channel und mappt `Outcome` auf Indicator/Injection.
fn spawn_stt_worker(
    config: Arc<RwLock<config::Config>>,
    indicator: Arc<Mutex<Indicator>>,
    egui_ctx: eframe::egui::Context,
    models_state: Arc<models::ModelsState>,
    trace_report: TraceReport,
) -> mpsc::Sender<pipeline::SttJob> {
    // Ergebnis der Pipeline ans Overlay melden (+ Repaint anstoßen).
    let report = move |f: &dyn Fn(&mut Indicator)| {
        if let Ok(mut ind) = indicator.lock() {
            f(&mut ind);
        }
        egui_ctx.request_repaint();
    };
    let (tx, rx) = mpsc::channel::<pipeline::SttJob>();
    std::thread::spawn(move || {
        let snapshot = |cfg: &Arc<RwLock<config::Config>>| cfg.read().map(|c| c.clone()).ok();

        let Some(cfg) = snapshot(&config) else { return };
        // Debug-Mitschnitt nur mit TALKER_DEBUG_WAV_DIR; der Tap nur dann (REQ-DIAG-022).
        let debug = talker::diagnostics::DebugCapture::from_env();
        let tap = debug.as_ref().map(|_| stt::ModelTap::default());
        let mut worker = pipeline::DictationWorker::new(
            &cfg,
            models_state,
            Box::new({
                let config = Arc::clone(&config);
                let tap = tap.clone();
                move |dir: &std::path::Path| {
                    let pause = config
                        .read()
                        .map_or(config::DEFAULT_ENDPOINT_SILENCE_MS, |c| {
                            c.endpoint_silence_ms
                        });
                    stt::NemotronTranscriber::with_endpoint_silence(dir, pause).map(
                        |t| match &tap {
                            Some(tap) => t.with_model_tap(tap.clone()),
                            None => t,
                        },
                    )
                }
            }),
            Box::new(try_load_cleaner),
        );
        report(&|ind| ind.ready(Instant::now()));

        let mut inject =
            |text: &str| injection::inject(&clipboard::NsPasteboard, &injection::CgKeySender, text);

        for job in rx {
            let Some(cfg) = snapshot(&config) else {
                // Poisoned Config-Lock: nicht still verwerfen (CLAUDE.md).
                log::error!("Config-Lock poisoned — Utterance verworfen.");
                let mut trace = job.into_trace();
                trace.result = DictationResult::Error("Config-Lock poisoned".into());
                trace_report(trace);
                report(&|ind| ind.fail(Instant::now(), "Interner Fehler (Config)"));
                continue;
            };
            let (hint, trace) = pipeline::process_job(
                &mut worker,
                job,
                &cfg,
                &mut inject,
                debug.as_ref().zip(tap.as_ref()),
            );
            trace_report(trace);
            match hint {
                Some(hint) => report(&|ind| ind.fail(Instant::now(), hint)),
                None => report(&|ind| ind.finish_ok(Instant::now())),
            }
        }
    });
    tx
}

/// Startet talker als neuen Prozess und beendet diesen (Ticket-0030):
/// der Event-Tap funktioniert nach einem Laufzeit-Accessibility-Grant erst
/// im frischen Prozess. Scheitert das Spawnen, läuft die App weiter.
fn relaunch() {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => {
            log::error!("Relaunch nicht möglich (current_exe): {e}");
            return;
        }
    };
    match std::process::Command::new(&exe).spawn() {
        Ok(_) => {
            log::info!("Accessibility erteilt — Neustart für den Event-Tap.");
            std::process::exit(0);
        }
        Err(e) => log::error!("Relaunch fehlgeschlagen: {e}"),
    }
}

fn main() -> ExitCode {
    talker::logging::init(talker::logging::default_log_dir().as_deref());
    let loaded = config::Config::load();
    talker::logging::apply_config_level(loaded.log_level);
    let config = Arc::new(RwLock::new(loaded));

    // Modell-Setup (Ticket-0028/0029): Zustand per Präsenz-Check (der volle
    // Checksum-Check läuft im Hintergrund nach), unfertige Downloads bei
    // vorhandenem Consent direkt wieder anstoßen.
    let models_root = models::default_models_root();
    let consent = config.read().map(|c| c.models_consent()).unwrap_or(false);
    let models_state = Arc::new(models::ModelsState::from_disk_quick(&models_root, consent));
    models::spawn_integrity_check(Arc::clone(&models_state), models_root.clone());
    models::start_needed_downloads(&models_state, &models_root, consent);

    // First-Run-Onboarding: fehlt eine Permission oder die Spracherkennung
    // (Setup/Consent), startet das Fenster sichtbar.
    let accessibility = permissions::ensure_accessibility();
    let mic_status = permissions::microphone_status();
    // Mic-Prompt sofort beim Erst-Start (Ticket-0030) — nicht erst beim
    // ersten PTT, der den Event-Tap (Accessibility + Relaunch) voraussetzt.
    if mic_status == permissions::MicPermission::Undetermined {
        permissions::request_microphone();
    }
    let mic_denied = mic_status == permissions::MicPermission::Denied;
    let show_onboarding = !accessibility || mic_denied || !models_state.stt_ready();

    let native_options = eframe::NativeOptions {
        // glow statt wgpu: wgpu kann auf macOS keine Fenster-Transparenz
        // (Overlay bekäme einen schwarzen Kasten) — egui#2680.
        renderer: eframe::Renderer::Glow,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("talker — Einstellungen")
            .with_inner_size([440.0, 600.0])
            .with_min_inner_size([400.0, 480.0])
            .with_visible(show_onboarding),
        event_loop_builder: Some(Box::new(|builder| {
            use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
            // Menüleisten-App: kein Dock-Icon.
            builder.with_activation_policy(ActivationPolicy::Accessory);
        })),
        ..Default::default()
    };

    let result = eframe::run_native(
        "talker",
        native_options,
        Box::new(move |cc| {
            // Läuft auf dem Main-Thread, der Event-Loop existiert bereits:
            // Tray, Event-Tap und Worker hier verdrahten.
            let indicator = Arc::new(Mutex::new(Indicator::default()));
            if let Ok(mut ind) = indicator.lock() {
                ind.loading(Instant::now());
            }
            let egui_ctx = cc.egui_ctx.clone();
            let initial_mode = config.read().map(|c| c.cleanup_mode).unwrap_or_default();
            let tray = Rc::new(tray::Tray::new(initial_mode)?);

            // Natives Overlay + Tray-Sync + 60-fps-Animations-Timer auf dem
            // Main-RunLoop — Tray-Sync ist idempotent (Cell-Diff in Tray::sync),
            // häufiges Aufrufen kostet nur Lesezugriffe (Ticket-0037).
            {
                use objc2::MainThreadMarker;
                use objc2_foundation::NSTimer;
                let mtm = MainThreadMarker::new().expect("creation ctx läuft auf Main-Thread");
                let overlay = Rc::new(overlay::Overlay::new(mtm));
                let ind = Arc::clone(&indicator);
                let cfg = Arc::clone(&config);
                let tray_sync = Rc::clone(&tray);
                let models_sync = Arc::clone(&models_state);
                // Soundness: Timer wird auf dem Main-RunLoop geplant und feuert
                // ausschließlich dort — der nicht-Send-Block verlässt den Thread nie.
                let block = block2::RcBlock::new(move |_timer: std::ptr::NonNull<NSTimer>| {
                    overlay.tick(&ind, &cfg);
                    let mode = cfg.read().map(|c| c.cleanup_mode).unwrap_or_default();
                    if let Ok(indicator) = ind.lock() {
                        tray_sync.sync(mode, indicator.phase(), models_sync.stt_ready());
                    }
                });
                let timer = unsafe {
                    NSTimer::scheduledTimerWithTimeInterval_repeats_block(1.0 / 60.0, true, &block)
                };
                std::mem::forget(timer); // lebt so lange wie die App
            }
            tray::set_instance(Rc::clone(&tray));

            if !accessibility {
                tray.set_permission_warning();
                log::warn!("Accessibility-Permission fehlt — Onboarding im Settings-Fenster.");
            }

            // Eine Messzeile je Dictation, auf `info` (ADR-0011/-0012).
            let trace_report: TraceReport = talker::diagnostics::log_report();
            let stt_tx = spawn_stt_worker(
                Arc::clone(&config),
                Arc::clone(&indicator),
                egui_ctx.clone(),
                Arc::clone(&models_state),
                Arc::clone(&trace_report),
            );

            // Tap-Installation als wiederholbare Factory: scheitert sie (z.B.
            // Accessibility erst nach App-Start erteilt), versucht ein Timer es
            // alle 3 s erneut — kein App-Neustart nötig, kein stiller Tod.
            let install_tap = {
                let config = Arc::clone(&config);
                let indicator = Arc::clone(&indicator);
                let egui_ctx = egui_ctx.clone();
                let stt_tx = stt_tx.clone();
                let models_state = Arc::clone(&models_state);
                move || {
                    let mic_config = Arc::clone(&config);
                    let ind_press = Arc::clone(&indicator);
                    let ind_release = Arc::clone(&indicator);
                    let ctx_press = egui_ctx.clone();
                    let ctx_release = egui_ctx.clone();
                    let stt_tx = stt_tx.clone();
                    let stt_press = stt_tx.clone();
                    let trace_ready = Arc::clone(&trace_report);
                    let trace_release = Arc::clone(&trace_report);
                    let models_press = Arc::clone(&models_state);
                    // Eine PttSession pro (erfolgreich) installiertem Tap — ersetzt
                    // die früheren thread_local!-Zellen (Ticket-0038). Ein Retry
                    // dieser Factory (permissions::should_relaunch_for_tap) findet
                    // nie eine laufende Aufnahme vor, weil der alte Tap dafür schon
                    // aktiv gewesen sein müsste.
                    let mic_cache = audio::MicCache::default();
                    let session_press = Arc::new(Mutex::new(pipeline::PttSession::new(
                        Box::new(move |cfg| {
                            Box::new(audio::MicSource::new(
                                cfg.mic_device.clone(),
                                mic_cache.clone(),
                            ))
                        }),
                        Arc::clone(&trace_report),
                    )));
                    let session_release = Arc::clone(&session_press);
                    hotkey::install(
                        Arc::clone(&config),
                        move |pressed_at| {
                            let cfg = mic_config.read().map(|c| c.clone()).unwrap_or_default();
                            // Balken sofort zeigen; der Start-Thread schaltet auf
                            // Wellen um, sobald das Mikrofon wirklich läuft.
                            if let Ok(mut ind) = ind_press.lock() {
                                ind.starting(Instant::now());
                            }
                            ctx_press.request_repaint();
                            let (ind_ready, ctx_ready, stt_ready, trace_ready) = (
                                Arc::clone(&ind_press),
                                ctx_press.clone(),
                                stt_press.clone(),
                                Arc::clone(&trace_ready),
                            );
                            let on_ready = move |ready: pipeline::MicReady| {
                                match ready {
                                    pipeline::MicReady::Recording {
                                        level,
                                        signal,
                                        mode,
                                        live,
                                    } => {
                                        if let Some(feed) = live
                                            && let Err(mpsc::SendError(job)) =
                                                stt_ready.send(pipeline::SttJob::Live { feed })
                                        {
                                            log::error!("STT-Worker nicht verfügbar.");
                                            let mut trace = job.into_trace();
                                            trace.result = DictationResult::Error(
                                                "STT-Worker nicht verfügbar".into(),
                                            );
                                            trace_ready(trace);
                                        }
                                        // Der Indicator besitzt den Aufnahme-Status
                                        // (inkl. aufgelöstem Modus); das Tray leitet
                                        // sein Icon daraus ab (Ticket-0035).
                                        if let Ok(mut ind) = ind_ready.lock() {
                                            ind.mic_started(level, signal, mode);
                                        }
                                    }
                                    pipeline::MicReady::Failed(e) => {
                                        log::warn!("{e}");
                                        if let Ok(mut ind) = ind_ready.lock() {
                                            ind.fail(Instant::now(), "Mikrofon nicht verfügbar");
                                        }
                                    }
                                }
                                ctx_ready.request_repaint();
                            };
                            let outcome = session_press
                                .lock()
                                .map(|mut s| s.press(pressed_at, &cfg, &models_press, on_ready))
                                .unwrap_or(pipeline::PressOutcome::Blocked(
                                    "Interner Fehler (Session)".into(),
                                ));
                            if let pipeline::PressOutcome::Blocked(hint) = outcome {
                                log::debug!("PTT gesperrt — {hint}");
                                if let Ok(mut ind) = ind_press.lock() {
                                    ind.fail(Instant::now(), hint);
                                }
                                ctx_press.request_repaint();
                            }
                        },
                        move |released_at| {
                            let outcome = session_release
                                .lock()
                                .map(|mut s| s.release(released_at))
                                .unwrap_or(pipeline::ReleaseOutcome::NoRecording);
                            let update = |f: &dyn Fn(&mut Indicator)| {
                                if let Ok(mut ind) = ind_release.lock() {
                                    f(&mut ind);
                                }
                                ctx_release.request_repaint();
                            };
                            match outcome {
                                pipeline::ReleaseOutcome::NoRecording => {}
                                pipeline::ReleaseOutcome::StartAborted => {
                                    log::debug!("Taste vor dem Mikrofon-Start losgelassen.");
                                    update(&|ind| ind.cancel());
                                }
                                pipeline::ReleaseOutcome::LiveEnded => {
                                    update(&|ind| ind.transcribing(Instant::now()));
                                }
                                pipeline::ReleaseOutcome::TooShort { ms } => {
                                    log::debug!("Utterance zu kurz ({ms} ms) — verworfen.");
                                    update(&|ind| ind.cancel());
                                }
                                pipeline::ReleaseOutcome::Ready {
                                    pcm,
                                    frontmost,
                                    ms,
                                    captured,
                                    trace,
                                } => {
                                    log::debug!(
                                        "Utterance aufgenommen: {ms} ms, {} Samples @16 kHz.",
                                        pcm.len()
                                    );
                                    let job = pipeline::SttJob::Utterance {
                                        pcm,
                                        frontmost,
                                        captured,
                                        trace,
                                    };
                                    if let Err(mpsc::SendError(job)) = stt_tx.send(job) {
                                        log::error!(
                                            "STT-Worker nicht verfügbar — Utterance verworfen."
                                        );
                                        let mut trace = job.into_trace();
                                        trace.result = DictationResult::Error(
                                            "STT-Worker nicht verfügbar".into(),
                                        );
                                        trace_release(trace);
                                        update(&|ind| {
                                            ind.fail(Instant::now(), "STT nicht verfügbar")
                                        });
                                    } else {
                                        update(&|ind| ind.transcribing(Instant::now()));
                                    }
                                }
                                pipeline::ReleaseOutcome::StopFailed(e) => {
                                    log::error!("{e}");
                                    update(&|ind| {
                                        ind.fail(Instant::now(), "Aufnahme fehlgeschlagen")
                                    });
                                }
                            }
                        },
                    )
                }
            };

            match install_tap() {
                Ok(tap) => {
                    // Tap muss so lange leben wie die App.
                    std::mem::forget(tap);
                    log::info!("bereit — PTT-Taste halten = diktieren.");
                }
                Err(e) => {
                    tray::with_instance(|t| t.set_permission_warning());
                    log::warn!("{e} — versuche alle 3 s erneut.");
                    use objc2_foundation::NSTimer;
                    // Soundness wie beim Overlay-Timer: geplant + gefeuert nur
                    // auf dem Main-RunLoop, der Block verlässt den Thread nie.
                    let models_relaunch = Arc::clone(&models_state);
                    let block = block2::RcBlock::new(move |timer: std::ptr::NonNull<NSTimer>| {
                        if let Ok(tap) = install_tap() {
                            std::mem::forget(tap);
                            tray::with_instance(|t| t.clear_permission_warning());
                            log::info!("Event-Tap nachträglich installiert — bereit.");
                            unsafe { timer.as_ref().invalidate() };
                        } else if permissions::should_relaunch_for_tap(
                            accessibility,
                            permissions::accessibility_granted(),
                        ) && !models_relaunch.any_download_running()
                        {
                            // TCC cached die Accessibility-Entscheidung pro
                            // Prozess — nach Laufzeit-Grant hilft nur ein
                            // Neustart (Ticket-0030). Laufende Modell-Downloads
                            // erst fertig laufen lassen (kein 5-GB-Abbruch).
                            relaunch();
                        }
                    });
                    let timer = unsafe {
                        NSTimer::scheduledTimerWithTimeInterval_repeats_block(3.0, true, &block)
                    };
                    std::mem::forget(timer);
                }
            }

            Ok(Box::new(ui::SettingsApp::new(
                Arc::clone(&config),
                tray.settings_id.clone(),
                tray.quit_id.clone(),
                !show_onboarding,
                Arc::clone(&indicator),
                Arc::clone(&models_state),
                models_root.clone(),
            )))
        }),
    );

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            log::error!("UI-Start fehlgeschlagen: {e}");
            ExitCode::FAILURE
        }
    }
}
