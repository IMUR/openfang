//! Silero VAD v5 — Voice Activity Detection via Candle.
//!
//! Loads the `idle-intelligence/silero-vad-v5-safetensors` model and runs
//! inference on CPU. Processes 512-sample chunks at 16 kHz (32 ms) and returns
//! a speech probability ∈ [0, 1].
//!
//! Architecture: STFT conv → magnitude → 4× Conv1d encoder → LSTM → Conv1d decoder → sigmoid.

use candle_core::{DType, Device, Result, Tensor};
use candle_nn::{conv1d, conv1d_no_bias, lstm, Conv1d, Conv1dConfig, Module, VarBuilder, LSTM};
use candle_nn::rnn::{LSTMConfig, LSTMState, RNN};

/// Frame size at 16 kHz: 512 samples = 32 ms.
const FRAME_SIZE: usize = 512;
/// Context overlap from previous frame.
const CONTEXT_SIZE: usize = 64;
/// Total input to model: context + frame.
const INPUT_SIZE: usize = CONTEXT_SIZE + FRAME_SIZE; // 576

/// Silero VAD v5 model loaded from safetensors.
pub struct SileroVad {
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

impl SileroVad {
    /// Load the model from a safetensors file path.
    pub fn from_file(path: &std::path::Path) -> std::result::Result<Self, String> {
        let device = Device::Cpu;
        Self::load(path, &device).map_err(|e| format!("Failed to load Silero VAD: {e}"))
    }

    /// Load from HuggingFace hub (downloads on first use).
    pub fn from_hub() -> std::result::Result<Self, String> {
        let api = hf_hub::api::sync::Api::new()
            .map_err(|e| format!("HuggingFace Hub API init failed: {e}"))?;
        let repo = api.model("idle-intelligence/silero-vad-v5-safetensors".to_string());
        let path = repo
            .get("silero-vad-v5.safetensors")
            .map_err(|e| format!("Failed to download Silero VAD model: {e}"))?;
        Self::from_file(&path)
    }

    fn load(path: &std::path::Path, device: &Device) -> Result<Self> {
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[path], DType::F32, device)?
        };

        // STFT: learned Conv1d, no bias
        // weight shape: [258, 1, 256]
        let stft_conv = conv1d_no_bias(
            1,
            258,
            256,
            Conv1dConfig {
                stride: 128,
                ..Default::default()
            },
            vb.pp("stft.conv"),
        )?;

        // Encoder: 4 Conv1d layers with bias
        let encoder = vec![
            conv1d(129, 128, 3, Conv1dConfig { padding: 1, ..Default::default() }, vb.pp("encoder.0"))?,
            conv1d(128, 64, 3, Conv1dConfig { padding: 1, stride: 2, ..Default::default() }, vb.pp("encoder.1"))?,
            conv1d(64, 64, 3, Conv1dConfig { padding: 1, stride: 2, ..Default::default() }, vb.pp("encoder.2"))?,
            conv1d(64, 128, 3, Conv1dConfig { padding: 1, ..Default::default() }, vb.pp("encoder.3"))?,
        ];

        // LSTM: input=128, hidden=128
        let lstm = lstm(128, 128, LSTMConfig::default(), vb.pp("decoder.rnn"))?;

        // Decoder output: Conv1d 128→1, kernel=1
        let decoder = conv1d(
            128,
            1,
            1,
            Conv1dConfig::default(),
            vb.pp("decoder.decoder"),
        )?;

        // Initial LSTM state: zeros [batch=1, hidden=128]
        let h = Tensor::zeros((1, 128), DType::F32, device)?;
        let c = Tensor::zeros((1, 128), DType::F32, device)?;
        let lstm_state = LSTMState::new(h, c);

        Ok(Self {
            stft_conv,
            encoder,
            lstm,
            decoder,
            lstm_state,
            context: vec![0.0; CONTEXT_SIZE],
            device: device.clone(),
        })
    }

    /// Process a chunk of 512 PCM16 samples at 16 kHz.
    /// Returns speech probability ∈ [0, 1].
    pub fn process_chunk(&mut self, pcm: &[i16]) -> std::result::Result<f32, String> {
        if pcm.len() < FRAME_SIZE {
            return Err(format!(
                "SileroVad: expected {} samples, got {}",
                FRAME_SIZE,
                pcm.len()
            ));
        }

        // Convert i16 → f32 normalized [-1, 1]
        let audio_f32: Vec<f32> = pcm[..FRAME_SIZE]
            .iter()
            .map(|&s| s as f32 / 32767.0)
            .collect();

        // Build input: context (64) + new audio (512) = 576 samples
        let mut input = Vec::with_capacity(INPUT_SIZE);
        input.extend_from_slice(&self.context);
        input.extend_from_slice(&audio_f32);

        // Update context for next call: last 64 samples of current frame
        self.context
            .copy_from_slice(&audio_f32[FRAME_SIZE - CONTEXT_SIZE..]);

        self.forward(&input)
            .map_err(|e| format!("SileroVad forward pass failed: {e}"))
    }

    /// Run the model forward pass.
    fn forward(&mut self, input: &[f32]) -> Result<f32> {
        // Input: [1, 576] → pad right (reflection) to [1, 1, 640]
        let x = Tensor::from_slice(input, (1, INPUT_SIZE), &self.device)?;

        // Reflection padding: pad right by 64 to get 640 samples
        // Reflect last 64 samples: input[511], input[510], ..., input[448]
        let pad_start = INPUT_SIZE - CONTEXT_SIZE; // 512
        let pad_slice = x.narrow(1, pad_start - CONTEXT_SIZE, CONTEXT_SIZE)?; // [1, 64]
        // Reverse for reflection
        let pad_slice = pad_slice.flip(&[1])?;
        let x = Tensor::cat(&[&x, &pad_slice], 1)?; // [1, 640]
        let x = x.unsqueeze(1)?; // [1, 1, 640]

        // STFT conv: [1, 1, 640] → [1, 258, time]
        let x = self.stft_conv.forward(&x)?;

        // Magnitude spectrum: pairs of (real, imag) → sqrt(real² + imag²)
        // 258 channels = 129 complex pairs
        let (_batch, _channels, _time) = x.dims3()?;
        let x_real = x.narrow(1, 0, 129)?; // even indices approximation
        let x_imag = x.narrow(1, 129, 129)?; // odd indices approximation
        let x = ((&x_real * &x_real)? + (&x_imag * &x_imag)?)?.sqrt()?;
        // x: [1, 129, time]

        // Encoder: 4 Conv1d layers with ReLU
        let mut x = x;
        for conv in &self.encoder {
            x = conv.forward(&x)?.relu()?;
        }
        // After encoder: [1, 128, 1] (time dimension reduced by stride=2 twice)

        // LSTM: expects [batch, features] per step
        // Squeeze time dimension and run single step
        let (_, _channels, time_steps) = x.dims3()?;
        // Process each time step through LSTM
        let mut state = self.lstm_state.clone();
        let mut last_h = state.h.clone();
        for t in 0..time_steps {
            let xt = x.narrow(2, t, 1)?.squeeze(2)?; // [1, 128]
            state = self.lstm.step(&xt, &state)?;
            last_h = state.h.clone();
        }
        self.lstm_state = state;

        // ReLU after LSTM
        let x = last_h.relu()?;

        // Decoder: Conv1d 128→1, kernel=1
        let x = x.unsqueeze(2)?; // [1, 128, 1]
        let x = self.decoder.forward(&x)?; // [1, 1, 1]

        // Sigmoid
        let x = candle_nn::ops::sigmoid(&x)?;

        // Extract scalar
        let prob = x.flatten_all()?.to_vec1::<f32>()?;
        Ok(prob[0])
    }

    /// Reset internal state (call when starting a new session).
    pub fn reset(&mut self) {
        self.context = vec![0.0; CONTEXT_SIZE];
        let h = Tensor::zeros((1, 128), DType::F32, &self.device).unwrap();
        let c = Tensor::zeros((1, 128), DType::F32, &self.device).unwrap();
        self.lstm_state = LSTMState::new(h, c);
    }
}
