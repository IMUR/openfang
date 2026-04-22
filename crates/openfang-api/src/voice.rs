//! Voice transport layer for real-time conversational voice chat.
//!
//! Voice is a presentation layer — the kernel and agent loop are unchanged.
//! Audio arrives as binary WebSocket frames (Opus), gets transcribed via
//! local STT, fed to the agent as text, and the agent's response is
//! synthesized via local TTS and sent back as Opus frames.
//!
//! Binary frame protocol:
//!
//! | Byte 0 | Name           | Direction       | Payload                            |
//! |--------|----------------|-----------------|------------------------------------|
//! | 0x01   | AudioData      | client→server   | Opus frame                         |
//! | 0x02   | AudioData      | server→client   | Opus frame                         |
//! | 0x10   | SpeechStart    | server→client   | empty                              |
//! | 0x11   | SpeechEnd      | server→client   | empty                              |
//! | 0x20   | SessionInit    | client→server   | JSON config                        |
//! | 0x21   | SessionAck     | server→client   | JSON `{"session_id":"..."}`        |
//! | 0x30   | VadSpeechStart | server→client   | empty                              |
//! | 0x31   | VadSpeechEnd   | server→client   | empty                              |
//! | 0x40   | Interrupt      | client→server   | empty                              |
//! | 0xF0   | Error          | server→client   | UTF-8 error string                 |

use openfang_types::config::VoiceConfig;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use tracing::{info, warn};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Protocol
// ---------------------------------------------------------------------------

/// Voice protocol message types identified by the first byte of a binary frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceProtocol {
    /// Client sends Opus audio data.
    AudioDataIn(Vec<u8>),
    /// Server sends Opus audio data.
    AudioDataOut(Vec<u8>),
    /// Server indicates TTS playback started.
    SpeechStart,
    /// Server indicates TTS playback ended.
    SpeechEnd,
    /// Client requests a voice session.
    SessionInit(SessionInitPayload),
    /// Server acknowledges the voice session.
    SessionAck { session_id: String },
    /// Server detected user started speaking (VAD).
    VadSpeechStart,
    /// Server detected user stopped speaking (VAD).
    VadSpeechEnd,
    /// Client requests interruption of current TTS.
    Interrupt,
    /// Server reports an error.
    Error(String),
}

/// Payload for SessionInit (0x20).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInitPayload {
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    #[serde(default = "default_codec")]
    pub codec: String,
    #[serde(default = "default_channels")]
    pub channels: u8,
    /// Optional base64-encoded WAV for voice cloning (overrides config).
    #[serde(default)]
    pub voice_clone_ref: Option<String>,
}

fn default_sample_rate() -> u32 {
    16000
}
fn default_codec() -> String {
    "opus".to_string()
}
fn default_channels() -> u8 {
    1
}

impl Default for SessionInitPayload {
    fn default() -> Self {
        Self {
            sample_rate: 16000,
            codec: "opus".to_string(),
            channels: 1,
            voice_clone_ref: None,
        }
    }
}

/// Parse a binary WebSocket frame into a `VoiceProtocol` message.
pub fn parse_binary_frame(data: &[u8]) -> Result<VoiceProtocol, String> {
    if data.is_empty() {
        return Err("Empty binary frame".into());
    }

    let payload = &data[1..];

    match data[0] {
        0x01 => {
            if payload.is_empty() {
                return Err("AudioDataIn: empty payload".into());
            }
            Ok(VoiceProtocol::AudioDataIn(payload.to_vec()))
        }
        0x02 => {
            if payload.is_empty() {
                return Err("AudioDataOut: empty payload".into());
            }
            Ok(VoiceProtocol::AudioDataOut(payload.to_vec()))
        }
        0x10 => Ok(VoiceProtocol::SpeechStart),
        0x11 => Ok(VoiceProtocol::SpeechEnd),
        0x20 => {
            let init: SessionInitPayload = if payload.is_empty() {
                SessionInitPayload::default()
            } else {
                serde_json::from_slice(payload)
                    .map_err(|e| format!("SessionInit: invalid JSON: {e}"))?
            };
            Ok(VoiceProtocol::SessionInit(init))
        }
        0x21 => {
            let json: serde_json::Value = serde_json::from_slice(payload)
                .map_err(|e| format!("SessionAck: invalid JSON: {e}"))?;
            let session_id = json["session_id"].as_str().unwrap_or("").to_string();
            Ok(VoiceProtocol::SessionAck { session_id })
        }
        0x30 => Ok(VoiceProtocol::VadSpeechStart),
        0x31 => Ok(VoiceProtocol::VadSpeechEnd),
        0x40 => Ok(VoiceProtocol::Interrupt),
        0xF0 => {
            let msg = std::str::from_utf8(payload)
                .unwrap_or("invalid UTF-8")
                .to_string();
            Ok(VoiceProtocol::Error(msg))
        }
        other => Err(format!("Unknown voice protocol byte: 0x{other:02X}")),
    }
}

/// Encode a `VoiceProtocol` message into binary WebSocket frame bytes.
pub fn encode_binary_frame(msg: &VoiceProtocol) -> Vec<u8> {
    match msg {
        VoiceProtocol::AudioDataIn(data) => {
            let mut frame = Vec::with_capacity(1 + data.len());
            frame.push(0x01);
            frame.extend_from_slice(data);
            frame
        }
        VoiceProtocol::AudioDataOut(data) => {
            let mut frame = Vec::with_capacity(1 + data.len());
            frame.push(0x02);
            frame.extend_from_slice(data);
            frame
        }
        VoiceProtocol::SpeechStart => vec![0x10],
        VoiceProtocol::SpeechEnd => vec![0x11],
        VoiceProtocol::SessionInit(payload) => {
            let json = serde_json::to_vec(payload).unwrap_or_default();
            let mut frame = Vec::with_capacity(1 + json.len());
            frame.push(0x20);
            frame.extend_from_slice(&json);
            frame
        }
        VoiceProtocol::SessionAck { session_id } => {
            let json = serde_json::to_vec(&serde_json::json!({
                "session_id": session_id,
            }))
            .unwrap_or_default();
            let mut frame = Vec::with_capacity(1 + json.len());
            frame.push(0x21);
            frame.extend_from_slice(&json);
            frame
        }
        VoiceProtocol::VadSpeechStart => vec![0x30],
        VoiceProtocol::VadSpeechEnd => vec![0x31],
        VoiceProtocol::Interrupt => vec![0x40],
        VoiceProtocol::Error(msg) => {
            let bytes = msg.as_bytes();
            let mut frame = Vec::with_capacity(1 + bytes.len());
            frame.push(0xF0);
            frame.extend_from_slice(bytes);
            frame
        }
    }
}

// ---------------------------------------------------------------------------
// Opus Codec
// ---------------------------------------------------------------------------

/// Opus encoder/decoder wrapper for voice chat.
///
/// Configured for voice: 16kHz mono, 20ms frames (320 samples).
/// Uses SILK mode internally which is optimized for speech at low sample rates.
pub struct OpusCodec {
    encoder: opus_rs::OpusEncoder,
    decoder: opus_rs::OpusDecoder,
}

/// 20ms frame at 16kHz = 320 samples.
pub const OPUS_FRAME_SAMPLES: usize = 320;

impl OpusCodec {
    /// Create a new Opus encoder/decoder pair for 16kHz mono voice.
    pub fn new() -> Result<Self, String> {
        let encoder = opus_rs::OpusEncoder::new(16000, 1, opus_rs::Application::Voip)
            .map_err(|e| format!("Opus encoder init failed: {e}"))?;
        let decoder = opus_rs::OpusDecoder::new(16000, 1)
            .map_err(|e| format!("Opus decoder init failed: {e}"))?;
        Ok(Self { encoder, decoder })
    }

    /// Decode an Opus packet to PCM16 samples.
    ///
    /// `opus-rs` uses `f32` samples in ±1.0; we convert to `i16` for the rest of the pipeline.
    pub fn decode(&mut self, opus_data: &[u8]) -> Result<Vec<i16>, String> {
        let mut f32_out = vec![0.0f32; OPUS_FRAME_SAMPLES * 6]; // up to 120ms @ 16kHz mono
        let samples = self
            .decoder
            .decode(opus_data, OPUS_FRAME_SAMPLES, &mut f32_out)
            .map_err(|e| format!("Opus decode failed: {e}"))?;
        f32_out.truncate(samples);
        Ok(f32_out
            .iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
            .collect())
    }

    /// Encode PCM16 samples to an Opus packet.
    /// Input should be exactly `OPUS_FRAME_SAMPLES` (320) samples for a 20ms @ 16kHz frame.
    pub fn encode(&mut self, pcm: &[i16]) -> Result<Vec<u8>, String> {
        if pcm.len() != OPUS_FRAME_SAMPLES {
            return Err(format!(
                "Opus encode expects {OPUS_FRAME_SAMPLES} samples, got {}",
                pcm.len()
            ));
        }
        let f32_in: Vec<f32> = pcm.iter().map(|&s| s as f32 / 32768.0).collect();
        let mut opus_data = vec![0u8; 4000];
        let len = self
            .encoder
            .encode(&f32_in, OPUS_FRAME_SAMPLES, &mut opus_data)
            .map_err(|e| format!("Opus encode failed: {e}"))?;
        opus_data.truncate(len);
        Ok(opus_data)
    }
}

// ---------------------------------------------------------------------------
// Voice Session
// ---------------------------------------------------------------------------

/// State of a voice session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceSessionState {
    /// Session initialized, waiting for audio.
    Idle,
    /// Receiving audio from client, VAD active.
    Listening,
    /// VAD detected end of speech, transcribing.
    Transcribing,
    /// Transcription sent to agent, waiting for response.
    Processing,
    /// Streaming TTS audio back to client.
    Speaking,
}

/// Result of processing an audio frame through the voice session.
pub enum VoiceAction {
    /// Keep accumulating audio.
    Continue,
    /// Speech onset detected (for VadSpeechStart notification).
    SpeechStarted,
    /// Silence detected — transcribe this PCM buffer.
    Transcribe(Vec<i16>),
    /// User started speaking during TTS playback — barge-in.
    BargeIn,
}

/// A voice session associated with a WebSocket connection.
pub struct VoiceSession {
    /// Unique session identifier.
    pub session_id: String,
    /// Current state.
    pub state: VoiceSessionState,
    /// Client-requested configuration.
    pub init: SessionInitPayload,
    /// Opus codec for encode/decode (None when codec="pcm16").
    pub opus: Option<OpusCodec>,
    /// PCM buffer accumulating decoded audio from client.
    pub pcm_buffer: Vec<i16>,
    /// Voice configuration from KernelConfig.
    pub config: VoiceConfig,
    /// Consecutive silent frames count (energy-based VAD).
    silence_frames: u32,
    /// Whether we've seen speech in the current utterance.
    pub speech_detected: bool,
    /// Sentence buffer for TTS output.
    pub sentence_buffer: SentenceBuffer,
    /// Neural VAD (Silero v5 via Candle). None = fallback to energy-based.
    /// Injected at creation from the kernel's boot-time loaded driver.
    #[cfg(feature = "memory-candle")]
    silero_vad: Option<std::sync::Arc<openfang_runtime::candle_vad::SileroVad>>,
    /// Accumulates PCM for Silero VAD (needs 512-sample chunks).
    vad_accumulator: Vec<i16>,
    /// Last Silero VAD probability (for smooth state transitions).
    last_vad_prob: f32,
}

impl VoiceSession {
    /// Create a new voice session from a SessionInit payload and config.
    ///
    /// `vad_driver`: pre-loaded Silero VAD from the kernel (None = energy-based fallback).
    /// When built without `memory-candle`, this parameter is not present.
    pub fn new(
        init: SessionInitPayload,
        config: VoiceConfig,
        #[cfg(feature = "memory-candle")] vad_driver: Option<
            std::sync::Arc<openfang_runtime::candle_vad::SileroVad>,
        >,
    ) -> Result<Self, String> {
        let opus = if init.codec == "pcm16" {
            None
        } else {
            Some(OpusCodec::new()?)
        };

        let min_chunk_chars = config.tts_min_chunk_chars;

        #[cfg(feature = "memory-candle")]
        if vad_driver.is_some() {
            info!("Voice session using Silero VAD v5 (candle, CPU)");
        } else {
            warn!("Voice session using energy-based VAD (Silero unavailable)");
        }

        Ok(Self {
            session_id: Uuid::new_v4().to_string(),
            state: VoiceSessionState::Idle,
            init,
            opus,
            pcm_buffer: Vec::with_capacity(16000 * 30),
            config,
            silence_frames: 0,
            speech_detected: false,
            sentence_buffer: SentenceBuffer::with_min_chars(min_chunk_chars),
            #[cfg(feature = "memory-candle")]
            silero_vad: vad_driver,
            vad_accumulator: Vec::with_capacity(512),
            last_vad_prob: 0.0,
        })
    }

    /// Decode incoming audio bytes to PCM16 samples.
    /// For Opus: decode the packet. For PCM16: interpret bytes as little-endian i16.
    pub fn decode_audio(&mut self, data: &[u8]) -> Result<Vec<i16>, String> {
        match self.opus.as_mut() {
            Some(codec) => codec.decode(data),
            None => {
                // Raw PCM16 little-endian
                if !data.len().is_multiple_of(2) {
                    return Err("PCM16 data must be even length".into());
                }
                Ok(data
                    .chunks_exact(2)
                    .map(|c| i16::from_le_bytes([c[0], c[1]]))
                    .collect())
            }
        }
    }

    /// Encode PCM16 samples to output bytes.
    /// For Opus: encode to Opus packet. For PCM16: convert to little-endian bytes.
    pub fn encode_audio(&mut self, pcm: &[i16]) -> Result<Vec<u8>, String> {
        match self.opus.as_mut() {
            Some(codec) => codec.encode(pcm),
            None => {
                // Raw PCM16 little-endian
                let mut bytes = Vec::with_capacity(pcm.len() * 2);
                for &sample in pcm {
                    bytes.extend_from_slice(&sample.to_le_bytes());
                }
                Ok(bytes)
            }
        }
    }

    /// Process incoming decoded PCM audio. Returns a `VoiceAction` indicating
    /// whether to keep listening or trigger transcription.
    pub fn handle_audio(&mut self, pcm: &[i16]) -> VoiceAction {
        let is_speech = self.detect_speech(pcm);

        // If we detect speech while the bot is Speaking, trigger a barge-in
        // and transition to Listening state immediately.
        if self.state == VoiceSessionState::Speaking {
            if is_speech {
                self.speech_detected = true;
                self.state = VoiceSessionState::Listening;
                self.silence_frames = 0;
                self.pcm_buffer.extend_from_slice(pcm);
                return VoiceAction::BargeIn;
            }
            return VoiceAction::Continue;
        }

        let silence_threshold_frames = (self.config.vad_silence_ms as f32 / 20.0).ceil() as u32;
        let max_samples = self.config.max_utterance_secs as usize * 16000;

        if is_speech {
            // Speech detected
            let was_silent = !self.speech_detected;
            if was_silent {
                self.speech_detected = true;
                self.state = VoiceSessionState::Listening;
            }
            self.silence_frames = 0;
            self.pcm_buffer.extend_from_slice(pcm);

            if was_silent {
                return VoiceAction::SpeechStarted;
            }
        } else if self.speech_detected {
            // Silence after speech
            self.silence_frames += 1;
            self.pcm_buffer.extend_from_slice(pcm);

            if self.silence_frames >= silence_threshold_frames {
                self.state = VoiceSessionState::Transcribing;
                self.speech_detected = false;
                self.silence_frames = 0;
                let buffer =
                    std::mem::replace(&mut self.pcm_buffer, Vec::with_capacity(16000 * 30));
                return VoiceAction::Transcribe(buffer);
            }
        }

        // Force transcription if buffer exceeds max duration
        if self.pcm_buffer.len() >= max_samples && self.speech_detected {
            self.state = VoiceSessionState::Transcribing;
            self.speech_detected = false;
            self.silence_frames = 0;
            let buffer = std::mem::replace(&mut self.pcm_buffer, Vec::with_capacity(16000 * 30));
            return VoiceAction::Transcribe(buffer);
        }

        VoiceAction::Continue
    }

    /// Detect speech using Silero VAD (neural) or energy-based (fallback).
    fn detect_speech(&mut self, pcm: &[i16]) -> bool {
        #[cfg(feature = "memory-candle")]
        if let Some(ref vad) = self.silero_vad {
            // Accumulate samples for Silero (needs 512-sample chunks)
            self.vad_accumulator.extend_from_slice(pcm);
            while self.vad_accumulator.len() >= 512 {
                let chunk: Vec<i16> = self.vad_accumulator.drain(..512).collect();
                match vad.process_chunk(&chunk) {
                    Ok(prob) => self.last_vad_prob = prob,
                    Err(e) => {
                        warn!("Silero VAD error: {e}");
                        return self.detect_speech_energy(pcm);
                    }
                }
            }
            let threshold = self.config.vad_speech_threshold;
            return self.last_vad_prob > threshold;
        }

        self.detect_speech_energy(pcm)
    }

    /// Energy-based speech detection (fallback when Silero unavailable).
    fn detect_speech_energy(&self, pcm: &[i16]) -> bool {
        if pcm.is_empty() {
            return false;
        }
        let sum_sq: f64 = pcm.iter().map(|&s| (s as f64) * (s as f64)).sum();
        let rms = (sum_sq / pcm.len() as f64).sqrt() / 32768.0;
        rms > self.config.vad_energy_threshold as f64
    }

    /// Reset the session to idle state (e.g. after barge-in).
    pub fn reset_to_idle(&mut self) {
        self.state = VoiceSessionState::Idle;
        self.speech_detected = false;
        self.silence_frames = 0;
        self.pcm_buffer.clear();
        self.sentence_buffer = SentenceBuffer::new();
        self.vad_accumulator.clear();
        self.last_vad_prob = 0.0;
        #[cfg(feature = "memory-candle")]
        if let Some(ref vad) = self.silero_vad {
            vad.reset();
        }
    }
}

/// Encode PCM16 samples into binary WS frame payloads (AudioDataOut).
///
/// For Opus: chunks into 20ms frames, encodes each, wraps in 0x02 header.
/// For PCM16: converts to little-endian bytes in a single frame.
pub fn encode_pcm_to_frames(pcm: &[i16], use_opus: bool) -> Vec<Vec<u8>> {
    if !use_opus {
        // Single frame with all PCM16 data
        let mut bytes = Vec::with_capacity(pcm.len() * 2);
        for &s in pcm {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        return vec![encode_binary_frame(&VoiceProtocol::AudioDataOut(bytes))];
    }

    // Opus: chunk into 20ms frames
    let mut frames = Vec::new();
    let mut enc = match OpusCodec::new() {
        Ok(e) => e,
        Err(_) => return frames,
    };
    for chunk in pcm.chunks(OPUS_FRAME_SAMPLES) {
        let mut frame_pcm = chunk.to_vec();
        frame_pcm.resize(OPUS_FRAME_SAMPLES, 0);
        if let Ok(opus) = enc.encode(&frame_pcm) {
            frames.push(encode_binary_frame(&VoiceProtocol::AudioDataOut(opus)));
        }
    }
    frames
}

// ---------------------------------------------------------------------------
// STT Client
// ---------------------------------------------------------------------------

/// HTTP client for the Parakeet TDT STT service (drtr:7733, `/v1/audio/transcriptions`).
pub struct SttClient {
    endpoint: String,
    model: String,
    client: reqwest::Client,
}

impl SttClient {
    pub fn new(endpoint: &str, model: &str) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            model: model.to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// Transcribe PCM16 audio to text via the Parakeet TDT STT service.
    pub async fn transcribe(&self, pcm: &[i16], sample_rate: u32) -> Result<String, String> {
        let wav = pcm_to_wav(pcm, sample_rate);
        let part = reqwest::multipart::Part::bytes(wav)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| format!("MIME error: {e}"))?;

        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", self.model.clone())
            .text("language", "en")
            .text("response_format", "json");

        let url = format!("{}/v1/audio/transcriptions", self.endpoint);
        let resp = self
            .client
            .post(&url)
            .multipart(form)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("STT request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("STT failed (HTTP {status}): {body}"));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("STT response parse failed: {e}"))?;

        Ok(json["text"].as_str().unwrap_or("").trim().to_string())
    }
}

// ---------------------------------------------------------------------------
// TTS Client
// ---------------------------------------------------------------------------

/// HTTP client for the Chatterbox-Turbo TTS service (drtr:7744, `/v1/audio/speech`).
pub struct TtsClient {
    endpoint: String,
    voice: String,
    language: String,
    speed: f32,
    /// Optional base64-encoded WAV for voice cloning (Chatterbox).
    voice_clone_ref: Option<String>,
    client: reqwest::Client,
}

impl TtsClient {
    pub fn new(endpoint: &str, voice: &str, speed: f32) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            voice: voice.to_string(),
            language: "English".to_string(),
            speed,
            voice_clone_ref: None,
            client: reqwest::Client::new(),
        }
    }

    /// Set a voice cloning reference audio (base64-encoded WAV).
    pub fn with_voice_clone_ref(mut self, ref_path: Option<&str>) -> Self {
        if let Some(path) = ref_path {
            if let Ok(bytes) = std::fs::read(path) {
                use base64::Engine;
                self.voice_clone_ref =
                    Some(base64::engine::general_purpose::STANDARD.encode(&bytes));
            } else {
                warn!("Voice clone ref not found: {path}");
            }
        }
        self
    }

    /// Synthesize text to PCM16 samples at 16kHz mono.
    ///
    /// TTS service returns 24kHz WAV; we parse the WAV header, extract PCM16, and resample to 16kHz.
    pub async fn synthesize(&self, text: &str) -> Result<Vec<i16>, String> {
        let url = format!("{}/v1/audio/speech", self.endpoint);
        let mut body = serde_json::json!({
            "input": text,
            "voice": self.voice,
            "speed": self.speed,
            "language": self.language,
        });
        if let Some(ref clone_ref) = self.voice_clone_ref {
            body["reference_audio"] = serde_json::Value::String(clone_ref.clone());
        }

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("TTS request failed: {e}"))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("TTS failed (HTTP {status}): {body}"));
        }

        let wav_bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("TTS response read failed: {e}"))?;

        let (pcm_24k, sample_rate) = parse_wav(&wav_bytes)?;

        if sample_rate == 16000 {
            Ok(pcm_24k)
        } else {
            Ok(resample(&pcm_24k, sample_rate, 16000))
        }
    }
}

// ---------------------------------------------------------------------------
// WAV Utilities
// ---------------------------------------------------------------------------

/// Encode PCM16 mono samples to a WAV byte buffer.
pub fn pcm_to_wav(pcm: &[i16], sample_rate: u32) -> Vec<u8> {
    let data_len = (pcm.len() * 2) as u32;
    let file_len = 36 + data_len;
    let mut buf = Vec::with_capacity(44 + pcm.len() * 2);

    // RIFF header
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&file_len.to_le_bytes());
    buf.extend_from_slice(b"WAVE");

    // fmt chunk
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    buf.extend_from_slice(&1u16.to_le_bytes()); // mono
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    buf.extend_from_slice(&2u16.to_le_bytes()); // block align
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

    // data chunk
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_len.to_le_bytes());
    for &sample in pcm {
        buf.extend_from_slice(&sample.to_le_bytes());
    }

    buf
}

/// Parse a WAV byte buffer, returning PCM16 samples and sample rate.
/// Handles standard RIFF/WAVE with PCM16 format.
pub fn parse_wav(data: &[u8]) -> Result<(Vec<i16>, u32), String> {
    if data.len() < 44 {
        return Err("WAV too short".into());
    }
    if &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err("Not a WAV file".into());
    }

    let sample_rate = u32::from_le_bytes(data[24..28].try_into().map_err(|_| "Bad sample rate")?);
    let bits_per_sample =
        u16::from_le_bytes(data[34..36].try_into().map_err(|_| "Bad bits/sample")?);

    if bits_per_sample != 16 {
        return Err(format!("Expected 16-bit PCM, got {bits_per_sample}-bit"));
    }

    // Find the "data" chunk
    let mut offset = 12;
    while offset + 8 < data.len() {
        let chunk_id = &data[offset..offset + 4];
        let chunk_size = u32::from_le_bytes(
            data[offset + 4..offset + 8]
                .try_into()
                .map_err(|_| "Bad chunk size")?,
        ) as usize;

        if chunk_id == b"data" {
            let pcm_start = offset + 8;
            let pcm_end = (pcm_start + chunk_size).min(data.len());
            let pcm_bytes = &data[pcm_start..pcm_end];
            let samples: Vec<i16> = pcm_bytes
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect();
            return Ok((samples, sample_rate));
        }
        offset += 8 + chunk_size;
    }

    Err("No data chunk found in WAV".into())
}

/// Resample PCM16 from one sample rate to another using windowed sinc interpolation.
///
/// Uses `rubato::SincFixedIn` for high-quality resampling (replaces linear interpolation).
pub fn resample(pcm: &[i16], from_rate: u32, to_rate: u32) -> Vec<i16> {
    if from_rate == to_rate || pcm.is_empty() {
        return pcm.to_vec();
    }

    use rubato::{
        Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
    };

    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };

    let ratio = to_rate as f64 / from_rate as f64;
    let mut resampler = match SincFixedIn::<f64>::new(
        ratio,
        2.0, // max relative ratio
        params,
        pcm.len(), // chunk size = full input
        1,         // mono
    ) {
        Ok(r) => r,
        Err(_) => {
            // Fallback to simple linear if rubato fails
            return resample_linear(pcm, from_rate, to_rate);
        }
    };

    // Convert i16 → f64
    let input: Vec<f64> = pcm.iter().map(|&s| s as f64 / 32768.0).collect();
    let waves_in = vec![input];

    match resampler.process(&waves_in, None) {
        Ok(waves_out) => waves_out[0]
            .iter()
            .map(|&s| (s * 32768.0).clamp(-32768.0, 32767.0) as i16)
            .collect(),
        Err(_) => resample_linear(pcm, from_rate, to_rate),
    }
}

/// Fallback linear interpolation resampler.
fn resample_linear(pcm: &[i16], from_rate: u32, to_rate: u32) -> Vec<i16> {
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = (pcm.len() as f64 / ratio).ceil() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = i as f64 * ratio;
        let idx = src_pos as usize;
        let frac = src_pos - idx as f64;
        let s0 = pcm[idx] as f64;
        let s1 = if idx + 1 < pcm.len() {
            pcm[idx + 1] as f64
        } else {
            s0
        };
        out.push((s0 + frac * (s1 - s0)) as i16);
    }
    out
}

// ---------------------------------------------------------------------------
// Markdown-to-Speakable Text Cleanup
// ---------------------------------------------------------------------------

/// Strip markdown formatting from text to make it suitable for TTS.
///
/// Removes code blocks, inline code, link URLs, heading markers, bold/italic
/// markers, and HTML tags. Keeps the readable text content.
pub fn markdown_to_speakable(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(&ch) = chars.peek() {
        match ch {
            // Fenced code blocks: skip entirely
            '`' => {
                // Check for ``` (fenced block)
                let backtick_count = count_char(&mut chars, '`');
                if backtick_count >= 3 {
                    // Skip until closing ```
                    let mut closing = 0;
                    for c in chars.by_ref() {
                        if c == '`' {
                            closing += 1;
                            if closing >= 3 {
                                break;
                            }
                        } else {
                            closing = 0;
                        }
                    }
                    result.push_str(" code omitted ");
                } else {
                    // Inline code: skip backticks, keep text
                    let mut code_text = String::new();
                    for c in chars.by_ref() {
                        if c == '`' {
                            break;
                        }
                        code_text.push(c);
                    }
                    // Only speak short inline code (filenames etc.)
                    if code_text.len() <= 30 {
                        result.push_str(&code_text);
                    } else {
                        result.push_str(" code ");
                    }
                }
            }
            // Links: [text](url) → keep text
            '[' => {
                chars.next();
                let mut link_text = String::new();
                let mut depth = 1;
                for c in chars.by_ref() {
                    if c == '[' {
                        depth += 1;
                    } else if c == ']' {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    link_text.push(c);
                }
                // Skip (url) part
                if chars.peek() == Some(&'(') {
                    chars.next();
                    let mut paren_depth = 1;
                    for c in chars.by_ref() {
                        if c == '(' {
                            paren_depth += 1;
                        } else if c == ')' {
                            paren_depth -= 1;
                            if paren_depth == 0 {
                                break;
                            }
                        }
                    }
                }
                result.push_str(&link_text);
            }
            // Bold/italic markers
            '*' | '_' => {
                chars.next();
                // Skip consecutive markers
                while chars.peek() == Some(&ch) {
                    chars.next();
                }
            }
            // Heading markers at start of line
            '#' => {
                chars.next();
                while chars.peek() == Some(&'#') {
                    chars.next();
                }
                // Skip the space after #
                if chars.peek() == Some(&' ') {
                    chars.next();
                }
            }
            // HTML tags: skip <...>
            '<' => {
                chars.next();
                for c in chars.by_ref() {
                    if c == '>' {
                        break;
                    }
                }
            }
            // Strikethrough ~~text~~
            '~' => {
                chars.next();
                if chars.peek() == Some(&'~') {
                    chars.next();
                } else {
                    result.push('~');
                }
            }
            // Everything else: keep
            _ => {
                result.push(ch);
                chars.next();
            }
        }
    }

    // Collapse multiple spaces/newlines
    let mut collapsed = String::with_capacity(result.len());
    let mut last_was_space = false;
    for ch in result.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                collapsed.push(' ');
                last_was_space = true;
            }
        } else {
            collapsed.push(ch);
            last_was_space = false;
        }
    }

    // Only trim trailing whitespace — leading spaces in streaming deltas
    // are significant word separators that the SentenceBuffer needs.
    collapsed.trim_end().to_string()
}

fn count_char(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, target: char) -> usize {
    let mut count = 0;
    while chars.peek() == Some(&target) {
        chars.next();
        count += 1;
    }
    count
}

// ---------------------------------------------------------------------------
// Clause Buffer
// ---------------------------------------------------------------------------

/// Minimum characters before yielding a clause to TTS.
const MIN_CLAUSE_CHARS: usize = 30;

/// Clause-level text accumulator for TTS.
///
/// Splits on clause boundaries (`,;:.!?\n—`) with a minimum character threshold
/// to avoid micro-chunks. Produces more natural cadence than sentence-only splitting
/// by feeding TTS with smaller, more frequent chunks.
///
/// Backward-compatible: provides the same `push()`/`next_sentence()`/`flush()` API
/// as the old `SentenceBuffer` so ws.rs voice_task works without changes.
pub struct SentenceBuffer {
    buffer: String,
    pending: VecDeque<String>,
    min_chars: usize,
}

impl SentenceBuffer {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            pending: VecDeque::new(),
            min_chars: MIN_CLAUSE_CHARS,
        }
    }

    /// Create with custom minimum clause size.
    pub fn with_min_chars(min_chars: usize) -> Self {
        Self {
            buffer: String::new(),
            pending: VecDeque::new(),
            min_chars,
        }
    }

    /// Push a text delta. Internally splits on clause boundaries.
    pub fn push(&mut self, text: &str) {
        self.buffer.push_str(text);
        self.extract_clauses();
    }

    /// Pop the next complete clause, if available.
    pub fn next_sentence(&mut self) -> Option<String> {
        self.pending.pop_front()
    }

    /// Flush any remaining text as a final clause.
    pub fn flush(&mut self) -> Option<String> {
        let remaining = self.buffer.trim().to_string();
        self.buffer.clear();
        if remaining.is_empty() {
            None
        } else {
            Some(remaining)
        }
    }

    fn extract_clauses(&mut self) {
        loop {
            let split_pos = self.find_clause_end();
            match split_pos {
                Some(pos) => {
                    let clause: String = self.buffer[..pos].trim().to_string();
                    self.buffer = self.buffer[pos..].to_string();
                    // Trim leading whitespace but preserve for next iteration
                    if self.buffer.starts_with(char::is_whitespace) {
                        self.buffer = self.buffer.trim_start().to_string();
                    }
                    if !clause.is_empty() {
                        self.pending.push_back(clause);
                    }
                }
                None => break,
            }
        }
    }

    fn find_clause_end(&self) -> Option<usize> {
        let bytes = self.buffer.as_bytes();

        // Scan for split points. Sentence-ending punctuation (.!?) always splits.
        // Clause breaks (,;:\n—) only split after min_chars threshold.
        for i in 0..bytes.len() {
            let is_sentence_end = matches!(bytes[i], b'.' | b'!' | b'?');
            let is_clause_break = matches!(bytes[i], b',' | b';' | b':' | b'\n');
            let is_dash = i + 2 < bytes.len()
                && bytes[i] == b'\xe2'
                && bytes[i + 1] == b'\x80'
                && bytes[i + 2] == b'\x94'; // em dash '—' in UTF-8

            if is_sentence_end {
                let next = i + 1;
                if next >= bytes.len() || bytes[next].is_ascii_whitespace() {
                    return Some(next);
                }
            }

            // Clause breaks only split after min_chars threshold
            if i >= self.min_chars {
                if is_clause_break {
                    let next = i + 1;
                    if next >= bytes.len() || bytes[next].is_ascii_whitespace() {
                        return Some(next);
                    }
                }
                if is_dash {
                    return Some(i + 3); // skip 3-byte em dash
                }
            }
        }
        None
    }
}

impl Default for SentenceBuffer {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_session_init_with_payload() {
        let json = br#"{"sample_rate":48000,"codec":"opus","channels":2}"#;
        let mut frame = vec![0x20];
        frame.extend_from_slice(json);

        let msg = parse_binary_frame(&frame).unwrap();
        match msg {
            VoiceProtocol::SessionInit(payload) => {
                assert_eq!(payload.sample_rate, 48000);
                assert_eq!(payload.codec, "opus");
                assert_eq!(payload.channels, 2);
            }
            other => panic!("Expected SessionInit, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_session_init_empty_payload() {
        let frame = vec![0x20];
        let msg = parse_binary_frame(&frame).unwrap();
        match msg {
            VoiceProtocol::SessionInit(payload) => {
                assert_eq!(payload.sample_rate, 16000);
                assert_eq!(payload.codec, "opus");
                assert_eq!(payload.channels, 1);
            }
            other => panic!("Expected SessionInit with defaults, got {other:?}"),
        }
    }

    #[test]
    fn test_roundtrip_audio_data_in() {
        let original = VoiceProtocol::AudioDataIn(vec![0xDE, 0xAD, 0xBE, 0xEF]);
        let encoded = encode_binary_frame(&original);
        let decoded = parse_binary_frame(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_roundtrip_audio_data_out() {
        let original = VoiceProtocol::AudioDataOut(vec![0xCA, 0xFE]);
        let encoded = encode_binary_frame(&original);
        let decoded = parse_binary_frame(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_roundtrip_session_init() {
        let original = VoiceProtocol::SessionInit(SessionInitPayload {
            sample_rate: 16000,
            codec: "opus".to_string(),
            channels: 1,
            voice_clone_ref: None,
        });
        let encoded = encode_binary_frame(&original);
        let decoded = parse_binary_frame(&encoded).unwrap();
        match decoded {
            VoiceProtocol::SessionInit(p) => {
                assert_eq!(p.sample_rate, 16000);
                assert_eq!(p.codec, "opus");
                assert_eq!(p.channels, 1);
            }
            other => panic!("Expected SessionInit, got {other:?}"),
        }
    }

    #[test]
    fn test_roundtrip_session_ack() {
        let original = VoiceProtocol::SessionAck {
            session_id: "test-123".to_string(),
        };
        let encoded = encode_binary_frame(&original);
        let decoded = parse_binary_frame(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_roundtrip_signal_messages() {
        for msg in [
            VoiceProtocol::SpeechStart,
            VoiceProtocol::SpeechEnd,
            VoiceProtocol::VadSpeechStart,
            VoiceProtocol::VadSpeechEnd,
            VoiceProtocol::Interrupt,
        ] {
            let encoded = encode_binary_frame(&msg);
            let decoded = parse_binary_frame(&encoded).unwrap();
            assert_eq!(msg, decoded);
        }
    }

    #[test]
    fn test_roundtrip_error() {
        let original = VoiceProtocol::Error("something went wrong".to_string());
        let encoded = encode_binary_frame(&original);
        let decoded = parse_binary_frame(&encoded).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_parse_empty_frame() {
        assert!(parse_binary_frame(&[]).is_err());
    }

    #[test]
    fn test_parse_unknown_byte() {
        assert!(parse_binary_frame(&[0xFF]).is_err());
    }

    #[test]
    fn test_parse_audio_data_empty_payload() {
        assert!(parse_binary_frame(&[0x01]).is_err());
        assert!(parse_binary_frame(&[0x02]).is_err());
    }

    #[test]
    fn test_voice_session_new() {
        let session = VoiceSession::new(
            SessionInitPayload::default(),
            VoiceConfig::default(),
            #[cfg(feature = "memory-candle")]
            None,
        )
        .unwrap();
        assert_eq!(session.state, VoiceSessionState::Idle);
        assert!(!session.session_id.is_empty());
        assert_eq!(session.init.sample_rate, 16000);
        assert!(session.pcm_buffer.is_empty());
    }

    // --- WAV utilities ---

    #[test]
    fn test_pcm_to_wav_roundtrip() {
        let pcm: Vec<i16> = (0..1600).map(|i| (i * 3) as i16).collect();
        let wav = pcm_to_wav(&pcm, 16000);
        let (decoded, rate) = parse_wav(&wav).unwrap();
        assert_eq!(rate, 16000);
        assert_eq!(decoded, pcm);
    }

    #[test]
    fn test_parse_wav_too_short() {
        assert!(parse_wav(&[0; 10]).is_err());
    }

    #[test]
    fn test_parse_wav_not_wav() {
        assert!(parse_wav(&[0; 44]).is_err());
    }

    // --- Resampler ---

    #[test]
    fn test_resample_identity() {
        let pcm: Vec<i16> = (0..100).map(|i| i * 100).collect();
        let out = resample(&pcm, 16000, 16000);
        assert_eq!(out, pcm);
    }

    #[test]
    fn test_resample_downsample() {
        // 24kHz → 16kHz should produce approximately 2/3 the samples
        let pcm: Vec<i16> = vec![0; 2400];
        let out = resample(&pcm, 24000, 16000);
        // Rubato sinc resampler may produce slightly different lengths than exact 2/3
        let expected = 1600;
        let tolerance = 100; // ~6% tolerance for sinc edge effects
        assert!(
            (out.len() as i64 - expected as i64).unsigned_abs() < tolerance,
            "Expected ~{expected} samples, got {}",
            out.len()
        );
    }

    // --- Markdown to speakable ---

    #[test]
    fn test_markdown_plain_text() {
        assert_eq!(markdown_to_speakable("Hello world"), "Hello world");
    }

    #[test]
    fn test_markdown_bold_italic() {
        assert_eq!(
            markdown_to_speakable("This is **bold** and *italic*"),
            "This is bold and italic"
        );
    }

    #[test]
    fn test_markdown_code_block() {
        let input = "Here is code:\n```rust\nfn main() {}\n```\nDone.";
        let output = markdown_to_speakable(input);
        assert!(output.contains("code omitted"));
        assert!(output.contains("Done."));
        assert!(!output.contains("fn main"));
    }

    #[test]
    fn test_markdown_inline_code_short() {
        assert_eq!(markdown_to_speakable("Run `ls -la` now"), "Run ls -la now");
    }

    #[test]
    fn test_markdown_link() {
        assert_eq!(
            markdown_to_speakable("See [the docs](https://example.com) for details"),
            "See the docs for details"
        );
    }

    #[test]
    fn test_markdown_heading() {
        assert_eq!(markdown_to_speakable("## Section Title"), "Section Title");
    }

    // --- Sentence buffer ---

    #[test]
    fn test_sentence_buffer_single() {
        let mut buf = SentenceBuffer::new();
        buf.push("Hello world. ");
        assert_eq!(buf.next_sentence(), Some("Hello world.".to_string()));
        assert_eq!(buf.next_sentence(), None);
    }

    #[test]
    fn test_sentence_buffer_multiple() {
        let mut buf = SentenceBuffer::new();
        buf.push("First. Second! Third? ");
        assert_eq!(buf.next_sentence(), Some("First.".to_string()));
        assert_eq!(buf.next_sentence(), Some("Second!".to_string()));
        assert_eq!(buf.next_sentence(), Some("Third?".to_string()));
    }

    #[test]
    fn test_sentence_buffer_incremental() {
        let mut buf = SentenceBuffer::new();
        buf.push("This is a ");
        assert_eq!(buf.next_sentence(), None);
        buf.push("sentence. And another.");
        assert_eq!(buf.next_sentence(), Some("This is a sentence.".to_string()));
        // "And another." ends with period at end-of-buffer — counts as complete sentence
        assert_eq!(buf.next_sentence(), Some("And another.".to_string()));
    }

    #[test]
    fn test_sentence_buffer_partial_word() {
        let mut buf = SentenceBuffer::new();
        buf.push("Hello worl");
        assert_eq!(buf.next_sentence(), None); // no sentence boundary
        buf.push("d. Done.");
        assert_eq!(buf.next_sentence(), Some("Hello world.".to_string()));
        assert_eq!(buf.next_sentence(), Some("Done.".to_string()));
    }

    #[test]
    fn test_sentence_buffer_flush_empty() {
        let mut buf = SentenceBuffer::new();
        assert_eq!(buf.flush(), None);
    }

    // --- VAD / VoiceSession state machine ---

    #[test]
    fn test_vad_silence_ignored() {
        let mut session = VoiceSession::new(
            SessionInitPayload::default(),
            VoiceConfig::default(),
            #[cfg(feature = "memory-candle")]
            None,
        )
        .unwrap();
        let silence = vec![0i16; OPUS_FRAME_SAMPLES];
        match session.handle_audio(&silence) {
            VoiceAction::Continue => {} // expected
            other => panic!(
                "Expected Continue for silence, got {:?}",
                std::mem::discriminant(&other)
            ),
        }
        assert_eq!(session.state, VoiceSessionState::Idle);
    }

    #[test]
    fn test_vad_speech_then_silence_triggers_transcribe() {
        let config = VoiceConfig {
            vad_silence_ms: 40, // 2 frames of silence at 20ms each
            vad_energy_threshold: 0.001,
            ..VoiceConfig::default()
        };
        let mut session = VoiceSession::new(
            SessionInitPayload::default(),
            config,
            #[cfg(feature = "memory-candle")]
            None,
        )
        .unwrap();

        // Send loud audio (speech) — first frame returns SpeechStarted
        let speech: Vec<i16> = (0..OPUS_FRAME_SAMPLES)
            .map(|i| (i % 50) as i16 * 500)
            .collect();
        assert!(matches!(
            session.handle_audio(&speech),
            VoiceAction::SpeechStarted
        ));
        assert_eq!(session.state, VoiceSessionState::Listening);

        // Second speech frame returns Continue (already listening)
        assert!(matches!(
            session.handle_audio(&speech),
            VoiceAction::Continue
        ));

        // Send silence
        let silence = vec![0i16; OPUS_FRAME_SAMPLES];
        assert!(matches!(
            session.handle_audio(&silence),
            VoiceAction::Continue
        ));
        // Second silence frame should trigger transcription
        match session.handle_audio(&silence) {
            VoiceAction::Transcribe(pcm) => {
                assert!(!pcm.is_empty());
                assert_eq!(session.state, VoiceSessionState::Transcribing);
            }
            other => panic!(
                "Expected Transcribe after silence threshold, got {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    #[test]
    fn test_vad_reset_to_idle() {
        let mut session = VoiceSession::new(
            SessionInitPayload::default(),
            VoiceConfig::default(),
            #[cfg(feature = "memory-candle")]
            None,
        )
        .unwrap();
        session.state = VoiceSessionState::Speaking;
        session.pcm_buffer.extend_from_slice(&[1, 2, 3]);
        session.reset_to_idle();
        assert_eq!(session.state, VoiceSessionState::Idle);
        assert!(session.pcm_buffer.is_empty());
    }

    // --- Opus codec tests ---

    #[test]
    fn test_opus_codec_creation() {
        let codec = OpusCodec::new();
        assert!(codec.is_ok());
    }

    #[test]
    fn test_opus_roundtrip_silence() {
        let mut codec = OpusCodec::new().unwrap();
        let silence = vec![0i16; OPUS_FRAME_SAMPLES];
        let encoded = codec.encode(&silence).unwrap();
        assert!(!encoded.is_empty());
        assert!(encoded.len() < silence.len() * 2); // compressed smaller than raw
        let decoded = codec.decode(&encoded).unwrap();
        assert_eq!(decoded.len(), OPUS_FRAME_SAMPLES);
        // Silence in should produce near-silence out
        let max_sample = decoded.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        assert!(
            max_sample < 100,
            "Expected near-silence, got max sample {max_sample}"
        );
    }

    #[test]
    fn test_opus_roundtrip_sine_wave() {
        let mut codec = OpusCodec::new().unwrap();
        // 440Hz sine wave at 16kHz
        let pcm: Vec<i16> = (0..OPUS_FRAME_SAMPLES)
            .map(|i| {
                let t = i as f64 / 16000.0;
                (f64::sin(2.0 * std::f64::consts::PI * 440.0 * t) * 16000.0) as i16
            })
            .collect();

        let encoded = codec.encode(&pcm).unwrap();
        assert!(!encoded.is_empty());

        let decoded = codec.decode(&encoded).unwrap();
        assert_eq!(decoded.len(), OPUS_FRAME_SAMPLES);
        // Should have non-trivial audio content
        let max_sample = decoded.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        assert!(
            max_sample > 1000,
            "Expected audible signal, got max sample {max_sample}"
        );
    }

    #[test]
    fn test_opus_multiple_frames() {
        let mut codec = OpusCodec::new().unwrap();
        // Encode and decode 10 frames to test stateful codec behavior
        for i in 0..10 {
            let pcm: Vec<i16> = (0..OPUS_FRAME_SAMPLES)
                .map(|j| ((i * OPUS_FRAME_SAMPLES + j) as i16).wrapping_mul(7))
                .collect();
            let encoded = codec.encode(&pcm).unwrap();
            let decoded = codec.decode(&encoded).unwrap();
            assert_eq!(decoded.len(), OPUS_FRAME_SAMPLES);
        }
    }
}
