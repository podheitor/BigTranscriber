// Prevents an extra console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod email;
mod stt;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use chrono::Local;
use tauri::State;

use audio::{spawn_channel, ChannelWorker};
use email::{EmailConfig, EmailSender};
use stt::Model;

/// Where models live and sessions are saved: ~/Projeto/BigTranscriber
fn base_dir() -> PathBuf {
    if let Ok(p) = std::env::var("BIGTRANSCRIBER_HOME") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join("Projeto").join("BigTranscriber")
}

#[derive(Default)]
struct AppState {
    session: Mutex<Option<Session>>,
}

struct Session {
    stop: Arc<AtomicBool>,
    workers: Vec<ChannelWorker>,
    out_dir: PathBuf,
    /// Present only when auto-e-mail is enabled for this session.
    email: Option<Arc<EmailSender>>,
    email_thread: Option<JoinHandle<()>>,
    timer_thread: Option<JoinHandle<()>>,
}

#[derive(serde::Serialize)]
struct Source {
    name: String,
    is_monitor: bool,
}

#[derive(serde::Serialize)]
struct Defaults {
    sink_monitor: String,
    default_source: String,
}

#[derive(serde::Deserialize)]
struct StartOpts {
    sys_source: Option<String>,
    mic_source: Option<String>,
    model: String,
    language: String,
    segment_secs: u32,
    sys_label: String,
    mic_label: String,
    // --- Auto-e-mail of the transcript ---
    email_enabled: bool,
    email_to: String,
    /// Send every N minutes (0 = off).
    email_every_minutes: u32,
    /// Send every N lines (0 = off).
    email_every_lines: u32,
}

fn run_pactl(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("pactl")
        .args(args)
        .output()
        .map_err(|e| format!("pactl failed: {e}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[tauri::command]
fn list_sources() -> Result<Vec<Source>, String> {
    let text = run_pactl(&["list", "short", "sources"])?;
    let mut out = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 2 {
            continue;
        }
        let name = cols[1].to_string();
        let is_monitor = name.ends_with(".monitor");
        out.push(Source { name, is_monitor });
    }
    Ok(out)
}

#[tauri::command]
fn defaults() -> Result<Defaults, String> {
    let sink = run_pactl(&["get-default-sink"])?;
    let source = run_pactl(&["get-default-source"])?;
    Ok(Defaults {
        sink_monitor: format!("{sink}.monitor"),
        default_source: source,
    })
}

#[tauri::command]
fn list_models() -> Vec<String> {
    let dir = base_dir().join("models");
    let mut models = Vec::new();
    if let Ok(rd) = fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "bin").unwrap_or(false) {
                if let Some(n) = p.file_name().and_then(|s| s.to_str()) {
                    models.push(n.to_string());
                }
            }
        }
    }
    models.sort();
    models
}

#[tauri::command]
fn session_status(state: State<AppState>) -> bool {
    state.session.lock().unwrap().is_some()
}

#[tauri::command]
fn start_session(
    app: tauri::AppHandle,
    state: State<AppState>,
    opts: StartOpts,
) -> Result<String, String> {
    let mut guard = state.session.lock().unwrap();
    if guard.is_some() {
        return Err("A session is already running.".into());
    }

    if opts.sys_source.is_none() && opts.mic_source.is_none() {
        return Err("Select at least one source (system audio and/or microphone).".into());
    }

    // Load the model once, share across channels.
    let model_path = base_dir().join("models").join(&opts.model);
    if !model_path.exists() {
        return Err(format!(
            "Model not found: {}. Download it first (scripts/get-model.sh).",
            model_path.display()
        ));
    }
    let model = Arc::new(Model::load(model_path.to_str().unwrap())?);

    // Session output folder.
    let start_time = Local::now();
    let stamp = start_time.format("%Y%m%d_%H%M%S").to_string();
    let out_dir = base_dir().join("recordings").join(&stamp);
    fs::create_dir_all(&out_dir).map_err(|e| format!("cannot create {}: {e}", out_dir.display()))?;

    let transcript_path = out_dir.join("transcript.txt");
    let file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&transcript_path)
        .map_err(|e| format!("cannot open transcript: {e}"))?;
    let transcript = Arc::new(Mutex::new(file));

    let active = opts.sys_source.is_some() as i32 + opts.mic_source.is_some() as i32;
    let total = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) as i32;
    let n_threads = (total / active.max(1)).clamp(2, 16);

    // Optional auto-e-mail of the transcript (every N minutes and/or N lines).
    let (email, email_thread) = if opts.email_enabled
        && !opts.email_to.trim().is_empty()
        && (opts.email_every_minutes > 0 || opts.email_every_lines > 0)
    {
        let cfg = EmailConfig {
            to: opts.email_to.trim().to_string(),
            every: Duration::from_secs(opts.email_every_minutes as u64 * 60),
            max_lines: opts.email_every_lines as usize,
            subject_prefix: "BigTranscriber — transcrição".to_string(),
            python: email::default_python(),
            script: email::default_script(),
            reply_to: email::default_reply_to(),
        };
        let (sender, handle) = EmailSender::spawn(cfg, Some(app.clone()));
        (Some(sender), Some(handle))
    } else {
        (None, None)
    };

    let stop = Arc::new(AtomicBool::new(false));
    let mut workers: Vec<ChannelWorker> = Vec::new();

    if let Some(src) = opts.sys_source.clone() {
        let w = spawn_channel(
            Some(app.clone()), stop.clone(), src, opts.sys_label.clone(), "sys".into(),
            model.clone(), opts.language.clone(), opts.segment_secs, n_threads,
            start_time, out_dir.join("system.wav"), transcript.clone(), email.clone(),
        )?;
        workers.push(w);
    }
    if let Some(src) = opts.mic_source.clone() {
        let w = spawn_channel(
            Some(app.clone()), stop.clone(), src, opts.mic_label.clone(), "mic".into(),
            model.clone(), opts.language.clone(), opts.segment_secs, n_threads,
            start_time, out_dir.join("mic.wav"), transcript.clone(), email.clone(),
        )?;
        workers.push(w);
    }

    // Timer thread: fires the "every N minutes" trigger. Polls in short steps so
    // stopping the session is responsive; tick() only sends once the interval passes.
    let timer_thread = email.as_ref().map(|em| {
        let em = em.clone();
        let stop = stop.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                for _ in 0..8 {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
                em.tick();
            }
        })
    });

    *guard = Some(Session {
        stop,
        workers,
        out_dir: out_dir.clone(),
        email,
        email_thread,
        timer_thread,
    });
    Ok(out_dir.display().to_string())
}

#[tauri::command]
fn stop_session(state: State<AppState>) -> Result<String, String> {
    let session = state.session.lock().unwrap().take();
    let Some(mut session) = session else {
        return Err("No session running.".into());
    };
    session.stop.store(true, Ordering::Relaxed);
    // Killing pw-record closes its stdout, which unblocks the reader thread.
    // SIGKILL is fine: capture is raw PCM and the WAV is finalized in-thread on EOF.
    for w in session.workers.drain(..) {
        let mut child = w.child;
        let _ = child.kill();
        let _ = child.wait();
        let _ = w.handle.join();
    }
    // Capture is done → flush any buffered lines as a final e-mail, then tear the
    // e-mail threads down in order: join the timer, drop the last Arc<EmailSender>
    // (closes the channel), then join the sender so the last send completes.
    if let Some(em) = session.email.take() {
        em.flush_now();
        if let Some(t) = session.timer_thread.take() {
            let _ = t.join();
        }
        drop(em);
        if let Some(t) = session.email_thread.take() {
            let _ = t.join();
        }
    }
    Ok(session.out_dir.display().to_string())
}

/// Force-send the buffered transcript now (UI "Enviar agora").
#[tauri::command]
fn send_email_now(state: State<AppState>) -> Result<String, String> {
    let guard = state.session.lock().unwrap();
    let Some(session) = guard.as_ref() else {
        return Err("Nenhuma sessão em execução.".into());
    };
    match &session.email {
        Some(em) => {
            let n = em.flush_now();
            if n == 0 {
                Ok("Nada novo para enviar ainda.".into())
            } else {
                Ok(format!("Enviando {n} linha(s) por e-mail…"))
            }
        }
        None => Err("E-mail automático não está ativado nesta sessão.".into()),
    }
}

/// Headless self-test: `bigtranscriber transcribe <model.bin> <audio.wav>`
/// Loads the model and prints the transcript. Verifies STT without the GUI.
fn cli_transcribe(model_path: &str, wav_path: &str) -> Result<(), String> {
    let t_load = std::time::Instant::now();
    let model = Model::load(model_path)?;
    let load_ms = t_load.elapsed().as_millis();
    let mut reader =
        hound::WavReader::open(wav_path).map_err(|e| format!("cannot read wav: {e}"))?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .samples::<i32>()
            .filter_map(|s| s.ok())
            .map(|s| s as f32 / 32768.0) // our pipeline is 16-bit PCM
            .collect(),
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(|s| s.ok()).collect(),
    };
    // whisper wants 16k mono; assume the input already is (our pipeline produces that).
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) as i32;
    let audio_secs = samples.len() as f64 / 16000.0;
    // Run a few times: the first call includes one-time Vulkan shader
    // compilation; later calls reflect steady-state (live) performance.
    for i in 0..3 {
        let t_inf = std::time::Instant::now();
        let utts = model.transcribe(&samples, "pt", threads)?;
        let inf_ms = t_inf.elapsed().as_millis();
        if i == 0 {
            println!("--- transcript ({} segments) ---", utts.len());
            for u in &utts {
                println!("[{:6.2}s] {}", u.start_secs, u.text);
            }
        }
        let rtf = inf_ms as f64 / 1000.0 / audio_secs.max(0.001);
        let tag = if i == 0 { "cold" } else { "warm" };
        println!("--- run {i} ({tag}): audio={audio_secs:.1}s inference={inf_ms}ms RTF={rtf:.2}x ---");
    }
    println!("(load={load_ms}ms threads={threads})");
    Ok(())
}

/// Headless live test of the real capture+transcribe pipeline (no GUI):
/// `bigtranscriber live <model.bin> [source] [seconds]`
/// Captures from `source` (default: system default source) and prints lines.
fn cli_live(model_path: &str, source: Option<&str>, seconds: u64) -> Result<(), String> {
    use std::sync::atomic::AtomicBool;
    let model = Arc::new(Model::load(model_path)?);
    let src = match source {
        Some(s) => s.to_string(),
        None => run_pactl(&["get-default-source"])?,
    };
    let out_dir = std::env::temp_dir().join("bigtranscriber_live");
    fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let file = fs::File::create(out_dir.join("transcript.txt")).map_err(|e| e.to_string())?;
    let transcript = Arc::new(Mutex::new(file));
    let stop = Arc::new(AtomicBool::new(false));
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) as i32;

    eprintln!("[live] capturing '{src}' for {seconds}s with model {model_path} ...");
    let worker = spawn_channel(
        None, stop.clone(), src, "VOZ".into(), "mic".into(),
        model, "pt".into(), 8, threads, Local::now(),
        out_dir.join("live.wav"), transcript, None,
    )?;
    std::thread::sleep(std::time::Duration::from_secs(seconds));
    stop.store(true, Ordering::Relaxed);
    let mut child = worker.child;
    let _ = child.kill();
    let _ = child.wait();
    let _ = worker.handle.join();
    eprintln!("[live] done. wav+transcript in {}", out_dir.display());
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 4 && args[1] == "transcribe" {
        match cli_transcribe(&args[2], &args[3]) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }
    if args.len() >= 3 && args[1] == "live" {
        let source = args.get(3).map(|s| s.as_str());
        let seconds = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(15);
        match cli_live(&args[2], source, seconds) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }

    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            list_sources,
            defaults,
            list_models,
            session_status,
            start_session,
            stop_session,
            send_email_now,
        ])
        .run(tauri::generate_context!())
        .expect("error while running BigTranscriber");
}
