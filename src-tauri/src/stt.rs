//! Offline speech-to-text via whisper-rs (whisper.cpp). CPU by default.

use std::sync::Arc;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// A loaded Whisper model, shared (read-only) across channel workers.
/// Each transcription creates its own state, so workers run independently.
pub struct Model {
    ctx: Arc<WhisperContext>,
}

/// One recognized utterance.
pub struct Utterance {
    /// Start offset within the supplied audio, in seconds.
    pub start_secs: f64,
    pub text: String,
}

impl Model {
    pub fn load(model_path: &str) -> Result<Model, String> {
        // Silence whisper.cpp/ggml stderr spam, unless BT_VERBOSE=1 (shows the
        // Vulkan device it selected, useful for diagnostics).
        if std::env::var("BT_VERBOSE").is_err() {
            whisper_rs::install_logging_hooks();
        }

        let mut params = WhisperContextParameters::default();
        params.use_gpu = true; // GPU (Vulkan) build; falls back to CPU if unavailable
        // Pick which Vulkan device to use (0 = first). Override if the NVIDIA
        // card isn't device 0: BIGTRANSCRIBER_GPU=N
        if let Ok(d) = std::env::var("BIGTRANSCRIBER_GPU") {
            if let Ok(n) = d.parse::<i32>() {
                params.gpu_device = n;
            }
        }

        let ctx = WhisperContext::new_with_params(model_path, params)
            .map_err(|e| format!("failed to load model '{model_path}': {e}"))?;
        Ok(Model { ctx: Arc::new(ctx) })
    }

    /// Transcribe a mono 16 kHz f32 buffer. `language` "" or "auto" => auto-detect.
    pub fn transcribe(
        &self,
        audio: &[f32],
        language: &str,
        n_threads: i32,
    ) -> Result<Vec<Utterance>, String> {
        let mut state = self
            .ctx
            .create_state()
            .map_err(|e| format!("whisper state error: {e}"))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(n_threads);
        if !language.is_empty() && language != "auto" {
            params.set_language(Some(language));
        }
        params.set_translate(false);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_no_context(true);
        params.set_suppress_blank(true);

        state
            .full(params, audio)
            .map_err(|e| format!("transcription failed: {e}"))?;

        let n = state.full_n_segments();

        let mut out = Vec::new();
        for i in 0..n {
            let Some(seg) = state.get_segment(i) else {
                continue;
            };
            let text = match seg.to_str_lossy() {
                Ok(t) => t.trim().to_string(),
                Err(_) => continue,
            };
            if text.is_empty() {
                continue;
            }
            // start_timestamp is in centiseconds (1/100 s).
            out.push(Utterance {
                start_secs: seg.start_timestamp() as f64 / 100.0,
                text,
            });
        }
        Ok(out)
    }
}
