//! Motor de continuidade: liga transporte, máquina de estados e plataforma.
//!
//! Um único task é dono da plataforma e do estado; tudo chega por canais
//! (comandos da UI, entrada capturada, eventos da sessão). Invariantes:
//! - Só se captura/suprime entrada em REMOTE; qualquer falha chama [`Engine::recover`].
//! - Só se injeta entrada vinda do par enquanto este PC concedeu o controle.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use protocol::{Message, MAX_POINTER_DELTA, NONCE_LEN, PROTOCOL_VERSION};
use serde::Serialize;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tokio::time::{interval, MissedTickBehavior};
use transport::session::{self, SessionEvent, SessionHandle};
use transport::tls::{self, Identity, Listener, Pinned, SecureChannel};
use transport::TransportError;
use windows_input::{InputEvent, Platform, Point, Side};

use crate::geometry::{point_on_edge, ratio_along, EdgeDetector, EdgeSignal, ReturnDetector};
use crate::{Event, State};

/// Pixels para dentro da borda ao posicionar o cursor na entrada.
const ENTRY_INSET: i32 = 2;
/// Pixels para dentro da borda ao devolver o cursor na saída.
const EXIT_INSET: i32 = 3;
const GRANT_TIMEOUT: Duration = Duration::from_millis(1_000);
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(60);
const MOVE_FLUSH_INTERVAL: Duration = Duration::from_millis(4);
const RECONNECT_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub device_id: String,
    /// Lado em que o outro PC está posicionado.
    pub peer_side: Side,
    /// Tempo encostado na borda antes de pedir o controle.
    pub edge_dwell_ms: u64,
    pub rearm_px: i32,
    /// Empurrão (px) contra a borda de volta para devolver o controle.
    pub return_push_px: i32,
}

impl EngineConfig {
    pub fn new(device_id: impl Into<String>, peer_side: Side) -> Self {
        Self {
            device_id: device_id.into(),
            peer_side,
            edge_dwell_ms: 60,
            rearm_px: 24,
            return_push_px: 15,
        }
    }
}

#[derive(Debug)]
pub enum Command {
    /// Escuta conexões de um par.
    Host(SocketAddr),
    /// Conecta a um par que está escutando.
    Connect(SocketAddr),
    /// Resposta do usuário ao código de verificação.
    Confirm(bool),
    /// Encerra a conexão e esquece a confiança no par.
    Disconnect,
    SetPeerSide(Side),
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Status {
    /// local | armed | remote | recovering
    pub state: &'static str,
    /// idle | listening | connecting | awaiting_confirm | handshaking | connected
    pub link: &'static str,
    /// Este PC está sendo controlado pelo par.
    pub controlled: bool,
    pub peer_side: &'static str,
    pub peer: Option<String>,
    pub listening: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    Status(Status),
    /// O usuário deve comparar o código com o do outro PC e responder com [`Command::Confirm`].
    Confirm {
        sas: String,
        peer: String,
    },
    Notice(String),
    /// Efeito visual de portal na borda do lado do par, na posição `ratio` (0..=10000) ao longo dela.
    /// `enter`: o cursor está chegando a este PC; senão, está saindo. `screen` = [esq, topo, dir, base].
    Portal {
        side: &'static str,
        ratio: u16,
        enter: bool,
        screen: [i32; 4],
    },
}

#[derive(Clone)]
pub struct EngineHandle {
    tx: UnboundedSender<Command>,
}

impl EngineHandle {
    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }
}

enum Internal {
    Incoming(Box<SecureChannel>),
    Listening(SocketAddr),
    ModeFailed(String),
    Link(u64, SessionEvent),
}

enum Mode {
    None,
    Host(JoinHandle<()>),
    Client(JoinHandle<()>),
}

struct Link {
    handle: SessionHandle,
    ready: bool,
}

pub struct Engine<P: Platform> {
    p: P,
    cfg: EngineConfig,
    fsm: State,
    edge: EdgeDetector,
    ret: ReturnDetector,
    identity: Arc<Identity>,
    pinned: Pinned,
    busy: Arc<AtomicBool>,
    ui: UnboundedSender<UiEvent>,
    int_tx: UnboundedSender<Internal>,
    cmd_rx: Option<UnboundedReceiver<Command>>,
    input_rx: Option<UnboundedReceiver<InputEvent>>,
    int_rx: Option<UnboundedReceiver<Internal>>,
    mode: Mode,
    listening: Option<SocketAddr>,
    pending: Option<(SecureChannel, Instant)>,
    link: Option<Link>,
    peer_addr: Option<SocketAddr>,
    gen: u64,
    controlled: bool,
    seq: u64,
    last_in_seq: u64,
    pending_move: (i32, i32),
    last_flush: Instant,
    awaiting_grant: Option<Instant>,
    entry_pos: Option<Point>,
    entry_ratio: u16,
    epoch: Instant,
    nonce_counter: u64,
}

impl<P: Platform + 'static> Engine<P> {
    pub fn new(
        platform: P,
        input_rx: UnboundedReceiver<InputEvent>,
        cfg: EngineConfig,
        ui: UnboundedSender<UiEvent>,
    ) -> Result<(Self, EngineHandle), TransportError> {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (int_tx, int_rx) = mpsc::unbounded_channel();
        let engine = Self {
            p: platform,
            edge: EdgeDetector::new(cfg.peer_side, cfg.edge_dwell_ms, cfg.rearm_px),
            ret: ReturnDetector::new(cfg.peer_side, cfg.return_push_px),
            cfg,
            fsm: State::Local,
            identity: Arc::new(Identity::generate()?),
            pinned: Arc::new(Mutex::new(None)),
            busy: Arc::new(AtomicBool::new(false)),
            ui,
            int_tx,
            cmd_rx: Some(cmd_rx),
            input_rx: Some(input_rx),
            int_rx: Some(int_rx),
            mode: Mode::None,
            listening: None,
            pending: None,
            link: None,
            peer_addr: None,
            gen: 0,
            controlled: false,
            seq: 0,
            last_in_seq: 0,
            pending_move: (0, 0),
            last_flush: Instant::now(),
            awaiting_grant: None,
            entry_pos: None,
            entry_ratio: 0,
            epoch: Instant::now(),
            nonce_counter: 0,
        };
        Ok((engine, EngineHandle { tx: cmd_tx }))
    }

    pub async fn run(mut self) {
        let mut cmd_rx = self.cmd_rx.take().expect("run chamado uma vez");
        let mut input_rx = self.input_rx.take().expect("run chamado uma vez");
        let mut int_rx = self.int_rx.take().expect("run chamado uma vez");
        let mut fast = interval(Duration::from_millis(8));
        fast.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut slow = interval(Duration::from_millis(250));
        slow.set_missed_tick_behavior(MissedTickBehavior::Delay);
        self.emit_status();

        loop {
            let fast_on = self.link_ready();
            tokio::select! {
                cmd = cmd_rx.recv() => match cmd {
                    Some(c) => if self.on_command(c) { break },
                    None => break,
                },
                ev = input_rx.recv() => match ev {
                    Some(e) => self.on_input(e),
                    None => break,
                },
                i = int_rx.recv() => if let Some(i) = i { self.on_internal(i) },
                _ = fast.tick(), if fast_on => self.on_fast_tick(),
                _ = slow.tick() => self.on_slow_tick(),
            }
        }
        self.recover("encerrando");
        self.drop_link();
        self.stop_mode();
    }

    // ---- utilidades ------------------------------------------------------

    fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    fn link_ready(&self) -> bool {
        self.link.as_ref().is_some_and(|l| l.ready)
    }

    fn notice(&self, s: impl Into<String>) {
        let _ = self.ui.send(UiEvent::Notice(s.into()));
    }

    fn status(&self) -> Status {
        let link = match (&self.link, &self.pending, &self.mode) {
            (Some(l), _, _) if l.ready => "connected",
            (Some(_), _, _) => "handshaking",
            (None, Some(_), _) => "awaiting_confirm",
            (None, None, Mode::Client(_)) => "connecting",
            (None, None, Mode::Host(_)) => "listening",
            _ => "idle",
        };
        Status {
            state: self.fsm.as_str(),
            link,
            controlled: self.controlled,
            peer_side: side_str(self.cfg.peer_side),
            peer: self.peer_addr.map(|a| a.to_string()),
            listening: self.listening.map(|a| a.to_string()),
        }
    }

    fn emit_status(&self) {
        let _ = self.ui.send(UiEvent::Status(self.status()));
    }

    fn fsm_event(&mut self, ev: Event) {
        self.fsm = self.fsm.next(ev);
    }

    fn send(&self, m: Message) -> bool {
        self.link.as_ref().is_some_and(|l| l.handle.try_send(m))
    }

    fn send_input(&mut self, make: impl FnOnce(u64) -> Message) {
        self.seq += 1;
        let m = make(self.seq);
        if !self.send(m) {
            self.recover("fila de saída cheia ou sessão encerrada");
        }
    }

    fn nonce(&mut self) -> [u8; NONCE_LEN] {
        use std::hash::{BuildHasher, Hasher};
        self.nonce_counter += 1;
        let mut out = [0u8; NONCE_LEN];
        for (i, chunk) in out.chunks_mut(8).enumerate() {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u64(self.nonce_counter + i as u64);
            chunk.copy_from_slice(&h.finish().to_be_bytes());
        }
        out
    }

    // ---- recuperação -----------------------------------------------------

    /// Devolve o controle local imediatamente: desliga a captura, solta teclas
    /// e botões injetados e volta ao estado LOCAL. Seguro de chamar a qualquer momento.
    pub fn recover(&mut self, reason: &str) {
        let was_remote = self.fsm == State::Remote;
        self.fsm_event(Event::Failure);
        self.p.capture(false);
        self.p.release_all();
        if was_remote {
            self.send(Message::ControlReleased { exit: 0 });
            if let Some(p) = self.entry_pos.take() {
                self.p.set_cursor(p);
            }
        }
        self.controlled = false;
        self.awaiting_grant = None;
        self.pending_move = (0, 0);
        self.edge.block();
        self.ret.reset();
        self.fsm_event(Event::RecoveryComplete);
        if was_remote || reason != "encerrando" {
            self.notice(format!("Controle local restaurado: {reason}"));
        }
        self.emit_status();
    }

    fn portal(&self, ratio: u16, enter: bool) {
        let s = self.p.screen();
        let _ = self.ui.send(UiEvent::Portal {
            side: side_str(self.cfg.peer_side),
            ratio,
            enter,
            screen: [s.left, s.top, s.right, s.bottom],
        });
    }

    /// Encerra o estado "controlado" neste PC: solta o que foi injetado e libera o bloqueio da entrada física.
    fn end_control(&mut self) {
        self.controlled = false;
        self.p.release_all();
        self.p.block_local(false);
        // O cursor fica parado na borda por onde acabou de sair: não pode contar como
        // intenção de cruzar de volta.
        self.disarm();
    }

    /// Cancela qualquer cruzamento em andamento e exige afastar-se da borda antes de rearmar.
    fn disarm(&mut self) {
        if self.fsm == State::Armed {
            self.fsm_event(Event::EdgeLeft);
        }
        self.edge.block();
    }

    fn drop_link(&mut self) {
        self.p.performance(false);
        self.link = None;
        self.peer_addr = None;
        self.gen += 1;
        self.busy.store(self.pending.is_some(), Ordering::Relaxed);
        self.emit_status();
    }

    fn stop_mode(&mut self) {
        match std::mem::replace(&mut self.mode, Mode::None) {
            Mode::Host(h) | Mode::Client(h) => h.abort(),
            Mode::None => {}
        }
        self.listening = None;
    }

    // ---- comandos --------------------------------------------------------

    /// Retorna `true` para encerrar o motor.
    fn on_command(&mut self, cmd: Command) -> bool {
        match cmd {
            Command::Host(addr) => {
                self.stop_mode();
                let (id, pinned, tx) = (
                    self.identity.clone(),
                    self.pinned.clone(),
                    self.int_tx.clone(),
                );
                self.mode = Mode::Host(tokio::spawn(host_task(addr, id, pinned, tx)));
                self.emit_status();
            }
            Command::Connect(addr) => {
                self.stop_mode();
                let (id, pinned, tx, busy) = (
                    self.identity.clone(),
                    self.pinned.clone(),
                    self.int_tx.clone(),
                    self.busy.clone(),
                );
                self.mode = Mode::Client(tokio::spawn(client_task(addr, id, pinned, busy, tx)));
                self.emit_status();
            }
            Command::Confirm(accept) => {
                if let Some((chan, _)) = self.pending.take() {
                    if accept {
                        *self.pinned.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(chan.peer_fingerprint);
                        self.start_session(chan);
                    } else {
                        self.busy.store(self.link.is_some(), Ordering::Relaxed);
                        self.notice("Pareamento recusado.");
                    }
                    self.emit_status();
                }
            }
            Command::Disconnect => {
                self.recover("desconectado pelo usuário");
                self.pending = None;
                self.drop_link();
                self.stop_mode();
                *self.pinned.lock().unwrap_or_else(|e| e.into_inner()) = None;
                self.emit_status();
            }
            Command::SetPeerSide(side) => {
                if self.fsm == State::Remote || self.controlled {
                    self.notice("Não é possível mudar a borda durante o controle.");
                } else {
                    self.cfg.peer_side = side;
                    self.edge.set_side(side);
                    self.ret.set_side(side);
                    self.emit_status();
                }
            }
            Command::Shutdown => return true,
        }
        false
    }

    // ---- eventos internos ------------------------------------------------

    fn on_internal(&mut self, i: Internal) {
        match i {
            Internal::Listening(a) => {
                self.listening = Some(a);
                self.emit_status();
            }
            Internal::ModeFailed(msg) => {
                self.stop_mode();
                self.notice(msg);
                self.emit_status();
            }
            Internal::Incoming(chan) => self.on_incoming(*chan),
            Internal::Link(gen, ev) if gen == self.gen => match ev {
                SessionEvent::Message(m) => self.on_message(m),
                SessionEvent::Closed(reason) => {
                    self.recover(&format!("conexão encerrada ({reason:?})"));
                    self.drop_link();
                }
            },
            Internal::Link(..) => {}
        }
    }

    fn on_incoming(&mut self, chan: SecureChannel) {
        if chan.pinned_match {
            // Par já confirmado nesta execução: reconexão automática.
            if self.link.is_some() {
                self.recover("reconexão do par");
                self.drop_link();
            }
            self.pending = None;
            self.start_session(chan);
            self.emit_status();
        } else if self.pending.is_none() && self.link.is_none() {
            self.busy.store(true, Ordering::Relaxed);
            let _ = self.ui.send(UiEvent::Confirm {
                sas: chan.sas.clone(),
                peer: chan.peer_addr.to_string(),
            });
            self.pending = Some((chan, Instant::now()));
            self.emit_status();
        }
        // Caso contrário: já ocupado, descarta (o canal é fechado ao sair do escopo).
    }

    fn start_session(&mut self, chan: SecureChannel) {
        let peer = chan.peer_addr;
        let hello = Message::Hello {
            protocol_version: PROTOCOL_VERSION,
            device_id: self.cfg.device_id.clone(),
            nonce: self.nonce(),
        };
        let (handle, mut rx) = session::start(chan, hello);
        self.gen += 1;
        let gen = self.gen;
        let tx = self.int_tx.clone();
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                if tx.send(Internal::Link(gen, ev)).is_err() {
                    break;
                }
            }
        });
        self.link = Some(Link {
            handle,
            ready: false,
        });
        self.peer_addr = Some(peer);
        self.busy.store(true, Ordering::Relaxed);
        self.seq = 0;
        self.last_in_seq = 0;
    }

    // ---- mensagens do par ------------------------------------------------

    fn on_message(&mut self, m: Message) {
        match m {
            Message::Hello { .. } => {
                if let Some(l) = self.link.as_mut() {
                    l.ready = true;
                }
                self.p.performance(true);
                self.notice("Conectado ao outro PC.");
                self.emit_status();
            }
            Message::ControlRequest { entry } => self.on_control_request(entry),
            Message::ControlGranted => {
                if self.awaiting_grant.take().is_some() && self.fsm == State::Armed {
                    self.fsm_event(Event::ControlGranted);
                    self.pending_move = (0, 0);
                    self.last_flush = Instant::now();
                    self.p.capture(true);
                    self.portal(self.entry_ratio, false);
                    self.emit_status();
                }
            }
            Message::ControlDenied => {
                if self.awaiting_grant.take().is_some() {
                    self.abort_crossing();
                }
            }
            Message::ControlReleased { exit } => self.on_control_released(exit),
            Message::PointerMove { dx, dy, seq } => {
                if self.accept_input(seq) {
                    self.apply_move(dx, dy);
                }
            }
            Message::PointerButton { button, state, seq } => {
                if self.accept_input(seq) {
                    self.p.inject(&InputEvent::Button { button, state });
                }
            }
            Message::Scroll { dx, dy, seq } => {
                if self.accept_input(seq) {
                    self.p.inject(&InputEvent::Scroll { dx, dy });
                }
            }
            Message::KeyEvent {
                scan_code,
                extended,
                state,
                seq,
            } => {
                if self.accept_input(seq) {
                    self.p.inject(&InputEvent::Key {
                        scan_code,
                        extended,
                        state,
                    });
                }
            }
            // Heartbeat é tratado pela sessão; clipboard e arquivos são de fases futuras.
            Message::Heartbeat { .. } | Message::ClipboardText { .. } => {}
        }
    }

    /// Só aceita entrada do par enquanto concedemos o controle, e com sequência crescente.
    fn accept_input(&mut self, seq: u64) -> bool {
        if !self.controlled || seq <= self.last_in_seq {
            return false;
        }
        self.last_in_seq = seq;
        true
    }

    fn on_control_request(&mut self, entry: u16) {
        // ARMED (cursor encostado na borda sem ter pedido nada) não impede conceder o controle.
        let idle = matches!(self.fsm, State::Local | State::Armed);
        let free = self.link_ready() && idle && !self.controlled && self.awaiting_grant.is_none();
        if !free {
            self.send(Message::ControlDenied);
            return;
        }
        let screen = self.p.screen();
        self.p.set_cursor(point_on_edge(
            screen,
            self.cfg.peer_side,
            entry,
            ENTRY_INSET,
        ));
        self.disarm();
        self.controlled = true;
        self.last_in_seq = 0;
        self.ret.reset();
        self.p.block_local(true);
        self.portal(entry, true);
        self.send(Message::ControlGranted);
        self.emit_status();
    }

    fn on_control_released(&mut self, exit: u16) {
        if self.fsm == State::Remote {
            self.p.capture(false);
            self.fsm_event(Event::ControlReleased);
            self.fsm_event(Event::RecoveryComplete);
            let screen = self.p.screen();
            self.p
                .set_cursor(point_on_edge(screen, self.cfg.peer_side, exit, EXIT_INSET));
            self.entry_pos = None;
            self.pending_move = (0, 0);
            self.edge.block();
            self.portal(exit, true);
            self.emit_status();
        } else if self.controlled {
            // O par encerrou o controle (ex.: atalho de emergência dele).
            self.end_control();
            self.emit_status();
        }
    }

    fn apply_move(&mut self, dx: i32, dy: i32) {
        let screen = self.p.screen();
        let cur = self.p.cursor();
        let target = screen.clamp(Point {
            x: cur.x + dx,
            y: cur.y + dy,
        });
        self.p.move_cursor(target);
        if self.ret.update(target, screen, dx, dy) {
            let exit = ratio_along(target, screen, self.cfg.peer_side);
            self.end_control();
            self.send(Message::ControlReleased { exit });
            self.portal(exit, false);
            self.emit_status();
        }
    }

    // ---- entrada local ---------------------------------------------------

    /// O usuário clicou/digitou neste PC enquanto era controlado: devolve o controle a ele.
    fn on_takeover(&mut self) {
        if !self.controlled {
            return;
        }
        let screen = self.p.screen();
        let exit = ratio_along(self.p.cursor(), screen, self.cfg.peer_side);
        self.end_control();
        self.send(Message::ControlReleased { exit });
        self.portal(exit, false);
        self.notice("Controle retomado neste PC.");
        self.emit_status();
    }

    fn on_input(&mut self, ev: InputEvent) {
        match ev {
            InputEvent::Emergency => self.recover("atalho de emergência"),
            InputEvent::Takeover => self.on_takeover(),
            _ if self.fsm != State::Remote => {}
            InputEvent::Move { dx, dy } => {
                self.pending_move.0 += dx;
                self.pending_move.1 += dy;
                if self.last_flush.elapsed() >= MOVE_FLUSH_INTERVAL {
                    self.flush_move();
                }
            }
            InputEvent::Button { button, state } => {
                self.flush_move();
                self.send_input(|seq| Message::PointerButton { button, state, seq });
            }
            InputEvent::Scroll { dx, dy } => {
                self.flush_move();
                self.send_input(|seq| Message::Scroll { dx, dy, seq });
            }
            InputEvent::Key {
                scan_code,
                extended,
                state,
            } => {
                self.flush_move();
                self.send_input(|seq| Message::KeyEvent {
                    scan_code,
                    extended,
                    state,
                    seq,
                });
            }
        }
    }

    /// Envia o movimento acumulado (coalescido). Cliques e teclas nunca são coalescidos.
    fn flush_move(&mut self) {
        let (dx, dy) = std::mem::take(&mut self.pending_move);
        self.last_flush = Instant::now();
        if (dx, dy) == (0, 0) {
            return;
        }
        let c = |v: i32| v.clamp(-MAX_POINTER_DELTA, MAX_POINTER_DELTA);
        self.send_input(|seq| Message::PointerMove {
            dx: c(dx),
            dy: c(dy),
            seq,
        });
    }

    // ---- ticks -----------------------------------------------------------

    fn on_fast_tick(&mut self) {
        self.p.keepalive();
        if self.fsm == State::Remote {
            self.flush_move();
            return;
        }
        if let Some(t) = self.awaiting_grant {
            if t.elapsed() > GRANT_TIMEOUT {
                self.awaiting_grant = None;
                self.abort_crossing();
            }
            return;
        }
        if self.controlled || !self.link_ready() {
            return;
        }
        let (pos, screen, now) = (self.p.cursor(), self.p.screen(), self.now_ms());
        let signal = self.edge.update(now, pos, screen);
        match signal {
            EdgeSignal::None => return,
            EdgeSignal::Entered => self.fsm_event(Event::EdgeEntered),
            EdgeSignal::Left => self.fsm_event(Event::EdgeLeft),
            EdgeSignal::Cross(ratio) if self.fsm == State::Armed => {
                self.entry_pos = Some(pos);
                self.entry_ratio = ratio;
                self.awaiting_grant = Some(Instant::now());
                if !self.send(Message::ControlRequest { entry: ratio }) {
                    self.recover("não foi possível pedir o controle");
                }
            }
            EdgeSignal::Cross(_) => {}
        }
        self.emit_status();
    }

    fn abort_crossing(&mut self) {
        self.fsm_event(Event::EdgeLeft);
        self.entry_pos = None;
        self.edge.block();
        self.emit_status();
    }

    fn on_slow_tick(&mut self) {
        self.p.keepalive();
        if self
            .pending
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed() > CONFIRM_TIMEOUT)
        {
            self.pending = None;
            self.busy.store(self.link.is_some(), Ordering::Relaxed);
            self.notice("Tempo para confirmar o código esgotado.");
            self.emit_status();
        }
    }
}

fn side_str(s: Side) -> &'static str {
    match s {
        Side::Left => "left",
        Side::Right => "right",
        Side::Top => "top",
        Side::Bottom => "bottom",
    }
}

async fn host_task(
    addr: SocketAddr,
    id: Arc<Identity>,
    pinned: Pinned,
    tx: UnboundedSender<Internal>,
) {
    let listener = match Listener::bind(addr, &id).await {
        Ok(l) => l,
        Err(e) => {
            let _ = tx.send(Internal::ModeFailed(format!(
                "Não foi possível escutar em {addr}: {e}"
            )));
            return;
        }
    };
    if let Ok(a) = listener.local_addr() {
        let _ = tx.send(Internal::Listening(a));
    }
    loop {
        match listener.accept(&pinned).await {
            Ok(chan) => {
                if tx.send(Internal::Incoming(Box::new(chan))).is_err() {
                    return;
                }
            }
            Err(e) => {
                let _ = tx.send(Internal::ModeFailed(format!(
                    "Erro ao aceitar conexões: {e}"
                )));
                return;
            }
        }
    }
}

/// Conecta ao anfitrião. A primeira falha (sem par confirmado) é reportada;
/// depois da confirmação, reconecta sozinho com a identidade fixada.
async fn client_task(
    addr: SocketAddr,
    id: Arc<Identity>,
    pinned: Pinned,
    busy: Arc<AtomicBool>,
    tx: UnboundedSender<Internal>,
) {
    loop {
        if !busy.load(Ordering::Relaxed) {
            let pin = *pinned.lock().unwrap_or_else(|e| e.into_inner());
            match tls::connect(addr, &id, pin).await {
                Ok(chan) => {
                    if tx.send(Internal::Incoming(Box::new(chan))).is_err() {
                        return;
                    }
                }
                Err(e) if pin.is_none() => {
                    let _ = tx.send(Internal::ModeFailed(format!(
                        "Falha ao conectar a {addr}: {e}"
                    )));
                    return;
                }
                Err(_) => {}
            }
        }
        tokio::time::sleep(RECONNECT_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests;
