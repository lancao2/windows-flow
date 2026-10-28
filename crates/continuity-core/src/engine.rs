//! Motor de continuidade: liga transporte, máquina de estados e plataforma.
//!
//! Um único task é dono da plataforma e do estado; tudo chega por canais
//! (comandos da UI, entrada capturada, eventos da sessão). Invariantes:
//! - Só se captura/suprime entrada em REMOTE; qualquer falha chama [`Engine::recover`].
//! - Só se injeta entrada vinda do par enquanto este PC concedeu o controle.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use file_transfer::{create_unique_dir, disk_name, plan, Incoming, Outgoing};
use protocol::{
    base64_decode, base64_encode, Message, MAX_FILES_PER_BATCH, MAX_FILE_CHUNK, MAX_POINTER_DELTA,
    NONCE_LEN, PROTOCOL_VERSION,
};
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
    /// Onde os arquivos recebidos são gravados.
    pub download_dir: PathBuf,
}

impl EngineConfig {
    pub fn new(device_id: impl Into<String>, peer_side: Side) -> Self {
        Self {
            device_id: device_id.into(),
            peer_side,
            edge_dwell_ms: 60,
            rearm_px: 24,
            return_push_px: 15,
            download_dir: std::env::var_os("USERPROFILE")
                .map(|h| PathBuf::from(h).join("Downloads"))
                .unwrap_or_else(|| PathBuf::from("Downloads")),
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
    Confirm {
        accept: bool,
        /// Confiar neste computador: nas próximas conexões não pede o código de novo.
        remember: bool,
    },
    /// Substitui a lista de computadores confiáveis (impressões digitais).
    SetTrusted(Vec<[u8; 32]>),
    /// Cancela os envios e recebimentos em andamento (nos dois PCs).
    CancelTransfers,
    /// Encerra a conexão e esquece a confiança no par.
    Disconnect,
    SetPeerSide(Side),
    /// Envia arquivos ao par; `ratio` é a posição na borda por onde "atravessam".
    SendFiles {
        paths: Vec<PathBuf>,
        ratio: u16,
    },
    Shutdown,
}

/// Andamento de um arquivo de um lote, para a bandeja e o anel de progresso.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileEvent {
    /// `in` (recebendo) ou `out` (enviando).
    pub dir: &'static str,
    /// `offer` | `progress` | `done` | `failed`
    pub state: &'static str,
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub index: u32,
    pub count: u32,
    /// Bytes já concluídos do lote (incluindo o andamento do arquivo atual).
    pub batch_done: u64,
    pub batch_bytes: u64,
    /// Posição na borda (0..=10000) por onde o arquivo atravessa.
    pub ratio: u16,
    pub side: &'static str,
    pub screen: [i32; 4],
    /// Caminho final do arquivo recebido (só em `done`).
    pub path: Option<String>,
    /// Item raiz a que o arquivo pertence: o arquivo avulso ou a pasta inteira.
    pub root: String,
    /// Caminho final do item raiz (só em `done`; a pasta ou o arquivo).
    pub root_path: Option<String>,
    pub root_bytes: u64,
    /// Este foi o último arquivo do item raiz (só em `done`).
    pub root_last: bool,
    /// O item raiz é uma pasta.
    pub is_dir: bool,
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
    /// Versão do app no outro PC (depois do Hello).
    pub peer_version: Option<String>,
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
    File(FileEvent),
    /// O par completou a conexão: nome do computador e se ele é (ou acabou de ser) confiável.
    PeerHello {
        name: String,
        /// Impressão digital (hex) do certificado do par.
        fingerprint: String,
        trusted: bool,
        /// O usuário acabou de marcar "lembrar este computador".
        remembered: bool,
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

/// Lote em envio: arquivos já abertos, na ordem.
struct OutBatch {
    queue: VecDeque<Outgoing>,
    count: u32,
    bytes: u64,
    /// Bytes dos arquivos já confirmados.
    done: u64,
    ratio: u16,
    next_index: u32,
}

/// Lote em recebimento.
struct InBatch {
    count: u32,
    bytes: u64,
    done: u64,
    ratio: u16,
    index: u32,
    /// Item raiz do arquivo atual.
    root: String,
    root_bytes: u64,
    root_last: bool,
    /// Pasta de destino real (com sufixo se já existia), quando o item raiz é uma pasta.
    root_dir: Option<PathBuf>,
}

/// Cópia dos números do lote para montar um [`FileEvent`].
struct InBatchView {
    count: u32,
    bytes: u64,
    done: u64,
    ratio: u16,
    root: String,
    root_bytes: u64,
    root_last: bool,
    root_dir: Option<PathBuf>,
    is_dir: bool,
}

impl InBatch {
    fn view(&self) -> InBatchView {
        InBatchView {
            count: self.count,
            bytes: self.bytes,
            done: self.done,
            ratio: self.ratio,
            root: self.root.clone(),
            root_bytes: self.root_bytes,
            root_last: self.root_last,
            root_dir: self.root_dir.clone(),
            is_dir: self.root_dir.is_some(),
        }
    }
}

const PROGRESS_INTERVAL: Duration = Duration::from_millis(40);
/// O receptor confirma a cada tantos chunks (e sempre ao completar).
const ACK_EVERY: u64 = 4 * MAX_FILE_CHUNK as u64;

pub struct Engine<P: Platform> {
    p: P,
    cfg: EngineConfig,
    fsm: State,
    edge: EdgeDetector,
    ret: ReturnDetector,
    identity: Arc<Identity>,
    trusted: HashSet<[u8; 32]>,
    peer_fp: Option<[u8; 32]>,
    peer_version: Option<String>,
    /// Este PC iniciou a conexão (cliente): ele é quem informa a posição ao par no Hello.
    role_client: bool,
    last_out_progress: Instant,
    remember_pending: bool,
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
    out_batch: Option<OutBatch>,
    outgoing: Option<Outgoing>,
    out_id: u64,
    in_batch: Option<InBatch>,
    /// Pastas raiz já criadas neste lote: nome anunciado -> pasta real em Downloads.
    in_roots: HashMap<String, PathBuf>,
    incoming: Option<Incoming>,
    last_acked: u64,
    last_progress: Instant,
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
            trusted: HashSet::new(),
            peer_fp: None,
            peer_version: None,
            role_client: false,
            last_out_progress: Instant::now(),
            remember_pending: false,
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
            out_batch: None,
            outgoing: None,
            out_id: 0,
            in_batch: None,
            in_roots: HashMap::new(),
            incoming: None,
            last_acked: 0,
            last_progress: Instant::now(),
        };
        Ok((engine, EngineHandle { tx: cmd_tx }))
    }

    /// Usa uma identidade fixa (lida do disco) em vez da efêmera; chamar antes de [`Engine::run`].
    pub fn set_identity(&mut self, identity: Identity) {
        self.identity = Arc::new(identity);
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
            peer_version: self.peer_version.clone(),
            listening: self.listening.map(|a| a.to_string()),
        }
    }

    fn apply_peer_side(&mut self, side: Side) {
        self.cfg.peer_side = side;
        self.edge.set_side(side);
        self.ret.set_side(side);
        self.emit_status();
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
        self.peer_fp = None;
        self.peer_version = None;
        self.abort_transfers(false);
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
            Command::SetTrusted(list) => self.trusted = list.into_iter().collect(),
            Command::Confirm { accept, remember } => {
                if let Some((chan, _)) = self.pending.take() {
                    if accept {
                        *self.pinned.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(chan.peer_fingerprint);
                        if remember {
                            self.trusted.insert(chan.peer_fingerprint);
                            self.remember_pending = true;
                        }
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
                    self.apply_peer_side(side);
                    // O outro PC adota o lado oposto sozinho.
                    self.send(Message::PeerSide {
                        side: side_str(side).to_string(),
                    });
                }
            }
            Command::CancelTransfers => self.cancel_transfers(),
            Command::SendFiles { paths, ratio } => self.start_send(paths, ratio),
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
                    if let transport::session::CloseReason::PeerVersion(v) = reason {
                        self.notice(format!(
                            "Versão incompatível: o outro PC usa o Windows Flow {} (protocolo {v}) e este usa a {} (protocolo {PROTOCOL_VERSION}). Atualize o mais antigo para a mesma versão.",
                            protocol::app_versions_for_protocol(v),
                            env!("CARGO_PKG_VERSION"),
                        ));
                    } else if reason == transport::session::CloseReason::Protocol {
                        self.notice(
                            "Os dois PCs precisam estar na mesma versão do Windows Flow (veja                              a versão no topo do app). Atualize o outro PC.",
                        );
                    }
                    self.drop_link();
                }
            },
            Internal::Link(..) => {}
        }
    }

    fn on_incoming(&mut self, chan: SecureChannel) {
        self.role_client = matches!(self.mode, Mode::Client(_));
        if chan.pinned_match || self.trusted.contains(&chan.peer_fingerprint) {
            // Par já confirmado (nesta execução ou salvo como confiável): sem pedir o código.
            *self.pinned.lock().unwrap_or_else(|e| e.into_inner()) = Some(chan.peer_fingerprint);
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
        self.peer_fp = Some(chan.peer_fingerprint);
        let hello = Message::Hello {
            protocol_version: PROTOCOL_VERSION,
            device_id: self.cfg.device_id.clone(),
            nonce: self.nonce(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
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
            Message::Hello {
                device_id,
                app_version,
                ..
            } => {
                if let Some(l) = self.link.as_mut() {
                    l.ready = true;
                }
                self.peer_version = (!app_version.is_empty()).then_some(app_version);
                // Quem iniciou a conexão informa a posição: o outro PC adota o lado oposto.
                if self.role_client {
                    self.send(Message::PeerSide {
                        side: side_str(self.cfg.peer_side).to_string(),
                    });
                }
                if let Some(fp) = self.peer_fp {
                    let _ = self.ui.send(UiEvent::PeerHello {
                        name: device_id,
                        fingerprint: fp.iter().map(|b| format!("{b:02x}")).collect(),
                        trusted: self.trusted.contains(&fp),
                        remembered: std::mem::take(&mut self.remember_pending),
                    });
                }
                self.p.performance(true);
                self.notice("Conectado ao outro PC.");
                self.emit_status();
            }
            Message::PeerSide { side } => {
                if let Some(side) = Side::parse(&side) {
                    // Só age se mudou: reaplicar o mesmo lado travaria a borda sem necessidade.
                    if self.fsm != State::Remote
                        && !self.controlled
                        && side.opposite() != self.cfg.peer_side
                    {
                        self.apply_peer_side(side.opposite());
                    }
                }
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
            Message::FileOffer {
                id,
                name,
                rel,
                size,
                index,
                count,
                batch_bytes,
                root_bytes,
                root_last,
                ratio,
            } => self.on_file_offer(
                id,
                name,
                rel,
                size,
                index,
                count,
                batch_bytes,
                root_bytes,
                root_last,
                ratio,
            ),
            Message::FileChunk { id, offset, data } => self.on_file_chunk(id, offset, &data),
            Message::FileAck { id, offset } => self.on_file_ack(id, offset),
            Message::FileCancel { id } => self.on_file_cancel(id),
            // Heartbeat é tratado pela sessão; o clipboard é de uma fase futura.
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
        // Arrastando arquivos (botão pressionado): encostar na borda não devolve o controle,
        // senão o anel de 2 s nunca completaria.
        if self.p.primary_button_down() {
            self.ret.reset();
        } else if self.ret.update(target, screen, dx, dy) {
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

    // ---- arquivos --------------------------------------------------------

    fn emit_file(
        &self,
        dir: &'static str,
        state: &'static str,
        f: (u64, &str, u64, u32),
        b: &InBatchView,
        path: Option<String>,
    ) {
        let s = self.p.screen();
        let _ = self.ui.send(UiEvent::File(FileEvent {
            dir,
            state,
            id: f.0,
            name: f.1.to_string(),
            size: f.2,
            index: f.3,
            count: b.count,
            batch_done: b.done,
            batch_bytes: b.bytes,
            ratio: b.ratio,
            side: side_str(self.cfg.peer_side),
            screen: [s.left, s.top, s.right, s.bottom],
            root: b.root.clone(),
            root_path: (state == "done")
                .then(|| {
                    b.root_dir
                        .as_ref()
                        .map(|d| d.to_string_lossy().into_owned())
                        .or_else(|| path.clone())
                })
                .flatten(),
            root_bytes: b.root_bytes,
            root_last: state == "done" && b.root_last,
            is_dir: b.is_dir,
            path,
        }));
    }

    /// Cancela tudo o que está em andamento. `tell_peer`: avisa o outro PC.
    fn abort_transfers(&mut self, tell_peer: bool) {
        self.abort_with(tell_peer, "failed");
    }

    /// O usuário cancelou: para tudo nos dois PCs.
    fn cancel_transfers(&mut self) {
        if self.outgoing.is_none() && self.incoming.is_none() {
            return;
        }
        self.abort_with(true, "cancelled");
        self.notice("Transferência cancelada.");
    }

    fn abort_with(&mut self, tell_peer: bool, state: &'static str) {
        if let Some(o) = self.outgoing.take() {
            if tell_peer {
                self.send(Message::FileCancel { id: o.id });
            }
            if let Some(v) = self.out_view(&o, 0) {
                self.emit_file("out", state, (o.id, &o.name, o.size, v.1), &v.0, None);
            }
        }
        self.out_batch = None;
        if let Some(i) = self.incoming.take() {
            if tell_peer {
                self.send(Message::FileCancel { id: i.id });
            }
            if let Some(b) = &self.in_batch {
                let view = b.view();
                self.emit_file("in", state, (i.id, &i.name, i.size, b.index), &view, None);
            }
        }
        self.in_batch = None;
        self.in_roots.clear();
    }

    /// Números do lote em envio para um [`FileEvent`]: (visão, índice do arquivo atual).
    fn out_view(&self, o: &Outgoing, extra: u64) -> Option<(InBatchView, u32)> {
        let b = self.out_batch.as_ref()?;
        let root = o.rel.split('/').next().unwrap_or(&o.name).to_string();
        Some((
            InBatchView {
                count: b.count,
                bytes: b.bytes,
                done: b.done + extra,
                ratio: b.ratio,
                root,
                root_bytes: o.root_bytes,
                root_last: o.root_last,
                root_dir: None,
                is_dir: o.rel.contains('/'),
            },
            b.next_index.saturating_sub(1),
        ))
    }

    fn emit_out(&self, state: &'static str, extra: u64) {
        if let Some(o) = &self.outgoing {
            if let Some(v) = self.out_view(o, extra) {
                self.emit_file("out", state, (o.id, &o.name, o.size, v.1), &v.0, None);
            }
        }
    }

    fn start_send(&mut self, paths: Vec<PathBuf>, ratio: u16) {
        if !self.link_ready() {
            self.notice("Conecte os dois PCs antes de enviar arquivos.");
            return;
        }
        if self.out_batch.is_some() {
            self.notice("Já existe um envio em andamento.");
            return;
        }
        let plan = plan(&paths);
        if plan.skipped > 0 {
            self.notice(format!(
                "{} item(ns) ignorado(s): atalhos, links e nomes inválidos não são enviados.",
                plan.skipped
            ));
        }
        if plan.empty_folders > 0 {
            self.notice("Pasta vazia: não há arquivos para enviar.");
        }
        let mut queue = VecDeque::new();
        for e in plan.entries.iter().take(MAX_FILES_PER_BATCH as usize) {
            self.out_id += 1;
            queue.push_back(Outgoing::from_entry(self.out_id, e));
        }
        if queue.is_empty() {
            return;
        }
        self.out_batch = Some(OutBatch {
            count: queue.len() as u32,
            bytes: queue.iter().map(|o| o.size).sum(),
            queue,
            done: 0,
            ratio: ratio.min(protocol::MAX_RATIO),
            next_index: 0,
        });
        self.next_outgoing();
    }

    /// Anuncia o próximo arquivo do lote, ou encerra o lote.
    fn next_outgoing(&mut self) {
        let Some(b) = self.out_batch.as_mut() else {
            return;
        };
        let Some(o) = b.queue.pop_front() else {
            self.out_batch = None;
            return;
        };
        let index = b.next_index;
        b.next_index += 1;
        let offer = Message::FileOffer {
            id: o.id,
            name: o.name.clone(),
            rel: o.rel.clone(),
            size: o.size,
            index,
            count: b.count,
            batch_bytes: b.bytes,
            root_bytes: o.root_bytes,
            root_last: o.root_last,
            ratio: b.ratio,
        };
        self.outgoing = Some(o);
        self.emit_out("offer", 0);
        if !self.send(offer) {
            self.abort_transfers(false);
            self.notice("Não foi possível iniciar o envio do arquivo.");
            return;
        }
        self.pump_out();
    }

    /// Envia chunks enquanto a janela permitir; avança para o próximo arquivo ao concluir.
    fn pump_out(&mut self) {
        loop {
            let Some(o) = self.outgoing.as_mut() else {
                return;
            };
            if o.is_done() {
                // Arquivo vazio: o receptor já o concluiu ao receber a oferta.
                self.finish_outgoing();
                continue;
            }
            match o.next_chunk() {
                Ok(Some((offset, data))) => {
                    let m = Message::FileChunk {
                        id: o.id,
                        offset,
                        data: base64_encode(&data),
                    };
                    if !self.send(m) {
                        self.abort_transfers(false);
                        self.notice("Envio interrompido: fila de saída cheia.");
                        return;
                    }
                }
                Ok(None) => return,
                Err(e) => {
                    self.abort_transfers(true);
                    self.notice(format!("Falha ao ler o arquivo: {e}"));
                    return;
                }
            }
        }
    }

    fn finish_outgoing(&mut self) {
        if let Some(o) = self.outgoing.as_ref() {
            let size = o.size;
            self.emit_out("done", size);
        }
        if let Some(o) = self.outgoing.take() {
            if let Some(b) = self.out_batch.as_mut() {
                b.done += o.size;
                if b.queue.is_empty() {
                    self.notice(format!("Enviado: {}", o.name));
                }
            }
        }
        self.next_outgoing();
    }

    fn on_file_ack(&mut self, id: u64, offset: u64) {
        let Some(o) = self.outgoing.as_mut().filter(|o| o.id == id) else {
            return;
        };
        o.ack(offset);
        let acked = o.acked();
        if o.is_done() {
            self.finish_outgoing();
        } else {
            if self.last_out_progress.elapsed() >= PROGRESS_INTERVAL {
                self.last_out_progress = Instant::now();
                self.emit_out("progress", acked);
            }
            self.pump_out();
        }
    }

    fn on_file_cancel(&mut self, id: u64) {
        if self.outgoing.as_ref().is_some_and(|o| o.id == id) {
            self.abort_with(false, "cancelled");
            self.notice("O outro PC cancelou o recebimento.");
        } else if self.incoming.as_ref().is_some_and(|i| i.id == id) {
            self.abort_with(false, "cancelled");
            self.notice("O outro PC cancelou o envio.");
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn on_file_offer(
        &mut self,
        id: u64,
        name: String,
        rel: String,
        size: u64,
        index: u32,
        count: u32,
        batch_bytes: u64,
        root_bytes: u64,
        root_last: bool,
        ratio: u16,
    ) {
        // Só aceita arquivos com a sessão pronta (o Hello do par já passou).
        if !self.link_ready() {
            return;
        }
        if index == 0 {
            self.in_roots.clear();
        }
        // Oferta nova: descarta o recebimento anterior. Um lote novo (índice 0) recomeça a
        // contagem; no meio de um lote, mantém o que já chegou.
        let prev_done = if index == 0 {
            0
        } else {
            self.in_batch.as_ref().map_or(0, |b| b.done)
        };
        self.incoming = None;
        // Destino: Downloads (arquivo avulso) ou dentro da pasta raiz recriada, com as
        // subpastas do caminho relativo (a pasta raiz ganha "(1)" se o nome já existe).
        let parts: Vec<&str> = rel.split('/').collect();
        let downloads = self.cfg.download_dir.clone();
        let (dir, root_dir) = if parts.len() <= 1 {
            (downloads, None)
        } else {
            let root = parts[0].to_string();
            let actual = match self.in_roots.get(&root) {
                Some(p) => Ok(p.clone()),
                None => create_unique_dir(&downloads, &root).inspect(|p| {
                    self.in_roots.insert(root.clone(), p.clone());
                }),
            };
            let made = actual.and_then(|actual| {
                let mut d = actual.clone();
                for c in &parts[1..parts.len() - 1] {
                    d.push(disk_name(c));
                }
                std::fs::create_dir_all(&d).map(|()| (d, Some(actual)))
            });
            match made {
                Ok(x) => x,
                Err(e) => {
                    self.send(Message::FileCancel { id });
                    self.in_batch = None;
                    self.notice(format!("Não foi possível criar a pasta de \"{rel}\": {e}"));
                    return;
                }
            }
        };
        let inc = match Incoming::create(&dir, id, &name, size) {
            Ok(i) => i,
            Err(e) => {
                self.send(Message::FileCancel { id });
                self.in_batch = None;
                self.notice(format!("Não foi possível receber \"{name}\": {e}"));
                return;
            }
        };
        let root_name = match &root_dir {
            Some(d) => d.file_name().map_or_else(
                || parts[0].to_string(),
                |n| n.to_string_lossy().into_owned(),
            ),
            None => inc.name.clone(),
        };
        let batch = InBatch {
            count,
            bytes: batch_bytes,
            done: prev_done,
            ratio,
            index,
            root: root_name,
            root_bytes,
            root_last,
            root_dir,
        };
        self.last_acked = 0;
        self.last_progress = Instant::now();
        self.emit_file(
            "in",
            "offer",
            (id, &inc.name, size, index),
            &batch.view(),
            None,
        );
        self.in_batch = Some(batch);
        self.incoming = Some(inc);
        if size == 0 {
            self.complete_incoming();
        }
    }

    fn on_file_chunk(&mut self, id: u64, offset: u64, data: &str) {
        let Some(bytes) = base64_decode(data) else {
            return;
        };
        let Some(inc) = self.incoming.as_mut().filter(|i| i.id == id) else {
            return;
        };
        match inc.write(offset, &bytes) {
            Ok(received) => {
                if inc.is_complete() {
                    self.complete_incoming();
                    return;
                }
                if received - self.last_acked >= ACK_EVERY {
                    self.last_acked = received;
                    self.send(Message::FileAck {
                        id,
                        offset: received,
                    });
                }
                if self.last_progress.elapsed() >= PROGRESS_INTERVAL {
                    self.last_progress = Instant::now();
                    if let (Some(inc), Some(b)) = (&self.incoming, &self.in_batch) {
                        let mut view = b.view();
                        view.done += inc.received();
                        self.emit_file(
                            "in",
                            "progress",
                            (inc.id, &inc.name, inc.size, b.index),
                            &view,
                            None,
                        );
                    }
                }
            }
            Err(e) => {
                self.abort_transfers(true);
                self.notice(format!("Falha ao gravar o arquivo recebido: {e:?}"));
            }
        }
    }

    /// Fecha o arquivo recebido (renomeia, avisa o par e a interface).
    fn complete_incoming(&mut self) {
        let Some(inc) = self.incoming.take() else {
            return;
        };
        let (id, name, size) = (inc.id, inc.name.clone(), inc.size);
        match inc.finish() {
            Ok(path) => {
                self.send(Message::FileAck { id, offset: size });
                if let Some(b) = self.in_batch.as_mut() {
                    b.done += size;
                }
                if let Some(b) = &self.in_batch {
                    self.emit_file(
                        "in",
                        "done",
                        (id, &name, size, b.index),
                        &b.view(),
                        Some(path.to_string_lossy().into_owned()),
                    );
                }
            }
            Err(e) => {
                self.send(Message::FileCancel { id });
                self.in_batch = None;
                self.notice(format!("Não foi possível salvar \"{name}\": {e}"));
            }
        }
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
        // Botão esquerdo pressionado = arrastando algo (arquivos para a borda de envio): não
        // cruza; ao soltar, é preciso se afastar da borda antes de cruzar de novo.
        if self.p.primary_button_down() {
            if self.fsm == State::Armed {
                self.disarm();
                self.emit_status();
            } else {
                self.edge.block();
            }
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
