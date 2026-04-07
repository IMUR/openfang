//! Silero VAD v5 — Voice Activity Detection via Candle.
//!
//! Loads the `idle-intelligence/silero-vad-v5-safetensors` model and runs
//! inference on CPU. Processes 512-sample chunks at 16 kHz (32 ms) and returns
//! a speech probability ∈ [0, 1].
//!
//! Architecture: STFT conv → magnitude → 4× Conv1d encoder → LSTM → Conv1d decoder → sigmoid.
//!
//! ## Design
//!
//! Follows the same pattern as `candle_reranker.rs` / `candle_ner.rs`:
//! - Loaded at boot, stored as `Option<Arc<SileroVad>>` on the kernel
//! - Gated behind the `memory-candle` feature flag (shares candle deps)
//! - Passed into voice sessions as a parameter, not owned by them
//! - Thread-safe via `Arc<Mutex<Inner>>` — sequential access is fine for VAD

use candle_core::{DType, Device, Tensor};
use candle_nn::{conv1d, conv1d_no_bias, lstm, Conv1d, Conv1dConfig, Module, VarBuilder, LSTM};
use candle_nn::rnn::{LSTMConfig, LSTMState, RNN};
use std::sync::{Arc, Mutex};
use tracing::info;

/// Frame size at 16 kHz: 512 samples = 32 ms.
const FRAME_SIZE: usize = 512;
/// Context overlap from previous frame.
const CONTEXT_SIZE: usize = 64;
/// Total input to model: context + frame.
const INPUT_SIZE: usize = CONTEXT_SIZE + FRAME_SIZE; // 576

/// Error type for VAD operations.
#[derive(Debug, thiserror::Error)]
pub enum VadError {
    #[error("Model load: {0}")]
    Load(String),
    #[error("Inference: {0}")]
    Inference(String),
    #[error("Model cache: {0}")]
    Cache(String),
}

/// Inner model state (behind Mutex for thread safety).
struct VadInner {
    stft_conv: Conv1d,
    encoder: Vec<Conv1d>,
    lstm: LSTM,
    decoder: Conv1d,
    /// LSTM hidden/cell state carried across frames.
    lstm_state: LSTMState,
    /// Context buffer: last CONTEXT_SIZE samples from previous frame.
    context: Vec<f32>,
    device: Device,
}

/// Silero VAD v5 model loaded from safetensors.
///
/// Thread-safe via `Arc<Mutex<VadInner>>`. VAD runs on CPU at ~5ms/chunk,
/// sequential access via Mutex is fine.
pub struct SileroVad {
    inner: Arc<Mutex<VadInner>>,
}

impl SileroVad {
    /// Load from HuggingFace hub (downloads on first use, cached after).
    ///
    /// Model: `idle-intelligence/silero-vad-v5-safetensors` (~1.2MB)
    pub async fn load() -> std::result::Result<Self, VadError> {
        // Download/resolve model file (blocking I/O, offload to thread pool)
        let path = tokio::task::spawn_blocking(|| {
            let api = hf_hub::api::sync::Api::new()
                .map_err(|e| VadError::Cache(format!("HuggingFace Hub API: {e}")))?;
            let repo = api.model("idle-intelligence/silero-vad-v5-safetensors".to_string());
            repo.get("silero-vad-v5.safetensors")
                .map_err(|e| VadError::Cache(format!("Model download: {e}")))
        })
        .await
        .map_err(|e| VadError::Load(format!("spawn_blocking: {e}")))??;

        info!("Loading Silero VAD v5 from {}", path.display());

        // Load model weights (blocking, offload)
        let inner = tokio::task::spawn_blocking(move || {
            Self::load_inner(&path)
        })
        .await
        .map_err(|e| VadError::Load(format!("spawn_blocking: {e}")))??;

        info!("Silero VAD v5 loaded (candle, CPU)");
        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    fn load_inner(path: &std::path::Path) -> std::result::Result<VadInner, VadError> {
        let device = Device::Cpu;
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[path], DType::F32, &device)
                .map_err(|e| VadError::Load(format!("safetensors: {e}")))?
        };

        // STFT: learned Conv1d, no bias
        // Safetensors key: stft.conv.weight [258, 1, 256]
        let stft_conv = conv1d_no_bias(
            1,
            258,
            256,
            Conv1dConfig {
                stride: 128,
                ..Default::default()
            },
            vb.pp("stft.conv"),
        )
        .map_err(|e| VadError::Load(format!("stft.conv: {e}")))?;

        // Encoder: 4 Conv1d layers with bias
        // Safetensors keys: encoder.{0,1,2,3}.conv.{weight,bias}
        let encoder = vec![
            conv1d(129, 128, 3, Conv1dConfig { padding: 1, ..Default::default() }, vb.pp("encoder.0.conv"))
                .map_err(|e| VadError::Load(format!("encoder.0: {e}")))?,
            conv1d(128, 64, 3, Conv1dConfig { padding: 1, stride: 2, ..Default::default() }, vb.pp("encoder.1.conv"))
                .map_err(|e| VadError::Load(format!("encoder.1: {e}")))?,
            conv1d(64, 64, 3, Conv1dConfig { padding: 1, stride: 2, ..Default::default() }, vb.pp("encoder.2.conv"))
                .map_err(|e| VadError::Load(format!("encoder.2: {e}")))?,
            conv1d(64, 128, 3, Conv1dConfig { padding: 1, ..Default::default() }, vb.pp("encoder.3.conv"))
                .map_err(|e| VadError::Load(format!("encoder.3: {e}")))?,
        ];

        // LSTM: input=128, hidden=128
        // Safetensors keys: decoder.lstm.{weight_ih_l0, weight_hh_l0, bias_ih_l0, bias_hh_l0}
        let lstm = lstm(128, 128, LSTMConfig::default(), vb.pp("decoder.lstm"))
            .map_err(|e| VadError::Load(format!("decoder.lstm: {e}")))?;

        // Decoder output: Conv1d 128→1, kernel=1
        // Safetensors keys: decoder.output.{weight, bias}
        let decoder = conv1d(128, 1, 1, Conv1dConfig::default(), vb.pp("decoder.output"))
            .map_err(|e| VadError::Load(format!("decoder.output: {e}")))?;

        // Initial LSTM state: zeros [batch=1, hidden=128]
        let h = Tensor::zeros((1, 128), DType::F32, &device)
            .map_err(|e| VadError::Load(format!("state init: {e}")))?;
        let c = Tensor::zeros((1, 128), DType::F32, &device)
            .map_err(|e| VadError::Load(format!("state init: {e}")))?;
        let lstm_state = LSTMState::new(h, c);

        Ok(VadInner {
            stft_conv,
            encoder,
            lstm,
            decoder,
            lstm_state,
            context: vec![0.0; CONTEXT_SIZE],
            device,
        })
    }

    /// Process a chunk of 512 PCM16 samples at 16 kHz.
    /// Returns speech probability ∈ [0, 1].
    pub fn process_chunk(&self, pcm: &[i16]) -> std::result::Result<f32, VadError> {
        let mut inner = self.inner.lock().unwrap();

        if pcm.len() < FRAME_SIZE {
            return Err(VadError::Inference(format!(
                "expected {} samples, got {}",
                FRAME_SIZE,
                pcm.len()
            )));
        }

        // Convert i16 → f32 normalized [-1, 1]
        let audio_f32: Vec<f32> = pcm[..FRAME_SIZE]
            .iter()
            .map(|&s| s as f32 / 32767.0)
            .collect();

        // Build input: context (64) + new audio (512) = 576 samples
        let mut input = Vec::with_capacity(INPUT_SIZE);
        input.extend_from_slice(&inner.context);
        input.extend_from_slice(&audio_f32);

        // Update context for next call: last 64 samples of current frame
        inner
            .context
            .copy_from_slice(&audio_f32[FRAME_SIZE - CONTEXT_SIZE..]);

        Self::forward(&mut inner, &input)
    }

    /// Run the model forward pass.
    fn forward(inner: &mut VadInner, input: &[f32]) -> std::result::Result<f32, VadError> {
        let map_err = |e: candle_core::Error| VadError::Inference(format!("{e}"));

        // Input: [1, 576] → pad right (reflection) to [1, 1, 640]
        let x = Tensor::from_slice(input, (1, INPUT_SIZE), &inner.device).map_err(map_err)?;

        // Reflection padding: pad right by 64 to get 640 samples
        let pad_start = INPUT_SIZE - CONTEXT_SIZE; // 512
        let pad_slice = x.narrow(1, pad_start - CONTEXT_SIZE, CONTEXT_SIZE).map_err(map_err)?;
        let pad_slice = pad_slice.flip(&[1]).map_err(map_err)?;
        let x = Tensor::cat(&[&x, &pad_slice], 1).map_err(map_err)?;
        let x = x.unsqueeze(1).map_err(map_err)?; // [1, 1, 640]

        // STFT conv: [1, 1, 640] → [1, 258, time]
        let x = inner.stft_conv.forward(&x).map_err(map_err)?;

        // Magnitude spectrum: 258 channels = 129 pairs → sqrt(real² + imag²)
        let x_real = x.narrow(1, 0, 129).map_err(map_err)?;
        let x_imag = x.narrow(1, 129, 129).map_err(map_err)?;
        let x = ((&x_real * &x_real).map_err(map_err)? + (&x_imag * &x_imag).map_err(map_err)?)
            .map_err(map_err)?
            .sqrt()
            .map_err(map_err)?;

        // Encoder: 4 Conv1d layers with ReLU
        let mut x = x;
        for conv in &inner.encoder {
            x = conv.forward(&x).map_err(map_err)?.relu().map_err(map_err)?;
        }

        // LSTM: process each time step
        let (_, _channels, time_steps) = x.dims3().map_err(map_err)?;
        let mut state = inner.lstm_state.clone();
        let mut last_h = state.h.clone();
        for t in 0..time_steps {
            let xt = x.narrow(2, t, 1).map_err(map_err)?.squeeze(2).map_err(map_err)?;
            state = inner.lstm.step(&xt, &state).map_err(map_err)?;
            last_h = state.h.clone();
        }
        inner.lstm_state = state;

        // ReLU after LSTM
        let x = last_h.relu().map_err(map_err)?;

        // Decoder: Conv1d 128→1, kernel=1 → sigmoid
        let x = x.unsqueeze(2).map_err(map_err)?;
        let x = inner.decoder.forward(&x).map_err(map_err)?;
        let x = candle_nn::ops::sigmoid(&x).map_err(map_err)?;

        // Extract scalar
        let prob = x.flatten_all().map_err(map_err)?.to_vec1::<f32>().map_err(map_err)?;
        Ok(prob[0])
    }

    /// Reset internal state (call when starting a new voice session).
    pub fn reset(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.context = vec![0.0; CONTEXT_SIZE];
        if let (Ok(h), Ok(c)) = (
            Tensor::zeros((1, 128), DType::F32, &inner.device),
            Tensor::zeros((1, 128), DType::F32, &inner.device),
        ) {
            inner.lstm_state = LSTMState::new(h, c);
        }
    }
}
