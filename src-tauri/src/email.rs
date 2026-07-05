//! Automatic e-mailing of the live transcript.
//!
//! Finalized transcript lines are buffered here and flushed as an e-mail on two
//! independent triggers: **every N minutes** and/or **every N lines** (either can
//! be disabled with 0). Sending happens on a dedicated background thread so the
//! capture/transcription pipeline never blocks on the network.
//!
//! Delivery reuses the user's existing Gmail API helper (`send_gmail.py`, OAuth
//! token, scope `gmail.send`) — the sanctioned way to send *From* the personal
//! Gmail. We never embed SMTP credentials. The interpreter/script/reply-to are
//! overridable via env vars so the app isn't hard-wired to one machine layout.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::Local;
use tauri::{AppHandle, Emitter};

/// Resolved delivery settings for one session.
#[derive(Clone)]
pub struct EmailConfig {
    pub to: String,
    /// Time trigger; `Duration::ZERO` disables it.
    pub every: Duration,
    /// Line-count trigger; `0` disables it.
    pub max_lines: usize,
    pub subject_prefix: String,
    pub python: String,
    pub script: String,
    pub reply_to: String,
}

struct Batch {
    buf: Vec<String>,
    last: Instant,
}

/// Shared handle: worker threads push lines, a timer thread ticks, and the UI can
/// force a flush. Cheap to `clone()` (it's held behind an `Arc`).
pub struct EmailSender {
    cfg: EmailConfig,
    batch: Mutex<Batch>,
    tx: Sender<String>,
}

/// Python interpreter used to run the Gmail helper.
/// Override with `BIGTRANSCRIBER_GMAIL_PYTHON`; defaults to `python3` on PATH.
pub fn default_python() -> String {
    std::env::var("BIGTRANSCRIBER_GMAIL_PYTHON").unwrap_or_else(|_| "python3".into())
}

/// Gmail helper script (see `scripts/send_gmail.py`, which needs your own Google
/// OAuth token). Override with `BIGTRANSCRIBER_GMAIL_PY`.
pub fn default_script() -> String {
    std::env::var("BIGTRANSCRIBER_GMAIL_PY").unwrap_or_else(|_| "scripts/send_gmail.py".into())
}

/// Optional Reply-To. Empty by default; set `BIGTRANSCRIBER_EMAIL_REPLYTO` if you
/// want replies to go somewhere other than the sending account.
pub fn default_reply_to() -> String {
    std::env::var("BIGTRANSCRIBER_EMAIL_REPLYTO").unwrap_or_default()
}

impl EmailSender {
    /// Start the background sender thread. Returns the shareable sender plus the
    /// thread's `JoinHandle` — join it *after* dropping every `Arc<EmailSender>`
    /// so the last queued e-mail is guaranteed to go out.
    pub fn spawn(cfg: EmailConfig, app: Option<AppHandle>) -> (Arc<EmailSender>, JoinHandle<()>) {
        let (tx, rx) = channel::<String>();
        let loop_cfg = cfg.clone();
        let handle = std::thread::spawn(move || {
            // Ends when every Sender is dropped (channel closed).
            while let Ok(body) = rx.recv() {
                send_body(&loop_cfg, &body, &app);
            }
        });
        let sender = Arc::new(EmailSender {
            cfg,
            batch: Mutex::new(Batch { buf: Vec::new(), last: Instant::now() }),
            tx,
        });
        (sender, handle)
    }

    /// Record one finalized transcript line; may trigger a line-count flush.
    pub fn note_line(&self, line: String) {
        let mut b = self.batch.lock().unwrap();
        b.buf.push(line);
        if self.cfg.max_lines > 0 && b.buf.len() >= self.cfg.max_lines {
            if let Some(body) = drain(&mut b) {
                drop(b); // release the lock before touching the channel
                let _ = self.tx.send(body);
            }
        }
    }

    /// Called periodically by the timer thread; flushes if the interval elapsed.
    pub fn tick(&self) {
        if self.cfg.every.is_zero() {
            return;
        }
        let mut b = self.batch.lock().unwrap();
        if !b.buf.is_empty() && b.last.elapsed() >= self.cfg.every {
            if let Some(body) = drain(&mut b) {
                drop(b);
                let _ = self.tx.send(body);
            }
        }
    }

    /// Force-send whatever is buffered right now (UI "send now" and on stop).
    /// Returns the number of lines queued, so the caller can report it.
    pub fn flush_now(&self) -> usize {
        let mut b = self.batch.lock().unwrap();
        match drain(&mut b) {
            Some(body) => {
                let n = body.lines().count();
                drop(b);
                let _ = self.tx.send(body);
                n
            }
            None => 0,
        }
    }
}

/// Move the buffered lines out as a single body, resetting the timer.
fn drain(b: &mut Batch) -> Option<String> {
    if b.buf.is_empty() {
        return None;
    }
    let body = b.buf.join("\n");
    b.buf.clear();
    b.last = Instant::now();
    Some(body)
}

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Blocking send of one batch via the Gmail helper. Runs only on the sender
/// thread, so a slow network can never stall transcription.
fn send_body(cfg: &EmailConfig, body: &str, app: &Option<AppHandle>) {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "bigtranscriber_email_{}_{}.txt",
        std::process::id(),
        n
    ));

    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let line_count = body.lines().count();
    let full = format!(
        "Transcrição automática — BigTranscriber\n{now} — {line_count} linha(s)\n\n{body}\n"
    );
    if let Err(e) = std::fs::write(&path, &full) {
        report(app, format!("e-mail: não foi possível gravar o corpo: {e}"), false);
        return;
    }

    let subject = format!("{} — {now}", cfg.subject_prefix);
    let mut cmd = std::process::Command::new(&cfg.python);
    cmd.arg(&cfg.script)
        .arg("--to")
        .arg(&cfg.to)
        .arg("--subject")
        .arg(&subject)
        .arg("--body-file")
        .arg(&path);
    if !cfg.reply_to.is_empty() {
        cmd.arg("--reply-to").arg(&cfg.reply_to);
    }
    let out = cmd.output();
    let _ = std::fs::remove_file(&path);

    match out {
        Ok(o) if o.status.success() => {
            report(app, format!("e-mail enviado para {} ({line_count} linhas)", cfg.to), true);
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            let tail = stderr.trim().lines().last().unwrap_or("").trim().to_string();
            let detail = if tail.is_empty() { "erro desconhecido".to_string() } else { tail };
            report(app, format!("falha ao enviar e-mail: {detail}"), false);
        }
        Err(e) => report(
            app,
            format!("não foi possível executar o helper de e-mail ({}): {e}", cfg.python),
            false,
        ),
    }
}

fn report(app: &Option<AppHandle>, msg: String, ok: bool) {
    match app {
        Some(h) => {
            let _ = h.emit(if ok { "email_ok" } else { "email_err" }, msg);
        }
        None => {
            if ok {
                println!("[email] {msg}");
            } else {
                eprintln!("[email] {msg}");
            }
        }
    }
}
