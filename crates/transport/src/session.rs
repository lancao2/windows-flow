//! Sessão sobre um canal já confirmado: Hello inicial, heartbeat, detecção de
//! perda do par e monotonicidade dos heartbeats.

use std::time::{Duration, Instant};

use protocol::Message;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{interval, timeout, MissedTickBehavior};

use crate::frame::{read_frame, write_frame};
use crate::tls::SecureChannel;
use crate::{TransportError, HANDSHAKE_TIMEOUT_MS, HEARTBEAT_INTERVAL_MS, HEARTBEAT_TIMEOUT_MS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// Nenhum frame dentro de [`HEARTBEAT_TIMEOUT_MS`].
    Timeout,
    PeerClosed,
    Io,
    /// Mensagem malformada, fora de ordem ou repetida.
    Protocol,
    /// O par fala outra versão do protocolo (a dele está no valor).
    PeerVersion(u16),
}

#[derive(Debug)]
pub enum SessionEvent {
    /// Mensagem de aplicação (o Hello é entregue; heartbeats não).
    Message(Message),
    Closed(CloseReason),
}

pub struct SessionHandle {
    tx: mpsc::Sender<Message>,
    tasks: Vec<JoinHandle<()>>,
}

impl SessionHandle {
    /// Enfileira sem esperar. Falso se a fila está cheia ou a sessão terminou.
    pub fn try_send(&self, msg: Message) -> bool {
        self.tx.try_send(msg).is_ok()
    }
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

fn reason_for(e: &TransportError) -> CloseReason {
    match e {
        TransportError::Closed => CloseReason::PeerClosed,
        TransportError::Protocol(protocol::ProtocolError::UnsupportedVersion(v)) => {
            CloseReason::PeerVersion(*v)
        }
        TransportError::Protocol(_) => CloseReason::Protocol,
        TransportError::Timeout => CloseReason::Timeout,
        _ => CloseReason::Io,
    }
}

/// Inicia a sessão. `hello` é sempre o primeiro frame enviado e o par deve
/// responder com o seu Hello como primeiro frame.
pub fn start(chan: SecureChannel, hello: Message) -> (SessionHandle, mpsc::Receiver<SessionEvent>) {
    let (mut reader, mut writer) = tokio::io::split(chan.io);
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(1024);
    let (ev_tx, ev_rx) = mpsc::channel::<SessionEvent>(1024);

    let ev_reader = ev_tx.clone();
    let reader_task = tokio::spawn(async move {
        let mut first = true;
        let mut last_hb: Option<u64> = None;
        loop {
            let limit = Duration::from_millis(if first {
                HANDSHAKE_TIMEOUT_MS
            } else {
                HEARTBEAT_TIMEOUT_MS
            });
            let msg = match timeout(limit, read_frame(&mut reader)).await {
                Err(_) => return close(&ev_reader, CloseReason::Timeout).await,
                Ok(Err(e)) => return close(&ev_reader, reason_for(&e)).await,
                Ok(Ok(m)) => m,
            };
            match (&msg, first) {
                (Message::Hello { .. }, true) => first = false,
                (_, true) | (Message::Hello { .. }, false) => {
                    return close(&ev_reader, CloseReason::Protocol).await;
                }
                (Message::Heartbeat { seq, .. }, false) => {
                    if last_hb.is_some_and(|l| *seq <= l) {
                        return close(&ev_reader, CloseReason::Protocol).await;
                    }
                    last_hb = Some(*seq);
                    continue;
                }
                _ => {}
            }
            if ev_reader.send(SessionEvent::Message(msg)).await.is_err() {
                return;
            }
        }
    });

    let writer_task = tokio::spawn(async move {
        let write_limit = Duration::from_millis(HEARTBEAT_TIMEOUT_MS);
        let started = Instant::now();
        let mut seq = 0u64;
        let mut tick = interval(Duration::from_millis(HEARTBEAT_INTERVAL_MS));
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let mut next = Some(hello);
        loop {
            let msg = match next.take() {
                Some(m) => m,
                None => tokio::select! {
                    m = out_rx.recv() => match m {
                        Some(m) => m,
                        None => return,
                    },
                    _ = tick.tick() => {
                        seq += 1;
                        Message::Heartbeat {
                            seq,
                            timestamp_ms: started.elapsed().as_millis() as u64,
                        }
                    }
                },
            };
            match timeout(write_limit, write_frame(&mut writer, &msg)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return close(&ev_tx, reason_for(&e)).await,
                Err(_) => return close(&ev_tx, CloseReason::Timeout).await,
            }
        }
    });

    (
        SessionHandle {
            tx: out_tx,
            tasks: vec![reader_task, writer_task],
        },
        ev_rx,
    )
}

async fn close(tx: &mpsc::Sender<SessionEvent>, reason: CloseReason) {
    let _ = tx.send(SessionEvent::Closed(reason)).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls::{connect, Identity, Listener, Pinned};
    use protocol::{ButtonState, NONCE_LEN};
    use std::sync::Arc;

    fn hello(id: &str) -> Message {
        Message::Hello {
            protocol_version: protocol::PROTOCOL_VERSION,
            device_id: id.into(),
            nonce: [0; NONCE_LEN],
            app_version: String::new(),
        }
    }

    async fn pair() -> (SecureChannel, SecureChannel) {
        let (a, b) = (Identity::generate().unwrap(), Identity::generate().unwrap());
        let listener = Listener::bind("127.0.0.1:0".parse().unwrap(), &a)
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let pinned: Pinned = Arc::default();
        let s = tokio::spawn(async move { listener.accept(&pinned).await.unwrap() });
        let c = connect(addr, &b, None).await.unwrap();
        (s.await.unwrap(), c)
    }

    async fn next_msg(rx: &mut mpsc::Receiver<SessionEvent>) -> SessionEvent {
        timeout(Duration::from_secs(5), rx.recv())
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn exchanges_hello_and_messages() {
        let (s, c) = pair().await;
        let (hs, mut rs) = start(s, hello("server"));
        let (hc, mut rc) = start(c, hello("client"));
        assert!(matches!(
            next_msg(&mut rs).await,
            SessionEvent::Message(Message::Hello { .. })
        ));
        assert!(matches!(
            next_msg(&mut rc).await,
            SessionEvent::Message(Message::Hello { .. })
        ));

        let key = Message::KeyEvent {
            scan_code: 1,
            extended: false,
            state: ButtonState::Up,
            seq: 1,
        };
        assert!(hc.try_send(key.clone()));
        match next_msg(&mut rs).await {
            SessionEvent::Message(m) => assert_eq!(m, key),
            e => panic!("{e:?}"),
        }
        drop(hs);
    }

    #[tokio::test]
    async fn silent_peer_is_detected_within_two_seconds() {
        let (s, c) = pair().await;
        let (_hs, mut rs) = start(s, hello("server"));
        // O par envia o Hello e depois fica mudo (sem heartbeats).
        let mut io = c.io;
        write_frame(&mut io, &hello("silent")).await.unwrap();
        assert!(matches!(
            next_msg(&mut rs).await,
            SessionEvent::Message(Message::Hello { .. })
        ));

        let t0 = Instant::now();
        match next_msg(&mut rs).await {
            SessionEvent::Closed(CloseReason::Timeout) => {}
            e => panic!("{e:?}"),
        }
        assert!(
            t0.elapsed() < Duration::from_millis(2_000),
            "{:?}",
            t0.elapsed()
        );
    }

    #[tokio::test]
    async fn dropped_peer_is_detected_quickly() {
        let (s, c) = pair().await;
        let (_hs, mut rs) = start(s, hello("server"));
        let mut io = c.io;
        write_frame(&mut io, &hello("gone")).await.unwrap();
        assert!(matches!(
            next_msg(&mut rs).await,
            SessionEvent::Message(Message::Hello { .. })
        ));
        drop(io);
        assert!(matches!(next_msg(&mut rs).await, SessionEvent::Closed(_)));
    }

    #[tokio::test]
    async fn first_frame_must_be_hello() {
        let (s, c) = pair().await;
        let (_hs, mut rs) = start(s, hello("server"));
        let mut io = c.io;
        write_frame(&mut io, &Message::ControlGranted)
            .await
            .unwrap();
        assert!(matches!(
            next_msg(&mut rs).await,
            SessionEvent::Closed(CloseReason::Protocol)
        ));
    }

    #[tokio::test]
    async fn repeated_heartbeat_seq_is_rejected() {
        let (s, c) = pair().await;
        let (_hs, mut rs) = start(s, hello("server"));
        let mut io = c.io;
        write_frame(&mut io, &hello("replay")).await.unwrap();
        let hb = Message::Heartbeat {
            seq: 5,
            timestamp_ms: 0,
        };
        write_frame(&mut io, &hb).await.unwrap();
        write_frame(&mut io, &hb).await.unwrap();
        assert!(matches!(
            next_msg(&mut rs).await,
            SessionEvent::Message(Message::Hello { .. })
        ));
        assert!(matches!(
            next_msg(&mut rs).await,
            SessionEvent::Closed(CloseReason::Protocol)
        ));
    }
}
