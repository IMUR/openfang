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

use serde::{Deserialize, Serialize};
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
            let session_id = json["session_id"]
                .as_str()
                .unwrap_or("")
                .to_string();
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

/// A voice session associated with a WebSocket connection.
pub struct VoiceSession {
    /// Unique session identifier.
    pub session_id: String,
    /// Current state.
    pub state: VoiceSessionState,
    /// Client-requested configuration.
    pub init: SessionInitPayload,
}

impl VoiceSession {
    /// Create a new voice session from a SessionInit payload.
    pub fn new(init: SessionInitPayload) -> Self {
        Self {
            session_id: Uuid::new_v4().to_string(),
            state: VoiceSessionState::Idle,
            init,
        }
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
        let session = VoiceSession::new(SessionInitPayload::default());
        assert_eq!(session.state, VoiceSessionState::Idle);
        assert!(!session.session_id.is_empty());
        assert_eq!(session.init.sample_rate, 16000);
    }
}
