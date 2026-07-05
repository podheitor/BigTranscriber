//! Read-only audio capture via `pw-record`. We only *read* from existing
//! PipeWire nodes (a sink's `.monitor` for system audio, and a mic source);
//! we never change defaults, create/destroy nodes, or touch Bluetooth.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use chrono::{DateTime, Local};
use tauri::{AppHandle, Emitter};

use crate::email::EmailSender;
use crate::stt::Model;

const SR: usize = 16000;
const METER_WINDOW: usize = SR / 10; // report an input level ~10x/second

#[derive(Clone, serde::Serialize)]
pub struct Line {
    pub time: String, // HH:MM:SS (wall clock)
    pub who: String,
    pub text: String,
    pub channel: String, // "sys" | "mic"
}

#[derive(Clone, serde::Serialize)]
struct LevelEvent {
    channel: String, // "sys" | "mic"
    level: f32,      // normalized RMS  (0..1, dBFS-mapped)
    peak: f32,       // normalized peak (0..1, dBFS-mapped)
}

/// Rolling input-level meter: accumulates ~100 ms of samples, then reports a
/// normalized RMS + peak for the UI VU meter. No-op when there's no GUI.
struct Meter {
    on: bool,
    sumsq: f64,
    peak: f32,
    count: usize,
}

impl Meter {
    fn new(on: bool) -> Self {
        Self { on, sumsq: 0.0, peak: 0.0, count: 0 }
    }

    /// Feed one sample; returns `Some((level, peak))` once a window completes.
    fn push(&mut self, s: i16) -> Option<(f32, f32)> {
        if !self.on {
            return None;
        }
        let f = s as f32 / 32768.0;
        let a = f.abs();
        self.sumsq += (f as f64) * (f as f64);
        if a > self.peak {
            self.peak = a;
        }
        self.count += 1;
        if self.count >= METER_WINDOW {
            let rms = (self.sumsq / self.count as f64).sqrt() as f32;
            let out = (norm_db(rms), norm_db(self.peak));
            self.sumsq = 0.0;
            self.peak = 0.0;
            self.count = 0;
            Some(out)
        } else {
            None
        }
    }
}

/// Map a 0..1 amplitude to a 0..1 meter reading on a dBFS scale (-60..0 dB).
fn norm_db(x: f32) -> f32 {
    if x <= 1e-6 {
        return 0.0;
    }
    let db = 20.0 * x.log10();
    ((db + 60.0) / 60.0).clamp(0.0, 1.0)
}

fn emit_level(app: &Option<AppHandle>, channel: &str, level: f32, peak: f32) {
    if let Some(h) = app {
        let _ = h.emit("level", &LevelEvent { channel: channel.to_string(), level, peak });
    }
}

pub struct ChannelWorker {
    pub child: Child,
    pub handle: JoinHandle<()>,
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_channel(
    app: Option<AppHandle>,
    stop: Arc<AtomicBool>,
    source: String,
    who: String,
    channel: String,
    model: Arc<Model>,
    language: String,
    segment_secs: u32,
    n_threads: i32,
    start_time: DateTime<Local>,
    wav_path: PathBuf,
    transcript: Arc<Mutex<std::fs::File>>,
    email: Option<Arc<EmailSender>>,
) -> Result<ChannelWorker, String> {
    let mut child = Command::new("pw-record")
        .args([
            "--container=raw",
            &format!("--target={source}"),
            "--rate=16000",
            "--channels=1",
            "--format=s16",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to start pw-record for '{source}': {e}"))?;

    let mut stdout = child.stdout.take().ok_or("no stdout from pw-record")?;

    let handle = std::thread::spawn(move || {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: SR as u32,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&wav_path, spec).ok();
        let mut meter = Meter::new(app.is_some());

        let chunk_samples = (segment_secs as usize) * SR;
        let mut pcm: Vec<i16> = Vec::with_capacity(chunk_samples + SR);
        let mut consumed: usize = 0;
        let mut byte = [0u8; 8192];
        // pw-record can emit a stray odd byte across reads; keep a 1-byte carry.
        let mut carry: Option<u8> = None;

        loop {
            match stdout.read(&mut byte) {
                Ok(0) => break,
                Ok(n) => {
                    let mut i = 0;
                    if let Some(lo) = carry.take() {
                        if n >= 1 {
                            let s = i16::from_le_bytes([lo, byte[0]]);
                            push_sample(s, &mut pcm, writer.as_mut());
                            if let Some((l, p)) = meter.push(s) { emit_level(&app, &channel, l, p); }
                            i = 1;
                        } else {
                            carry = Some(lo);
                        }
                    }
                    while i + 1 < n {
                        let s = i16::from_le_bytes([byte[i], byte[i + 1]]);
                        push_sample(s, &mut pcm, writer.as_mut());
                        if let Some((l, p)) = meter.push(s) { emit_level(&app, &channel, l, p); }
                        i += 2;
                    }
                    if i < n {
                        carry = Some(byte[i]);
                    }
                    while pcm.len() >= chunk_samples {
                        let chunk: Vec<i16> = pcm.drain(..chunk_samples).collect();
                        process_chunk(&app, &model, &language, n_threads, &who, &channel,
                                      start_time, consumed, &chunk, &transcript, &email);
                        consumed += chunk_samples;
                    }
                }
                Err(_) => break,
            }
            if stop.load(Ordering::Relaxed) {
                break;
            }
        }

        if !pcm.is_empty() {
            let chunk = std::mem::take(&mut pcm);
            process_chunk(&app, &model, &language, n_threads, &who, &channel,
                          start_time, consumed, &chunk, &transcript, &email);
        }
        if let Some(w) = writer {
            let _ = w.finalize();
        }
    });

    Ok(ChannelWorker { child, handle })
}

#[inline]
fn push_sample(s: i16, pcm: &mut Vec<i16>, writer: Option<&mut hound::WavWriter<std::io::BufWriter<std::fs::File>>>) {
    pcm.push(s);
    if let Some(w) = writer {
        let _ = w.write_sample(s);
    }
}

#[allow(clippy::too_many_arguments)]
fn process_chunk(
    app: &Option<AppHandle>,
    model: &Model,
    language: &str,
    n_threads: i32,
    who: &str,
    channel: &str,
    start_time: DateTime<Local>,
    offset_samples: usize,
    chunk: &[i16],
    transcript: &Arc<Mutex<std::fs::File>>,
    email: &Option<Arc<EmailSender>>,
) {
    // Silence gate: skip near-silent chunks so we don't burn CPU on nothing.
    let mean_sq: f64 =
        chunk.iter().map(|&s| (s as f64) * (s as f64)).sum::<f64>() / chunk.len().max(1) as f64;
    if mean_sq.sqrt() < 40.0 {
        return;
    }

    let audio: Vec<f32> = chunk.iter().map(|&s| s as f32 / 32768.0).collect();
    let base = offset_samples as f64 / SR as f64;

    match model.transcribe(&audio, language, n_threads) {
        Ok(utts) => {
            for u in utts {
                let secs = base + u.start_secs;
                let clock = start_time + chrono::Duration::milliseconds((secs * 1000.0) as i64);
                let time = clock.format("%H:%M:%S").to_string();
                let line = Line {
                    time: time.clone(),
                    who: who.to_string(),
                    text: u.text.clone(),
                    channel: channel.to_string(),
                };
                match app {
                    Some(h) => {
                        let _ = h.emit("transcript", &line);
                    }
                    None => println!("[{}] {}: {}", line.time, line.who, line.text),
                }
                if let Ok(mut f) = transcript.lock() {
                    let _ = writeln!(f, "[{}] {}: {}", time, who, u.text);
                }
                if let Some(em) = email {
                    em.note_line(format!("[{}] {}: {}", time, who, u.text));
                }
            }
        }
        Err(e) => {
            if let Some(h) = app {
                let _ = h.emit("error", e);
            } else {
                eprintln!("[error] {e}");
            }
        }
    }
}
