//! Conexão autenticada e criptografada entre pares (Fase 1).
//!
//! - [`tls`]: TLS 1.3 mútuo com certificados efêmeros. A confiança vem de um
//!   código de verificação (SAS) que os dois usuários comparam; depois disso a
//!   impressão digital do par fica fixada para reconexões automáticas.
//! - [`frame`]: framing com prefixo de tamanho e limite aplicado antes de alocar.
//! - [`session`]: heartbeat, timeout, monotonicidade de sequência.

pub mod frame;
pub mod session;
pub mod tls;

use thiserror::Error;

/// Inatividade máxima antes de declarar o par perdido (requisito: recuperar em até 2 s).
pub const HEARTBEAT_TIMEOUT_MS: u64 = 1_500;
/// Intervalo de envio de heartbeat.
pub const HEARTBEAT_INTERVAL_MS: u64 = 500;
/// Tempo máximo para os dois usuários confirmarem o código e trocarem o Hello.
pub const HANDSHAKE_TIMEOUT_MS: u64 = 60_000;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("erro de E/S: {0}")]
    Io(#[from] std::io::Error),
    #[error("erro TLS: {0}")]
    Tls(String),
    #[error("protocolo: {0}")]
    Protocol(#[from] protocol::ProtocolError),
    #[error("tempo esgotado")]
    Timeout,
    #[error("endereço fora da rede local")]
    NotLan,
    #[error("par fechou a conexão")]
    Closed,
}

impl From<rustls::Error> for TransportError {
    fn from(e: rustls::Error) -> Self {
        TransportError::Tls(e.to_string())
    }
}
