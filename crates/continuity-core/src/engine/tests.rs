//! Testes do motor com plataforma falsa sobre TCP+TLS reais em loopback.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::time::{sleep, timeout};
use windows_input::{ButtonState, InputEvent, Platform, Point, Rect, Side};

use super::*;

const SCREEN: Rect = Rect {
    left: 0,
    top: 0,
    right: 1920,
    bottom: 1080,
};

#[derive(Default)]
struct Fake {
    cursor: Option<Point>,
    moves: Vec<Point>,
    injected: Vec<InputEvent>,
    capturing: bool,
    blocked: bool,
    release_calls: u32,
    button_down: bool,
}

#[derive(Clone, Default)]
struct FakePlatform(Arc<Mutex<Fake>>);

impl FakePlatform {
    fn set(&self, p: Point) {
        self.0.lock().unwrap().cursor = Some(p);
    }
    fn cursor_now(&self) -> Point {
        self.0
            .lock()
            .unwrap()
            .cursor
            .unwrap_or(Point { x: 500, y: 500 })
    }
    fn blocked(&self) -> bool {
        self.0.lock().unwrap().blocked
    }
    fn capturing(&self) -> bool {
        self.0.lock().unwrap().capturing
    }
}

impl Platform for FakePlatform {
    fn screen(&self) -> Rect {
        SCREEN
    }
    fn cursor(&self) -> Point {
        self.cursor_now()
    }
    fn set_cursor(&mut self, p: Point) {
        self.set(p);
    }
    fn move_cursor(&mut self, p: Point) {
        let mut f = self.0.lock().unwrap();
        f.cursor = Some(p);
        f.moves.push(p);
    }
    fn inject(&mut self, ev: &InputEvent) {
        self.0.lock().unwrap().injected.push(*ev);
    }
    fn release_all(&mut self) {
        self.0.lock().unwrap().release_calls += 1;
    }
    fn capture(&mut self, on: bool) {
        self.0.lock().unwrap().capturing = on;
    }
    fn block_local(&mut self, on: bool) {
        self.0.lock().unwrap().blocked = on;
    }
    fn primary_button_down(&self) -> bool {
        self.0.lock().unwrap().button_down
    }
    fn keepalive(&self) {}
}

async fn eventually(what: &str, f: impl Fn() -> bool) {
    for _ in 0..400 {
        if f() {
            return;
        }
        sleep(Duration::from_millis(10)).await;
    }
    panic!("tempo esgotado: {what}");
}

static NODE_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

struct Node {
    fake: FakePlatform,
    input: UnboundedSender<InputEvent>,
    handle: EngineHandle,
    ui: UnboundedReceiver<UiEvent>,
    last: Option<Status>,
    sas: Option<String>,
    portals: Vec<(&'static str, u16, bool)>,
    files: Vec<FileEvent>,
    hellos: Vec<(String, bool)>,
    downloads: std::path::PathBuf,
}

impl Node {
    fn new(name: &str, side: Side) -> Self {
        let fake = FakePlatform::default();
        let (input, input_rx) = mpsc::unbounded_channel();
        let (ui_tx, ui) = mpsc::unbounded_channel();
        let mut cfg = EngineConfig::new(name, side);
        let downloads = std::env::temp_dir().join(format!(
            "wf-engine-{name}-{}-{}",
            std::process::id(),
            NODE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&downloads);
        cfg.download_dir = downloads.clone();
        let (engine, handle) = Engine::new(fake.clone(), input_rx, cfg, ui_tx).unwrap();
        tokio::spawn(engine.run());
        Node {
            fake,
            input,
            handle,
            ui,
            last: None,
            sas: None,
            portals: vec![],
            files: vec![],
            hellos: vec![],
            downloads,
        }
    }

    async fn pump(&mut self, what: &str) {
        match timeout(Duration::from_secs(5), self.ui.recv()).await {
            Ok(Some(UiEvent::Status(s))) => self.last = Some(s),
            Ok(Some(UiEvent::Confirm { sas, .. })) => self.sas = Some(sas),
            Ok(Some(UiEvent::Portal {
                side, ratio, enter, ..
            })) => self.portals.push((side, ratio, enter)),
            Ok(Some(UiEvent::File(f))) => self.files.push(f),
            Ok(Some(UiEvent::PeerHello {
                name, remembered, ..
            })) => self.hellos.push((name, remembered)),
            Ok(Some(UiEvent::Notice(_))) => {}
            _ => panic!("tempo esgotado: {what}; último status: {:?}", self.last),
        }
    }

    async fn wait_status(&mut self, what: &str, pred: impl Fn(&Status) -> bool) -> Status {
        loop {
            if let Some(s) = &self.last {
                if pred(s) {
                    return s.clone();
                }
            }
            self.pump(what).await;
        }
    }

    async fn wait_sas(&mut self) -> String {
        while self.sas.is_none() {
            self.pump("código de verificação").await;
        }
        self.sas.take().unwrap()
    }
}

fn confirm(accept: bool) -> Command {
    Command::Confirm {
        accept,
        remember: false,
    }
}

async fn connected_pair() -> (Node, Node) {
    let mut a = Node::new("pc-a", Side::Right);
    let mut b = Node::new("pc-b", Side::Left);
    a.handle.send(Command::Host("127.0.0.1:0".parse().unwrap()));
    let listening = a.wait_status("escutando", |s| s.listening.is_some()).await;
    b.handle.send(Command::Connect(
        listening.listening.unwrap().parse().unwrap(),
    ));

    let (sas_a, sas_b) = (a.wait_sas().await, b.wait_sas().await);
    assert_eq!(sas_a, sas_b, "os dois PCs devem exibir o mesmo código");
    a.handle.send(confirm(true));
    b.handle.send(confirm(true));
    a.wait_status("A conectado", |s| s.link == "connected")
        .await;
    b.wait_status("B conectado", |s| s.link == "connected")
        .await;
    (a, b)
}

/// A encosta na borda direita; retorna quando A está em REMOTE e B controlado.
async fn cross(a: &mut Node, b: &mut Node) {
    a.fake.set(Point { x: 1919, y: 540 });
    a.wait_status("A em REMOTE", |s| s.state == "remote").await;
    b.wait_status("B controlado", |s| s.controlled).await;
}

#[tokio::test]
async fn pairing_shows_same_code_and_connects() {
    let (a, b) = connected_pair().await;
    assert_eq!(a.last.as_ref().unwrap().state, "local");
    assert_eq!(b.last.as_ref().unwrap().state, "local");
}

#[tokio::test]
async fn rejecting_the_code_does_not_connect() {
    let mut a = Node::new("pc-a", Side::Right);
    let mut b = Node::new("pc-b", Side::Left);
    a.handle.send(Command::Host("127.0.0.1:0".parse().unwrap()));
    let l = a.wait_status("escutando", |s| s.listening.is_some()).await;
    b.handle
        .send(Command::Connect(l.listening.unwrap().parse().unwrap()));
    a.wait_sas().await;
    b.wait_sas().await;
    a.handle.send(confirm(false));
    b.handle.send(confirm(true));
    // B inicia a sessão, mas A nunca a inicia: B não chega a "connected".
    sleep(Duration::from_millis(500)).await;
    while let Ok(ev) = a.ui.try_recv() {
        if let UiEvent::Status(s) = ev {
            assert_ne!(s.link, "connected");
        }
    }
}

#[tokio::test]
async fn cursor_crosses_edge_moves_remote_and_returns() {
    let (mut a, mut b) = connected_pair().await;
    cross(&mut a, &mut b).await;
    assert!(a.fake.capturing(), "A captura entrada em REMOTE");
    assert_eq!(
        b.fake.cursor_now().x,
        2,
        "B recebe o cursor na borda esquerda"
    );
    assert!((b.fake.cursor_now().y - 540).abs() <= 1);

    // Movimento: deltas de A viram posições em B.
    let start = b.fake.cursor_now();
    a.input.send(InputEvent::Move { dx: 10, dy: 5 }).unwrap();
    eventually("movimento aplicado em B", || {
        b.fake.cursor_now()
            == Point {
                x: start.x + 10,
                y: start.y + 5,
            }
    })
    .await;

    // Teclas e botões chegam em ordem.
    let key = |state| InputEvent::Key {
        scan_code: 0x1E,
        extended: false,
        state,
    };
    a.input.send(key(ButtonState::Down)).unwrap();
    a.input.send(key(ButtonState::Up)).unwrap();
    eventually("teclas injetadas em B", || {
        b.fake.0.lock().unwrap().injected == vec![key(ButtonState::Down), key(ButtonState::Up)]
    })
    .await;

    // Empurrar contra a borda esquerda de B devolve o controle a A.
    a.input.send(InputEvent::Move { dx: -60, dy: 0 }).unwrap();
    a.wait_status("A volta a LOCAL", |s| s.state == "local")
        .await;
    b.wait_status("B não controlado", |s| !s.controlled).await;
    assert!(!a.fake.capturing(), "A para de capturar ao devolver");
    assert_eq!(a.fake.cursor_now().x, 1919 - EXIT_INSET);
    assert!(
        b.fake.0.lock().unwrap().release_calls >= 1,
        "B solta o que injetou"
    );
}

#[tokio::test]
async fn emergency_hotkey_recovers_without_network() {
    let (mut a, mut b) = connected_pair().await;
    cross(&mut a, &mut b).await;
    a.input.send(InputEvent::Emergency).unwrap();
    a.wait_status("A volta a LOCAL", |s| s.state == "local")
        .await;
    assert!(!a.fake.capturing());
    // Avisa o par, que também libera o controle.
    b.wait_status("B liberado", |s| !s.controlled).await;
}

#[tokio::test]
async fn emergency_works_even_when_peer_is_gone() {
    let (mut a, mut b) = connected_pair().await;
    cross(&mut a, &mut b).await;
    b.handle.send(Command::Shutdown);
    let t0 = Instant::now();
    a.input.send(InputEvent::Emergency).unwrap();
    a.wait_status("A volta a LOCAL", |s| s.state == "local")
        .await;
    assert!(!a.fake.capturing());
    assert!(t0.elapsed() < Duration::from_secs(1));
}

#[tokio::test]
async fn peer_disconnect_returns_local_control_within_two_seconds() {
    let (mut a, mut b) = connected_pair().await;
    cross(&mut a, &mut b).await;
    let t0 = Instant::now();
    b.handle.send(Command::Shutdown);
    a.wait_status("A volta a LOCAL", |s| s.state == "local")
        .await;
    assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
    assert!(!a.fake.capturing());
}

#[tokio::test]
async fn user_disconnect_while_remote_releases_everything() {
    let (mut a, mut b) = connected_pair().await;
    cross(&mut a, &mut b).await;
    a.handle.send(Command::Disconnect);
    a.wait_status("A LOCAL e ocioso", |s| {
        s.state == "local" && s.link == "idle"
    })
    .await;
    assert!(!a.fake.capturing());
    b.wait_status("B liberado", |s| !s.controlled).await;
}

// ---- testes diretos (sem rede) das regras de segurança do receptor ----

fn bare_engine() -> (Engine<FakePlatform>, FakePlatform) {
    let fake = FakePlatform::default();
    let (_tx, rx) = mpsc::unbounded_channel();
    let (ui, _ui_rx) = mpsc::unbounded_channel();
    let (e, _h) = Engine::new(fake.clone(), rx, EngineConfig::new("x", Side::Left), ui).unwrap();
    (e, fake)
}

#[test]
fn never_injects_input_when_control_was_not_granted() {
    let (mut e, fake) = bare_engine();
    e.on_message(Message::KeyEvent {
        scan_code: 1,
        extended: false,
        state: ButtonState::Down,
        seq: 1,
    });
    e.on_message(Message::PointerMove {
        dx: 50,
        dy: 50,
        seq: 2,
    });
    e.on_message(Message::Scroll {
        dx: 0,
        dy: 120,
        seq: 3,
    });
    let f = fake.0.lock().unwrap();
    assert!(f.injected.is_empty() && f.moves.is_empty());
}

#[test]
fn replayed_or_out_of_order_sequences_are_ignored() {
    let (mut e, fake) = bare_engine();
    e.controlled = true;
    let key = |seq| Message::KeyEvent {
        scan_code: 1,
        extended: false,
        state: ButtonState::Down,
        seq,
    };
    e.on_message(key(5));
    e.on_message(key(5));
    e.on_message(key(4));
    e.on_message(key(6));
    assert_eq!(fake.0.lock().unwrap().injected.len(), 2);
}

#[test]
fn recover_is_idempotent_and_always_releases() {
    let (mut e, fake) = bare_engine();
    e.recover("teste");
    e.recover("teste");
    assert_eq!(e.fsm, State::Local);
    assert!(!fake.capturing());
    assert!(fake.0.lock().unwrap().release_calls >= 2);
}

#[tokio::test]
async fn can_cross_again_after_returning_and_peer_does_not_cross_back_by_itself() {
    let (mut a, mut b) = connected_pair().await;
    for round in 1..=3 {
        // A -> B
        a.fake.set(Point { x: 1500, y: 540 });
        sleep(Duration::from_millis(60)).await;
        cross(&mut a, &mut b).await;

        // B -> A: empurrar contra a borda esquerda de B.
        a.input.send(InputEvent::Move { dx: -60, dy: 0 }).unwrap();
        a.wait_status("A volta a LOCAL", |s| s.state == "local")
            .await;
        b.wait_status("B livre", |s| !s.controlled).await;

        // B fica parado na borda por onde saiu. Não pode armar nem pedir o controle sozinho.
        sleep(Duration::from_millis(300)).await;
        while let Ok(ev) = b.ui.try_recv() {
            if let UiEvent::Status(s) = ev {
                b.last = Some(s);
            }
        }
        while let Ok(ev) = a.ui.try_recv() {
            if let UiEvent::Status(s) = ev {
                a.last = Some(s);
            }
        }
        let (sa, sb) = (a.last.clone().unwrap(), b.last.clone().unwrap());
        assert_eq!(
            sb.state, "local",
            "rodada {round}: B não deve armar após devolver"
        );
        assert!(
            !sa.controlled && !sb.controlled,
            "rodada {round}: ninguém controlado"
        );
        assert_eq!(sa.state, "local", "rodada {round}");
    }
}

#[tokio::test]
async fn crossing_shows_portal_leaving_one_pc_and_entering_the_other() {
    let (mut a, mut b) = connected_pair().await;
    cross(&mut a, &mut b).await;
    sleep(Duration::from_millis(100)).await;
    for n in [&mut a, &mut b] {
        while let Ok(ev) = n.ui.try_recv() {
            if let UiEvent::Portal {
                side, ratio, enter, ..
            } = ev
            {
                n.portals.push((side, ratio, enter));
            }
        }
    }
    assert_eq!(
        a.portals,
        vec![("right", 5004, false)],
        "A: saindo pela direita"
    );
    assert_eq!(
        b.portals,
        vec![("left", 5004, true)],
        "B: entrando pela esquerda"
    );
}

#[tokio::test]
async fn controlled_pc_blocks_its_physical_input_until_released() {
    let (mut a, mut b) = connected_pair().await;
    assert!(!b.fake.blocked());
    cross(&mut a, &mut b).await;
    assert!(
        b.fake.blocked(),
        "B bloqueia mouse/teclado físicos enquanto controlado"
    );
    a.input.send(InputEvent::Move { dx: -60, dy: 0 }).unwrap();
    a.wait_status("A volta a LOCAL", |s| s.state == "local")
        .await;
    b.wait_status("B livre", |s| !s.controlled).await;
    assert!(!b.fake.blocked());
}

#[tokio::test]
async fn physical_click_on_controlled_pc_takes_control_back() {
    let (mut a, mut b) = connected_pair().await;
    cross(&mut a, &mut b).await;
    b.input.send(InputEvent::Takeover).unwrap();
    b.wait_status("B livre", |s| !s.controlled).await;
    a.wait_status("A volta a LOCAL", |s| s.state == "local")
        .await;
    assert!(!b.fake.blocked());
    assert!(!a.fake.capturing());
}

#[test]
fn takeover_without_control_is_ignored() {
    let (mut e, fake) = bare_engine();
    e.on_input(InputEvent::Takeover);
    assert!(!fake.blocked());
    assert!(!e.controlled);
}

#[tokio::test]
async fn files_cross_the_link_with_progress_and_land_in_downloads() {
    let (a, mut b) = connected_pair().await;
    let dir = std::env::temp_dir().join(format!("wf-engine-src-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 253) as u8).collect();
    let (f1, f2, f3) = (
        dir.join("grande.bin"),
        dir.join("vazio.txt"),
        dir.join("nota.txt"),
    );
    std::fs::write(&f1, &big).unwrap();
    std::fs::write(&f2, b"").unwrap();
    std::fs::write(&f3, "olá").unwrap();

    a.handle.send(Command::SendFiles {
        paths: vec![f1, f2, f3],
        ratio: 4_200,
    });
    // B recebe os três arquivos; A só observa.
    loop {
        if b.files.iter().filter(|f| f.state == "done").count() == 3 {
            break;
        }
        b.pump("arquivos concluídos em B").await;
    }
    assert_eq!(std::fs::read(b.downloads.join("grande.bin")).unwrap(), big);
    assert_eq!(std::fs::read(b.downloads.join("vazio.txt")).unwrap(), b"");
    assert_eq!(
        std::fs::read_to_string(b.downloads.join("nota.txt")).unwrap(),
        "olá"
    );

    let offers: Vec<_> = b.files.iter().filter(|f| f.state == "offer").collect();
    assert_eq!(offers.len(), 3);
    assert!(offers
        .iter()
        .all(|f| f.ratio == 4_200 && f.count == 3 && f.side == "left"));
    assert!(
        b.files.iter().any(|f| f.state == "progress"),
        "anel recebe andamento"
    );
    let last = b.files.last().unwrap();
    assert_eq!((last.batch_done, last.batch_bytes), (3_000_004, 3_000_004));
    assert!(last
        .path
        .as_deref()
        .is_some_and(|p| p.ends_with("nota.txt")));
    // Nunca sobra arquivo parcial.
    assert!(std::fs::read_dir(&b.downloads).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".wfpart")));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&b.downloads);
}

#[tokio::test]
async fn holding_the_button_at_the_edge_does_not_take_control() {
    let (mut a, mut b) = connected_pair().await;
    a.fake.0.lock().unwrap().button_down = true;
    a.fake.set(Point { x: 1919, y: 540 });
    sleep(Duration::from_millis(400)).await;
    while let Ok(ev) = a.ui.try_recv() {
        if let UiEvent::Status(s) = ev {
            assert_ne!(s.state, "remote");
        }
    }
    assert!(!a.fake.capturing());
    // Soltar o botão ainda encostado na borda também não cruza: precisa se afastar antes.
    a.fake.0.lock().unwrap().button_down = false;
    sleep(Duration::from_millis(300)).await;
    assert!(!a.fake.capturing());
    a.fake.set(Point { x: 1800, y: 540 });
    sleep(Duration::from_millis(100)).await;
    cross(&mut a, &mut b).await;
}

#[tokio::test]
async fn screens_can_be_switched_while_a_transfer_is_running() {
    let (mut a, mut b) = connected_pair().await;
    let dir = std::env::temp_dir().join(format!("wf-engine-free-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("grande.bin");
    std::fs::write(&f, vec![7u8; 24_000_000]).unwrap();
    a.handle.send(Command::SendFiles {
        paths: vec![f],
        ratio: 5_000,
    });
    // Sem o botão pressionado, encostar na borda cruza mesmo com a transferência em curso.
    cross(&mut a, &mut b).await;
    assert!(a.fake.capturing());
    loop {
        if b.files.iter().any(|f| f.state == "done") {
            break;
        }
        b.pump("transferência concluída").await;
    }
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&b.downloads);
}

#[tokio::test]
async fn folders_are_recreated_with_their_tree_and_become_one_root_item() {
    let (a, mut b) = connected_pair().await;
    let dir = std::env::temp_dir().join(format!("wf-engine-tree-{}", std::process::id()));
    let root = dir.join("Projeto");
    std::fs::create_dir_all(root.join("src/sub")).unwrap();
    std::fs::write(root.join("leia.txt"), "oi").unwrap();
    std::fs::write(root.join("src/main.rs"), vec![1u8; 100_000]).unwrap();
    std::fs::write(root.join("src/sub/vazio.txt"), b"").unwrap();
    let loose = dir.join("solto.txt");
    std::fs::write(&loose, "x").unwrap();
    // A pasta já existe no destino: a nova ganha "(1)" e nada é sobrescrito.
    std::fs::create_dir_all(b.downloads.join("Projeto")).unwrap();
    std::fs::write(b.downloads.join("Projeto/antigo.txt"), "intacto").unwrap();

    a.handle.send(Command::SendFiles {
        paths: vec![root, loose],
        ratio: 3_000,
    });
    loop {
        if b.files.iter().filter(|f| f.state == "done").count() == 4 {
            break;
        }
        b.pump("pasta concluída em B").await;
    }
    let got = b.downloads.join("Projeto (1)");
    assert_eq!(std::fs::read_to_string(got.join("leia.txt")).unwrap(), "oi");
    assert_eq!(
        std::fs::read(got.join("src/main.rs")).unwrap().len(),
        100_000
    );
    assert!(got.join("src/sub/vazio.txt").exists());
    assert_eq!(
        std::fs::read_to_string(b.downloads.join("Projeto/antigo.txt")).unwrap(),
        "intacto"
    );

    // A bandeja recebe um item por raiz: a pasta (uma vez) e o arquivo avulso.
    let roots: Vec<_> = b
        .files
        .iter()
        .filter(|f| f.state == "done" && f.root_last)
        .collect();
    assert_eq!(roots.len(), 2);
    assert_eq!(roots[0].root, "Projeto (1)");
    assert!(roots[0].is_dir && roots[0].root_bytes == 100_002);
    assert_eq!(
        roots[0].root_path.as_deref(),
        Some(got.to_string_lossy().as_ref())
    );
    assert_eq!(roots[1].root, "solto.txt");
    assert!(!roots[1].is_dir);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&b.downloads);
}

#[tokio::test]
async fn controlled_pc_keeps_control_while_dragging_at_its_edge() {
    let (mut a, mut b) = connected_pair().await;
    cross(&mut a, &mut b).await;
    // Com o botão pressionado em B (arrastando arquivo), empurrar contra a borda não devolve.
    b.fake.0.lock().unwrap().button_down = true;
    a.input.send(InputEvent::Move { dx: -80, dy: 0 }).unwrap();
    sleep(Duration::from_millis(300)).await;
    assert!(a.fake.capturing(), "A continua controlando B");
    // Soltou: um novo empurrão devolve o controle normalmente.
    b.fake.0.lock().unwrap().button_down = false;
    a.input.send(InputEvent::Move { dx: -60, dy: 0 }).unwrap();
    a.wait_status("A volta a LOCAL", |s| s.state == "local")
        .await;
}

#[tokio::test]
async fn remembered_computer_reconnects_without_asking_for_the_code_again() {
    let mut a = Node::new("pc-a", Side::Right);
    let mut b = Node::new("pc-b", Side::Left);
    a.handle.send(Command::Host("127.0.0.1:0".parse().unwrap()));
    let listening = a.wait_status("escutando", |s| s.listening.is_some()).await;
    let addr: SocketAddr = listening.listening.unwrap().parse().unwrap();
    b.handle.send(Command::Connect(addr));
    a.wait_sas().await;
    b.wait_sas().await;
    let remember = |accept| Command::Confirm {
        accept,
        remember: true,
    };
    a.handle.send(remember(true));
    b.handle.send(remember(true));
    a.wait_status("A conectado", |s| s.link == "connected")
        .await;
    b.wait_status("B conectado", |s| s.link == "connected")
        .await;
    // Cada lado soube o nome do outro e que acabou de lembrá-lo.
    while a.hellos.is_empty() {
        a.pump("hello de A").await;
    }
    while b.hellos.is_empty() {
        b.pump("hello de B").await;
    }
    assert_eq!(a.hellos[0], ("pc-b".to_string(), true));
    assert_eq!(b.hellos[0], ("pc-a".to_string(), true));

    // Desconectar e conectar de novo: nenhum código é pedido.
    b.handle.send(Command::Disconnect);
    b.wait_status("B ocioso", |s| s.link == "idle").await;
    a.sas = None;
    b.sas = None;
    b.handle.send(Command::Connect(addr));
    a.wait_status("A reconectado", |s| {
        s.link == "connected" && s.peer.is_some()
    })
    .await;
    b.wait_status("B reconectado", |s| s.link == "connected")
        .await;
    assert!(
        a.sas.is_none() && b.sas.is_none(),
        "não deve pedir o código"
    );
}
