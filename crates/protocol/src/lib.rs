//! Mensagens tipadas, versionadas e com limites explícitos.
//!
//! Toda mensagem recebida de um peer passa por [`decode`], que impõe o limite
//! de tamanho do frame antes de desserializar e valida os campos depois.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Versão atual do protocolo. Incrementar em mudanças incompatíveis.
pub const PROTOCOL_VERSION: u16 = 4;

/// Tamanho máximo de um frame serializado (64 KiB). Os chunks de arquivo viajam
/// em base64 dentro de um frame e por isso são menores ([`MAX_FILE_CHUNK`]).
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
/// Bytes de arquivo por chunk (antes do base64): 32 KiB viram ~43 KiB no frame.
pub const MAX_FILE_CHUNK: usize = 32 * 1024;
/// Tamanho máximo de um arquivo transferido (16 GiB).
pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// Arquivos por lote (uma pasta inteira conta arquivo a arquivo).
pub const MAX_FILES_PER_BATCH: u32 = 10_000;
/// Bytes por lote (64 GiB).
pub const MAX_BATCH_BYTES: u64 = 64 * 1024 * 1024 * 1024;
/// Profundidade máxima de pastas num caminho relativo.
pub const MAX_REL_DEPTH: usize = 32;
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
    /// Anuncia um arquivo de um lote. `index` começa em 0; `batch_bytes` é o total do lote
    /// e `ratio` a posição, na borda de quem envia, por onde o arquivo "atravessa".
    FileOffer {
        id: u64,
        /// Nome do arquivo (último componente de `rel`).
        name: String,
        /// Caminho relativo com `/`, a partir do item solto na borda: `a.txt` para um arquivo
        /// avulso ou `Pasta/sub/a.txt` dentro de uma pasta. O 1º componente é o item raiz.
        rel: String,
        size: u64,
        index: u32,
        count: u32,
        batch_bytes: u64,
        /// Total de bytes do item raiz (o arquivo avulso ou a pasta inteira).
        root_bytes: u64,
        /// Este é o último arquivo do item raiz.
        root_last: bool,
        ratio: u16,
    },
    /// Pedaço do arquivo `id` a partir de `offset` (dados em base64).
    FileChunk {
        id: u64,
        offset: u64,
        data: String,
    },
    /// Confirma o recebimento (e a gravação) dos bytes até `offset`; libera a janela de envio.
    FileAck {
        id: u64,
        offset: u64,
    },
    /// Cancela o arquivo `id` (qualquer um dos lados).
    FileCancel {
        id: u64,
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
            Message::FileOffer {
                name,
                rel,
                size,
                index,
                count,
                batch_bytes,
                root_bytes,
                ratio,
                ..
            } => {
                if !is_valid_file_name(name) {
                    return Err(ProtocolError::InvalidField("name"));
                }
                if !is_valid_rel(rel) || rel.rsplit('/').next() != Some(name.as_str()) {
                    return Err(ProtocolError::InvalidField("rel"));
                }
                if *size > MAX_FILE_BYTES
                    || *batch_bytes < *size
                    || *batch_bytes > MAX_BATCH_BYTES
                    || *root_bytes < *size
                    || *root_bytes > *batch_bytes
                {
                    return Err(ProtocolError::InvalidField("size"));
                }
                if *count == 0 || *count > MAX_FILES_PER_BATCH || index >= count {
                    return Err(ProtocolError::InvalidField("index"));
                }
                if *ratio > MAX_RATIO {
                    return Err(ProtocolError::InvalidField("ratio"));
                }
            }
            Message::FileChunk { offset, data, .. }
                if data.len() > base64_len(MAX_FILE_CHUNK) || *offset > MAX_FILE_BYTES =>
            {
                return Err(ProtocolError::InvalidField("data"));
            }
            _ => {}
        }
        Ok(())
    }
}

/// Caminho relativo seguro com `/`: cada componente é um nome válido, sem `..`, sem raiz e com
/// profundidade limitada.
pub fn is_valid_rel(rel: &str) -> bool {
    !rel.is_empty()
        && rel.len() <= 1_024
        && rel.split('/').count() <= MAX_REL_DEPTH
        && rel.split('/').all(is_valid_file_name)
}

/// Nome de arquivo simples: sem separadores, sem `:`, sem caracteres de controle e sem
/// terminar em ponto/espaço (o Windows os descartaria e mudaria o nome gravado).
pub fn is_valid_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 200
        && name != "."
        && name != ".."
        && !name.ends_with(['.', ' '])
        && !name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|'])
        && !name.chars().any(char::is_control)
}

/// Comprimento do base64 (com preenchimento) de `n` bytes.
pub const fn base64_len(n: usize) -> usize {
    n.div_ceil(3) * 4
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(base64_len(data.len()));
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Decodifica base64 padrão com preenchimento; `None` se malformado.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if !b.len().is_multiple_of(4) {
        return None;
    }
    let val = |c: u8| -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity(b.len() / 4 * 3);
    for (i, q) in b.chunks(4).enumerate() {
        let last = (i + 1) * 4 == b.len();
        let pad = q.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for &c in &q[..4 - pad] {
            n = (n << 6) | val(c)?;
        }
        n <<= 6 * pad as u32;
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
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
    fn base64_roundtrip_and_rejects_garbage() {
        for n in 0..40usize {
            let data: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            let enc = base64_encode(&data);
            assert_eq!(enc.len(), base64_len(n));
            assert_eq!(base64_decode(&enc).unwrap(), data);
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert!(base64_decode("TWE").is_none());
        assert!(base64_decode("T=Fu").is_none());
        assert!(base64_decode("TW!u").is_none());
    }

    #[test]
    fn full_chunk_fits_in_a_frame() {
        let m = Message::FileChunk {
            id: u64::MAX,
            offset: MAX_FILE_BYTES,
            data: base64_encode(&vec![0xAB; MAX_FILE_CHUNK]),
        };
        assert_eq!(decode(&encode(&m).unwrap()).unwrap(), m);
    }

    fn offer(name: &str, size: u64) -> Message {
        Message::FileOffer {
            id: 1,
            name: name.into(),
            rel: name.into(),
            size,
            index: 0,
            count: 1,
            batch_bytes: size,
            root_bytes: size,
            root_last: true,
            ratio: 5_000,
        }
    }

    #[test]
    fn relative_paths_stay_inside_the_root() {
        for bad in [
            "", "/a", "a//b", "../a", "a/../b", "a\\b", "C:/a", "a/", "a/b.",
        ] {
            assert!(!is_valid_rel(bad), "{bad:?}");
        }
        assert!(is_valid_rel("Pasta/sub/relatório.txt"));
        assert!(!is_valid_rel(&vec!["d"; MAX_REL_DEPTH + 1].join("/")));
        // `rel` precisa terminar no nome do arquivo anunciado.
        let m = Message::FileOffer {
            id: 1,
            name: "a.txt".into(),
            rel: "Pasta/b.txt".into(),
            size: 1,
            index: 0,
            count: 1,
            batch_bytes: 1,
            root_bytes: 1,
            root_last: true,
            ratio: 0,
        };
        assert_eq!(m.validate(), Err(ProtocolError::InvalidField("rel")));
    }

    #[test]
    fn file_offer_blocks_path_tricks_and_bad_sizes() {
        for bad in ["", "..", "a/b", "a\\b", "C:x", "x.", "x ", "a|b", "a\nb"] {
            assert_eq!(
                offer(bad, 1).validate(),
                Err(ProtocolError::InvalidField("name")),
                "{bad:?}"
            );
        }
        assert!(offer("relatório final.pdf", 10).validate().is_ok());
        assert_eq!(
            offer("a", MAX_FILE_BYTES + 1).validate(),
            Err(ProtocolError::InvalidField("size"))
        );
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
