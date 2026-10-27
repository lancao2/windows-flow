//! Framing: `u32` big-endian com o tamanho, seguido do payload do protocolo.

use protocol::{Message, ProtocolError, MAX_FRAME_BYTES};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::TransportError;

pub async fn write_frame<W: AsyncWrite + Unpin>(
    w: &mut W,
    msg: &Message,
) -> Result<(), TransportError> {
    let bytes = protocol::encode(msg)?;
    let mut buf = Vec::with_capacity(4 + bytes.len());
    buf.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    buf.extend_from_slice(&bytes);
    w.write_all(&buf).await?;
    w.flush().await?;
    Ok(())
}

/// Lê um frame. O tamanho é validado antes de alocar o buffer.
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> Result<Message, TransportError> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await.map_err(eof_as_closed)?;
    let len = u32::from_be_bytes(len) as usize;
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge.into());
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await.map_err(eof_as_closed)?;
    Ok(protocol::decode(&buf)?)
}

fn eof_as_closed(e: std::io::Error) -> TransportError {
    if e.kind() == std::io::ErrorKind::UnexpectedEof {
        TransportError::Closed
    } else {
        TransportError::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{ButtonState, Message};

    #[tokio::test]
    async fn roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let m = Message::KeyEvent {
            scan_code: 30,
            extended: false,
            state: ButtonState::Down,
            seq: 1,
        };
        write_frame(&mut a, &m).await.unwrap();
        assert_eq!(read_frame(&mut b).await.unwrap(), m);
    }

    #[tokio::test]
    async fn rejects_oversized_length_without_allocating() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&u32::MAX.to_be_bytes()).await.unwrap();
        assert!(matches!(
            read_frame(&mut b).await,
            Err(TransportError::Protocol(ProtocolError::FrameTooLarge))
        ));
    }

    #[tokio::test]
    async fn rejects_garbage_payload() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(&5u32.to_be_bytes()).await.unwrap();
        a.write_all(b"xxxxx").await.unwrap();
        assert!(matches!(
            read_frame(&mut b).await,
            Err(TransportError::Protocol(ProtocolError::Malformed))
        ));
    }

    #[tokio::test]
    async fn eof_is_closed() {
        let (a, mut b) = tokio::io::duplex(64);
        drop(a);
        assert!(matches!(
            read_frame(&mut b).await,
            Err(TransportError::Closed)
        ));
    }
}
