use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use continuity_core::engine::{Command, Engine, EngineConfig, EngineHandle, Status, UiEvent};
use serde::Serialize;
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, RunEvent, State, WebviewUrl,
    WebviewWindowBuilder, WindowEvent,
};
use tokio::sync::mpsc;
use windows_input::windows_impl::WinPlatform;
use windows_input::Side;

mod xfer;

const DEFAULT_PORT: u16 = 47_800;
/// Espessura (px físicos) da faixa do overlay de portal junto à borda.
const PORTAL_DEPTH: i32 = 440;
/// Comprimento (px físicos) da faixa ao longo da borda, centrada no ponto de contato.
const PORTAL_SPAN: i32 = 1_000;
const PORTAL_VISIBLE_MS: u64 = 1_700;

struct App {
    engine: EngineHandle,
    status: Mutex<Option<Status>>,
    hotkey_registered: bool,
    portal_enabled: AtomicBool,
    portal_gen: AtomicU64,
    snapshot: Mutex<Vec<u8>>,
    /// Último estado informado pelo overlay (WebGL ok, plano B, erro de shader...).
    portal_health: Mutex<String>,
    xfer: Mutex<xfer::Xfer>,
}

#[derive(Serialize, Clone)]
struct PortalPayload {
    side: String,
    enter: bool,
    w: i32,
    h: i32,
    /// Posição do ponto de contato ao longo da borda, em px dentro da janela.
    v0: i32,
}

#[derive(Serialize, Clone)]
struct ConfirmPayload {
    sas: String,
    peer: String,
}

#[derive(Serialize)]
struct Info {
    device_name: String,
    local_ip: Option<String>,
    default_port: u16,
    hotkey_registered: bool,
    portal_health: String,
}

#[tauri::command]
fn get_info(app: State<'_, App>) -> Info {
    Info {
        device_name: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "este-pc".into()),
        local_ip: local_ip().map(|ip| ip.to_string()),
        default_port: DEFAULT_PORT,
        hotkey_registered: app.hotkey_registered,
        portal_health: app
            .portal_health
            .lock()
            .map(|h| h.clone())
            .unwrap_or_default(),
    }
}

#[tauri::command]
fn get_status(app: State<'_, App>) -> Option<Status> {
    app.status.lock().ok().and_then(|s| s.clone())
}

#[tauri::command]
fn host(app: State<'_, App>, port: Option<u16>) {
    let addr = SocketAddr::new(
        IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        port.unwrap_or(DEFAULT_PORT),
    );
    app.engine.send(Command::Host(addr));
}

#[tauri::command]
fn connect(app: State<'_, App>, addr: String) -> Result<(), String> {
    let addr = parse_peer(&addr)?;
    app.engine.send(Command::Connect(addr));
    Ok(())
}

#[tauri::command]
fn confirm(app: State<'_, App>, accept: bool) {
    app.engine.send(Command::Confirm(accept));
}

#[tauri::command]
fn disconnect(app: State<'_, App>) {
    app.engine.send(Command::Disconnect);
}

#[tauri::command]
fn set_peer_side(app: State<'_, App>, side: String) -> Result<(), String> {
    let side = Side::parse(&side).ok_or("lado inválido")?;
    app.engine.send(Command::SetPeerSide(side));
    Ok(())
}

/// Aceita "192.168.0.10" (porta padrão) ou "192.168.0.10:47800".
fn parse_peer(s: &str) -> Result<SocketAddr, String> {
    let s = s.trim();
    if let Ok(a) = s.parse::<SocketAddr>() {
        return Ok(a);
    }
    s.parse::<IpAddr>()
        .map(|ip| SocketAddr::new(ip, DEFAULT_PORT))
        .map_err(|_| "Endereço inválido. Use IP ou IP:porta, ex.: 192.168.0.10".to_string())
}

/// IP local da interface de saída (UDP "conectado" não envia nenhum pacote).
fn local_ip() -> Option<IpAddr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.168.255.255:9").ok()?;
    Some(sock.local_addr().ok()?.ip())
}

fn forward_events(app: AppHandle, mut rx: mpsc::UnboundedReceiver<UiEvent>) {
    tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                UiEvent::Status(s) => {
                    if let Some(state) = app.try_state::<App>() {
                        if let Ok(mut g) = state.status.lock() {
                            *g = Some(s.clone());
                        }
                    }
                    let _ = app.emit("engine-status", s);
                }
                UiEvent::Confirm { sas, peer } => {
                    let _ = app.emit("engine-confirm", ConfirmPayload { sas, peer });
                    show_main_window(&app);
                }
                UiEvent::Notice(msg) => {
                    let _ = app.emit("engine-notice", msg);
                }
                UiEvent::Portal {
                    side,
                    ratio,
                    enter,
                    screen,
                } => show_portal(&app, side, ratio, enter, screen),
                UiEvent::File(f) => xfer::on_file_event(&app, f),
            }
        }
    });
}

/// Mostra a animação de transição: captura (só em memória) a faixa da tela junto à borda,
/// abre uma janela transparente e "click-through" sobre ela e o frontend deforma esses
/// pixels com um shader. Nunca rouba foco nem bloqueia o mouse.
fn show_portal(app: &AppHandle, side: &str, ratio: u16, enter: bool, s: [i32; 4]) {
    let Some(state) = app.try_state::<App>() else {
        return;
    };
    if !state.portal_enabled.load(Ordering::Relaxed) {
        return;
    }
    let Some(win) = app.get_webview_window("portal") else {
        return;
    };
    let (l, t, r, b) = (s[0], s[1], s[2], s[3]);
    let (sw, sh) = (r - l, b - t);
    let depth = PORTAL_DEPTH.min(if matches!(side, "left" | "right") {
        sw
    } else {
        sh
    });
    let horizontal_edge = matches!(side, "top" | "bottom");
    let (len, origin) = if horizontal_edge { (sw, l) } else { (sh, t) };
    let span = PORTAL_SPAN.min(len);
    // Centro do contato ao longo da borda e janela de `span` px ao redor dele.
    let center = origin + (i64::from(ratio) * i64::from(len) / 10_000) as i32;
    let start = (center - span / 2).clamp(origin, origin + len - span);
    let v0 = center - start;
    let (x, y, w, h) = match side {
        "left" => (l, start, depth, span),
        "right" => (r - depth, start, depth, span),
        "top" => (start, t, span, depth),
        _ => (start, b - depth, span, depth),
    };

    // Se uma animação anterior ainda está na tela, some com ela antes de capturar para não
    // fotografar o efeito antigo.
    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
        std::thread::sleep(Duration::from_millis(40));
    }
    // Captura antes de mostrar a janela, para não fotografar o próprio overlay.
    let pixels = windows_input::windows_impl::capture_screen_rgba(x, y, w, h).unwrap_or_default();
    if let Ok(mut snap) = state.snapshot.lock() {
        *snap = pixels;
    }

    let placed = place_overlay(&win, x, y, w, h);
    qa_log(&format!(
        "portal side={side} enter={enter} alvo=({x},{y} {w}x{h}) posicionado={placed} real={:?} {:?}",
        win.outer_position().ok(),
        win.outer_size().ok()
    ));
    let _ = win.show();
    let payload = PortalPayload {
        side: side.to_string(),
        enter,
        w,
        h,
        v0,
    };
    let _ = app.emit_to("portal", "portal", payload);

    let gen = state.portal_gen.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(PORTAL_VISIBLE_MS)).await;
        if let Some(st) = app.try_state::<App>() {
            if st.portal_gen.load(Ordering::SeqCst) == gen {
                if let Some(w) = app.get_webview_window("portal") {
                    let _ = w.hide();
                }
                // Descarta a captura da tela assim que a animação termina.
                if let Ok(mut snap) = st.snapshot.lock() {
                    *snap = Vec::new();
                }
            }
        }
    });
}

/// Posiciona o overlay exatamente sobre o retângulo capturado e confere o resultado.
/// Ao mudar de monitor com escala (DPI) diferente, o Windows redimensiona a janela para
/// manter o tamanho lógico; por isso movemos primeiro, dimensionamos depois e conferimos,
/// repetindo se o tamanho real não bater. Uma janela menor que a captura deixaria uma
/// borda nítida entre o trecho distorcido e a tela real.
pub(crate) fn place_overlay(win: &tauri::WebviewWindow, x: i32, y: i32, w: i32, h: i32) -> bool {
    let (w, h) = (w.max(1) as u32, h.max(1) as u32);
    for _ in 0..5 {
        let _ = win.set_position(PhysicalPosition::new(x, y));
        let _ = win.set_size(PhysicalSize::new(w, h));
        let ok = matches!(
            (win.outer_position(), win.outer_size()),
            (Ok(p), Ok(s)) if p.x == x && p.y == y && s.width == w && s.height == h
        );
        if ok {
            return true;
        }
    }
    false
}

/// Log de diagnóstico da animação em `%TEMP%\windows-flow-portal.log`: só geometria, tempos
/// e erros (nunca conteúdo de tela ou de teclado). Reinicia ao passar de 64 KB.
fn qa_log(line: &str) {
    use std::io::Write;
    let path = std::env::temp_dir().join("windows-flow-portal.log");
    let too_big = std::fs::metadata(&path).is_ok_and(|m| m.len() > 64 * 1024);
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true);
    if too_big {
        opts.write(true).truncate(true);
    } else {
        opts.append(true);
    }
    if let Ok(mut f) = opts.open(path) {
        let _ = writeln!(f, "{line}");
    }
}

/// O overlay informa o estado do renderizador e de cada animação tocada.
#[tauri::command]
fn portal_report(app: State<'_, App>, msg: String) {
    qa_log(&format!("overlay: {msg}"));
    if !msg.starts_with("play") {
        if let Ok(mut h) = app.portal_health.lock() {
            *h = msg;
        }
    }
}
/// Pixels RGBA (linhas de cima para baixo) da faixa capturada; o frontend sabe w x h pelo evento.
#[tauri::command]
fn portal_snapshot(app: State<'_, App>) -> tauri::ipc::Response {
    let bytes = app.snapshot.lock().map(|s| s.clone()).unwrap_or_default();
    tauri::ipc::Response::new(bytes)
}
#[tauri::command]
fn set_portal_enabled(app: State<'_, App>, enabled: bool) {
    app.portal_enabled.store(enabled, Ordering::Relaxed);
}

/// Mostra a animação (saída e, 0,9 s depois, entrada) numa borda do monitor principal,
/// para o usuário ver como fica. `side` vem do comando; o padrão é a direita.
fn run_preview(handle: &AppHandle, side: &'static str, virtual_screen: bool) {
    let rect = if virtual_screen {
        let Ok(monitors) = handle.available_monitors() else {
            return;
        };
        let (mut l, mut t, mut r, mut b) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for m in monitors {
            let (p, s) = (m.position(), m.size());
            l = l.min(p.x);
            t = t.min(p.y);
            r = r.max(p.x + s.width as i32);
            b = b.max(p.y + s.height as i32);
        }
        [l, t, r, b]
    } else {
        let Ok(Some(m)) = handle.primary_monitor() else {
            return;
        };
        let (p, s) = (m.position(), m.size());
        [p.x, p.y, p.x + s.width as i32, p.y + s.height as i32]
    };
    show_portal(handle, side, 5_000, false, rect);
    let h = handle.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(900)).await;
        show_portal(&h, side, 5_000, true, rect);
    });
}

#[tauri::command]
fn preview_portal(handle: AppHandle) {
    run_preview(&handle, "right", false);
}
fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Cria (oculta) a janela do overlay. Transparente, sem foco, sempre no topo e ignorando o mouse.
fn create_portal_window(app: &tauri::App) -> tauri::Result<()> {
    let win = WebviewWindowBuilder::new(
        app,
        "portal",
        WebviewUrl::App(
            if std::env::args().any(|a| a == "--force-2d") {
                "index.html#portal-2d"
            } else {
                "index.html#portal"
            }
            .into(),
        ),
    )
    .title("Windows Flow Portal")
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .shadow(false)
    .resizable(false)
    .focused(false)
    .focusable(false)
    .visible(false)
    .build()?;
    win.set_ignore_cursor_events(true)?;
    Ok(())
}

pub fn run() {
    let app = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_info,
            get_status,
            host,
            connect,
            confirm,
            disconnect,
            set_peer_side,
            set_portal_enabled,
            portal_snapshot,
            portal_report,
            preview_portal,
            xfer::xfer_layout,
            xfer::xfer_hide,
            xfer::xfer_dismiss,
            xfer::xfer_drag
        ])
        .on_window_event(|window, event| {
            // Fechar a janela mantém o app na bandeja; "Sair" encerra de verdade.
            match event {
                WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    let _ = window.hide();
                }
                WindowEvent::DragDrop(e) => xfer::on_drag_drop(window, e),
                _ => {}
            }
        })
        .setup(|app| {
            let (input_tx, input_rx) = mpsc::unbounded_channel();
            let platform = WinPlatform::new(input_tx)?;
            let hotkey_registered = platform.hotkey_registered();
            let device = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "windows-flow".into());
            let (ui_tx, ui_rx) = mpsc::unbounded_channel();
            let mut config = EngineConfig::new(device, Side::Right);
            config.download_dir = windows_input::windows_impl::downloads_dir();
            let (engine, handle) = Engine::new(platform, input_rx, config, ui_tx)?;

            app.manage(App {
                engine: handle,
                status: Mutex::new(None),
                hotkey_registered,
                portal_enabled: AtomicBool::new(true),
                portal_gen: AtomicU64::new(0),
                snapshot: Mutex::new(Vec::new()),
                portal_health: Mutex::new("aguardando o overlay iniciar".into()),
                xfer: Mutex::new(xfer::Xfer::default()),
            });
            create_portal_window(app)?;
            xfer::create_windows(app)?;
            xfer::start_watcher(app.handle().clone());
            if std::env::args().any(|a| a == "--preview-xfer") {
                let h = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    xfer::run_preview(h);
                });
            }
            // Flag de QA: mostra a animação logo após abrir, sem precisar de dois PCs.
            if let Some(arg) = std::env::args().find(|a| a.starts_with("--preview-portal")) {
                let side: &'static str = match arg.split('=').nth(1) {
                    Some("left") => "left",
                    Some("top") => "top",
                    Some("bottom") => "bottom",
                    _ => "right",
                };
                let virtual_screen = arg.contains('=');
                let h = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let delay = std::env::args()
                        .find_map(|a| {
                            a.strip_prefix("--preview-delay=")
                                .and_then(|s| s.parse::<u64>().ok())
                        })
                        .unwrap_or(2);
                    tokio::time::sleep(Duration::from_millis(500 + delay * 1_000)).await;
                    run_preview(&h, side, virtual_screen);
                });
            }
            forward_events(app.handle().clone(), ui_rx);
            tauri::async_runtime::spawn(engine.run());

            let show = MenuItem::with_id(app, "show", "Abrir Windows Flow", true, None::<&str>)?;
            let disc = MenuItem::with_id(app, "disconnect", "Desconectar", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Sair", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &disc, &quit])?;
            TrayIconBuilder::new()
                .icon(
                    app.default_window_icon()
                        .cloned()
                        .expect("ícone padrão ausente"),
                )
                .tooltip("Windows Flow")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    "disconnect" => {
                        if let Some(s) = app.try_state::<App>() {
                            s.engine.send(Command::Disconnect);
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("falha ao construir o Windows Flow");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            // Devolve o controle local antes de sair (os hooks também morrem com o processo).
            if let Some(s) = handle.try_state::<App>() {
                s.engine.send(Command::Shutdown);
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    });
}
