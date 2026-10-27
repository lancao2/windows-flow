//! Mensagens tipadas, versionadas e com limites explícitos.
//!
//! Toda mensagem recebida de um peer passa por [`decode`], que impõe o limite
//! de tamanho do frame antes de desserializar e valida os campos depois.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Versão atual do protocolo. Incrementar em mudanças incompatíveis.
pub const PROTOCOL_VERSION: u16 = 1;

/// Tamanho máximo de um frame serializado (64 KiB). Chunks de arquivo terão
/// limite próprio na fase 5.
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
/// Tamanho máximo de texto de clipboard (1 MiB seria excessivo para o frame; o
/// clipboard usará canal próprio na fase 4).
pub const MAX_CLIPBOARD_BYTES: usize = 32 * 1024;
/// Limite de |dx| e |dy| por evento de ponteiro.
pub const MAX_POINTER_DELTA: i32 = 10_000;
/// Valor máximo de uma posição proporcional ao longo de uma borda.
pub const MAX_RATIO: u16 = 10_000;
/// Tamanho do nonce do Hello.
pub const NONCE_LEN: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonState {
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointerButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

/// Mensagens do plano de dados (Fase 0: apenas as de controle e entrada).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    Hello {
        protocol_version: u16,
        device_id: String,
        nonce: [u8; NONCE_LEN],
    },
    Heartbeat {
        seq: u64,
        timestamp_ms: u64,
    },
    /// Pede o controle do par. `entry` é a posição de entrada na borda, de 0 a
    /// [`MAX_RATIO`] ao longo do eixo da borda.
    ControlRequest {
        entry: u16,
    },
    ControlGranted,
    ControlDenied,
    /// Devolve o controle. `exit` é a posição de saída na borda (0..=[`MAX_RATIO`]).
    ControlReleased {
        exit: u16,
    },
    PointerMove {
        dx: i32,
        dy: i32,
        seq: u64,
    },
    PointerButton {
        button: PointerButton,
        state: ButtonState,
        seq: u64,
    },
    Scroll {
        dx: i32,
        dy: i32,
        seq: u64,
    },
    KeyEvent {
        scan_code: u16,
        extended: bool,
        state: ButtonState,
        seq: u64,
    },
    ClipboardText {
        origin_id: String,
        revision: u64,
        text: String,
    },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("frame excede o limite de {MAX_FRAME_BYTES} bytes")]
    FrameTooLarge,
    #[error("mensagem malformada")]
    Malformed,
    #[error("versão de protocolo incompatível: {0}")]
    UnsupportedVersion(u16),
    #[error("campo inválido: {0}")]
    InvalidField(&'static str),
}

impl Message {
    /// Valida limites semânticos de uma mensagem já desserializada.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        match self {
            Message::Hello {
                protocol_version,
                device_id,
                ..
            } => {
                if *protocol_version != PROTOCOL_VERSION {
                    return Err(ProtocolError::UnsupportedVersion(*protocol_version));
                }
                if device_id.is_empty() || device_id.len() > 128 {
                    return Err(ProtocolError::InvalidField("device_id"));
                }
            }
            Message::ControlRequest { entry: r } | Message::ControlReleased { exit: r }
                if *r > MAX_RATIO =>
            {
                return Err(ProtocolError::InvalidField("ratio"));
            }
            Message::PointerMove { dx, dy, .. } | Message::Scroll { dx, dy, .. }
                if dx.abs() > MAX_POINTER_DELTA || dy.abs() > MAX_POINTER_DELTA =>
            {
                return Err(ProtocolError::InvalidField("delta"));
            }
            Message::ClipboardText {
                origin_id, text, ..
            } => {
                if origin_id.is_empty() || origin_id.len() > 128 {
                    return Err(ProtocolError::InvalidField("origin_id"));
                }
                if text.len() > MAX_CLIPBOARD_BYTES {
                    return Err(ProtocolError::InvalidField("text"));
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// Serializa e confere o limite de tamanho.
pub fn encode(msg: &Message) -> Result<Vec<u8>, ProtocolError> {
    msg.validate()?;
    let bytes = serde_json::to_vec(msg).map_err(|_| ProtocolError::Malformed)?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }
    Ok(bytes)
}

/// Desserializa um frame vindo da rede: limite de tamanho, parse, validação.
pub fn decode(bytes: &[u8]) -> Result<Message, ProtocolError> {
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge);
    }
    let msg: Message = serde_json::from_slice(bytes).map_err(|_| ProtocolError::Malformed)?;
    msg.validate()?;
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_key_event() {
        let m = Message::KeyEvent {
            scan_code: 0x1E,
            extended: false,
            state: ButtonState::Down,
            seq: 7,
        };
        assert_eq!(decode(&encode(&m).unwrap()).unwrap(), m);
    }

    #[test]
    fn rejects_oversized_frame() {
        let big = vec![b' '; MAX_FRAME_BYTES + 1];
        assert_eq!(decode(&big), Err(ProtocolError::FrameTooLarge));
    }

    #[test]
    fn rejects_malformed_and_unknown() {
        assert_eq!(decode(b"not json"), Err(ProtocolError::Malformed));
        assert_eq!(decode(br#"{"type":"nope"}"#), Err(ProtocolError::Malformed));
    }

    #[test]
    fn rejects_wrong_version() {
        let m = Message::Hello {
            protocol_version: 99,
            device_id: "pc-a".into(),
            nonce: [0; NONCE_LEN],
        };
        assert_eq!(m.validate(), Err(ProtocolError::UnsupportedVersion(99)));
    }

    #[test]
    fn rejects_huge_pointer_delta() {
        let m = Message::PointerMove {
            dx: MAX_POINTER_DELTA + 1,
            dy: 0,
            seq: 1,
        };
        assert_eq!(encode(&m), Err(ProtocolError::InvalidField("delta")));
    }

    #[test]
    fn rejects_out_of_range_ratio() {
        let m = Message::ControlRequest {
            entry: MAX_RATIO + 1,
        };
        assert_eq!(encode(&m), Err(ProtocolError::InvalidField("ratio")));
        let ok = Message::ControlReleased { exit: MAX_RATIO };
        assert_eq!(decode(&encode(&ok).unwrap()).unwrap(), ok);
    }

    #[test]
    fn rejects_oversized_clipboard() {
        let m = Message::ClipboardText {
            origin_id: "a".into(),
            revision: 1,
            text: "x".repeat(MAX_CLIPBOARD_BYTES + 1),
        };
        assert_eq!(m.validate(), Err(ProtocolError::InvalidField("text")));
    }
}
