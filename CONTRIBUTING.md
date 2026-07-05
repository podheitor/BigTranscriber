# Contributing to BigTranscriber

Thanks for your interest! This is a small, focused desktop app; contributions that
keep it simple and offline-first are very welcome.

## Project layout

```
dist/                 frontend (plain HTML/CSS/JS — no bundler)
src-tauri/src/
  main.rs             Tauri commands, session state, CLI subcommands
  audio.rs            read-only pw-record capture + input-level metering
  stt.rs              whisper-rs (whisper.cpp) inference
  email.rs            transcript batching + Gmail-helper delivery
scripts/              build.sh, get-model.sh, send_gmail.py (reference)
packaging/            desktop entry used by the PKGBUILD
.github/workflows/    CI release pipeline
```

## Building

- **GPU (Vulkan):** `./scripts/build.sh` (fetches a header-only Vulkan SDK the first
  time), then `cd src-tauri && cargo build --release --features gpu`.
- **CPU-only (default):** `cargo build --release`.
- Grab a model first: `./scripts/get-model.sh small` (or `large-v3`).

The GPU backend is gated behind the opt-in `gpu` Cargo feature
(`gpu = ["whisper-rs/vulkan"]`); CI and portable release builds are plain CPU builds.

## Guidelines

- Keep capture **read-only** — never change the user's audio defaults, devices, or Bluetooth.
- Keep inference **in-process** — no Python/sidecar for core transcription.
- No secrets or personal paths in source. Configuration goes through env vars
  (see the auto-email section in the README).
- `cargo fmt` before opening a PR; keep changes minimal and well-commented where non-obvious.

## Reporting issues

Include your distro, GPU + driver, whether you built GPU or CPU, and the relevant
lines from a `BT_VERBOSE=1` run if it's a transcription/GPU issue.
