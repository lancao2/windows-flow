//! Transferência de arquivos pela borda da tela.
//!
//! Enviar: com o botão esquerdo pressionado perto da borda do outro PC, uma janela
//! transparente de captura aparece; arrastar arquivos até a borda e segurar 2 s completa o
//! anel e envia. Receber: um anel mostra o andamento junto à borda e vira uma bandeja de
//! onde o arquivo pode ser arrastado (nativamente, via OLE) para qualquer lugar.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use continuity_core::engine::{Command, FileEvent};
use serde::Serialize;
use tauri::{
    AppHandle, DragDropEvent, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder, Window,
};
use windows_input::windows_impl::{cursor_pos, primary_button_down, virtual_screen};
use windows_input::Point;

use super::{place_overlay, App};

pub const SEND: &str = "xfer-send";
pub const RECV: &str = "xfer-recv";

/// Tamanho lógico (px CSS) das janelas do anel e da animação.
const BUBBLE_WIN: (i32, i32) = (400, 400);
/// Distância (px físicos) da borda a partir da qual a janela de captura aparece.
const ARM_DIST: i32 = 150;
/// Distância máxima da borda para o anel contar o tempo: "encostado na extremidade".
const HOLD_DIST: i32 = 40;
const HOLD_MS: u64 = 2_000;
const TICK_MS: u64 = 30;
/// Duração da animação de envio antes de a janela poder sumir.
const COMMIT_MS: u64 = 1_700;
/// Pressão do botão por esse número de ciclos antes de mostrar a janela (ignora cliques).
const ARM_TICKS: u32 = 4;
const MAX_TRAY: usize = 8;

#[derive(Clone, Copy)]
struct Anchor {
    side: &'static str,
    /// Coordenada global, ao longo da borda, em que a janela se centraliza.
    center: i32,
    screen: [i32; 4],
    /// Início da janela ao longo da borda e escala do monitor (preenchidos ao posicionar).
    start: i32,
    scale: f64,
}

#[derive(Default)]
struct Hold {
    paths: Vec<PathBuf>,
    entered: bool,
    started: Option<Instant>,
    committed: bool,
}

struct TrayItem {
    id: u64,
    path: PathBuf,
}

#[derive(Default)]
pub struct Xfer {
    anchors: HashMap<&'static str, Anchor>,
    hold: Hold,
    tray: Vec<TrayItem>,
}

#[derive(Serialize, Clone)]
pub struct Placement {
    side: &'static str,
    /// Posição do ponto de contato ao longo da borda, em px CSS dentro da janela.
    v: i32,
    w: i32,
    h: i32,
}

#[derive(Serialize, Clone)]
struct SendPayload {
    kind: &'static str,
    /// Progresso do anel (0..1).
    p: f64,
    placement: Option<Placement>,
    /// Posição do cursor ao longo da borda, em px CSS dentro da janela.
    v: i32,
}

#[derive(Serialize, Clone)]
struct RecvPayload {
    file: FileEvent,
    placement: Option<Placement>,
}

fn edge_distance(p: Point, s: [i32; 4], side: &str) -> i32 {
    match side {
        "left" => p.x - s[0],
        "right" => s[2] - 1 - p.x,
        "top" => p.y - s[1],
        _ => s[3] - 1 - p.y,
    }
}

fn along(p: Point, side: &str) -> i32 {
    if matches!(side, "left" | "right") {
        p.y
    } else {
        p.x
    }
}

fn center_from_ratio(side: &str, ratio: u16, s: [i32; 4]) -> i32 {
    let (lo, len) = if matches!(side, "left" | "right") {
        (s[1], s[3] - s[1])
    } else {
        (s[0], s[2] - s[0])
    };
    lo + (i64::from(ratio) * i64::from((len - 1).max(0)) / 10_000) as i32
}

fn ratio_from_pos(side: &str, pos: i32, s: [i32; 4]) -> u16 {
    let (lo, len) = if matches!(side, "left" | "right") {
        (s[1], s[3] - s[1])
    } else {
        (s[0], s[2] - s[0])
    };
    let span = i64::from((len - 1).max(1));
    (i64::from(pos - lo) * 10_000 / span).clamp(0, 10_000) as u16
}

fn set_anchor(
    app: &AppHandle,
    label: &'static str,
    side: &'static str,
    center: i32,
    screen: [i32; 4],
) {
    if let Some(st) = app.try_state::<App>() {
        if let Ok(mut x) = st.xfer.lock() {
            x.anchors.insert(
                label,
                Anchor {
                    side,
                    center,
                    screen,
                    start: 0,
                    scale: 1.0,
                },
            );
        }
    }
}

/// Posiciona a janela `label` rente à borda, centrada no ponto de ancoragem, com `lw`x`lh` px
/// CSS. `interactive` falso = o mouse atravessa a janela.
fn place(app: &AppHandle, label: &str, lw: i32, lh: i32, interactive: bool) -> Option<Placement> {
    let st = app.try_state::<App>()?;
    let anchor = *st.xfer.lock().ok()?.anchors.get(label)?;
    let win = app.get_webview_window(label)?;
    let [l, t, r, b] = anchor.screen;
    let c = anchor.center;
    let (px, py) = match anchor.side {
        "left" => (l, c),
        "right" => (r - 1, c),
        "top" => (c, t),
        _ => (c, b - 1),
    };
    let scale = app
        .monitor_from_point(f64::from(px), f64::from(py))
        .ok()
        .flatten()
        .map_or(1.0, |m| m.scale_factor());
    let (w, h) = (
        (f64::from(lw) * scale).round() as i32,
        (f64::from(lh) * scale).round() as i32,
    );
    let (x, y, start) = match anchor.side {
        "left" | "right" => {
            let y = (c - h / 2).clamp(t, (b - h).max(t));
            (if anchor.side == "left" { l } else { r - w }, y, y)
        }
        _ => {
            let x = (c - w / 2).clamp(l, (r - w).max(l));
            (x, if anchor.side == "top" { t } else { b - h }, x)
        }
    };
    if let Ok(mut xf) = st.xfer.lock() {
        if let Some(a) = xf.anchors.get_mut(label) {
            a.start = start;
            a.scale = scale;
        }
    }
    let _ = win.set_ignore_cursor_events(!interactive);
    place_overlay(&win, x, y, w, h);
    let _ = win.show();
    Some(Placement {
        side: anchor.side,
        v: (f64::from(c - start) / scale).round() as i32,
        w: lw,
        h: lh,
    })
}

fn hide(app: &AppHandle, label: &str) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.hide();
        let _ = w.set_ignore_cursor_events(true);
    }
}

pub fn create_windows(app: &tauri::App) -> tauri::Result<()> {
    for (label, url, drops) in [
        (SEND, "index.html#xfer-send", true),
        (RECV, "index.html#xfer-recv", false),
    ] {
        let mut b = WebviewWindowBuilder::new(app, label, WebviewUrl::App(url.into()))
            .title("Windows Flow Transferência")
            .decorations(false)
            .transparent(true)
            .always_on_top(true)
            .skip_taskbar(true)
            .shadow(false)
            .resizable(false)
            .focused(false)
            .focusable(false)
            .visible(false);
        if !drops {
            b = b.disable_drag_drop_handler();
        }
        let win = b.build()?;
        win.set_ignore_cursor_events(true)?;
    }
    Ok(())
}

// ---- envio ----------------------------------------------------------------

/// Eventos de arrastar-e-soltar da janela de captura: guardam os arquivos que entraram nela.
pub fn on_drag_drop(window: &Window, ev: &DragDropEvent) {
    if window.label() != SEND {
        return;
    }
    let app = window.app_handle();
    let Some(st) = app.try_state::<App>() else {
        return;
    };
    let Ok(mut x) = st.xfer.lock() else {
        return;
    };
    match ev {
        DragDropEvent::Enter { paths, .. } if !paths.is_empty() => {
            x.hold = Hold {
                paths: paths.clone(),
                entered: true,
                started: None,
                committed: false,
            };
        }
        DragDropEvent::Drop { .. } | DragDropEvent::Leave => {
            // Soltar antes do anel completar (ou sair da janela) cancela o envio.
            if !x.hold.committed {
                x.hold = Hold::default();
            } else {
                x.hold.entered = false;
            }
        }
        _ => {}
    }
}

enum Phase {
    Idle,
    Catch,
    Commit(Instant),
}

pub fn start_watcher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(TICK_MS));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut phase = Phase::Idle;
        let mut armed = 0u32;
        loop {
            tick.tick().await;
            send_tick(&app, &mut phase, &mut armed);
        }
    });
}

fn emit_send(app: &AppHandle, kind: &'static str, p: f64, placement: Option<Placement>, v: i32) {
    let _ = app.emit_to(
        SEND,
        "xfer-send",
        SendPayload {
            kind,
            p,
            placement,
            v,
        },
    );
}

fn send_tick(app: &AppHandle, phase: &mut Phase, armed: &mut u32) {
    let Some(st) = app.try_state::<App>() else {
        return;
    };
    let status = st.status.lock().ok().and_then(|s| s.clone());
    let usable = status
        .as_ref()
        .is_some_and(|s| s.link == "connected" && s.state == "local" && !s.controlled);
    let side = status.as_ref().map_or("right", |s| s.peer_side);
    let down = primary_button_down();
    let cur = cursor_pos();
    let r = virtual_screen();
    let screen = [r.left, r.top, r.right, r.bottom];
    let dist = edge_distance(cur, screen, side);

    match phase {
        Phase::Idle => {
            if usable && down && dist <= ARM_DIST {
                *armed += 1;
            } else {
                *armed = 0;
            }
            if *armed >= ARM_TICKS {
                if let Ok(mut x) = st.xfer.lock() {
                    x.hold = Hold::default();
                }
                set_anchor(app, SEND, side, along(cur, side), screen);
                let pl = place(app, SEND, BUBBLE_WIN.0, BUBBLE_WIN.1, true);
                emit_send(app, "catch", 0.0, pl, 0);
                *phase = Phase::Catch;
            }
        }
        Phase::Catch => {
            let entered = st.xfer.lock().is_ok_and(|x| x.hold.entered);
            if !down || !usable || (!entered && dist > ARM_DIST * 3) {
                hide(app, SEND);
                emit_send(app, "idle", 0.0, None, 0);
                if let Ok(mut x) = st.xfer.lock() {
                    x.hold = Hold::default();
                }
                *phase = Phase::Idle;
                *armed = 0;
                return;
            }
            let mut commit: Option<(Vec<PathBuf>, u16)> = None;
            let mut outgoing: Option<(&'static str, f64, i32)> = None;
            if let Ok(mut x) = st.xfer.lock() {
                let anchor = x.anchors.get(SEND).copied();
                if x.hold.entered && !x.hold.committed {
                    if dist <= HOLD_DIST {
                        let started = *x.hold.started.get_or_insert_with(Instant::now);
                        let p =
                            (started.elapsed().as_secs_f64() * 1000.0 / HOLD_MS as f64).min(1.0);
                        let v = anchor.map_or(0, |a| {
                            (f64::from(along(cur, side) - a.start) / a.scale).round() as i32
                        });
                        if p >= 1.0 {
                            x.hold.committed = true;
                            let paths = std::mem::take(&mut x.hold.paths);
                            commit = Some((paths, ratio_from_pos(side, along(cur, side), screen)));
                            outgoing = Some(("commit", 1.0, v));
                        } else {
                            outgoing = Some(("hold", p, v));
                        }
                    } else if x.hold.started.take().is_some() {
                        outgoing = Some(("cancel", 0.0, 0));
                    }
                }
            }
            if let Some((kind, p, v)) = outgoing {
                emit_send(app, kind, p, None, v);
            }
            if let Some((paths, ratio)) = commit {
                st.engine.send(Command::SendFiles { paths, ratio });
                *phase = Phase::Commit(Instant::now() + Duration::from_millis(COMMIT_MS));
            }
        }
        Phase::Commit(until) => {
            // A janela fica de pé até soltar o botão: soltar o arquivo "no vazio" da borda
            // poderia copiá-lo para a área de trabalho.
            if Instant::now() >= *until && (!down || !usable) {
                hide(app, SEND);
                emit_send(app, "idle", 0.0, None, 0);
                if let Ok(mut x) = st.xfer.lock() {
                    x.hold = Hold::default();
                }
                *phase = Phase::Idle;
                *armed = 0;
            }
        }
    }
}

// ---- recebimento ------------------------------------------------------------

/// Andamento de um arquivo recebido: mostra o anel, atualiza e registra a bandeja.
pub fn on_file_event(app: &AppHandle, f: FileEvent) {
    if f.dir != "in" {
        return;
    }
    let Some(st) = app.try_state::<App>() else {
        return;
    };
    let mut placement = None;
    if f.state == "offer" {
        let already_visible = app
            .get_webview_window(RECV)
            .is_some_and(|w| w.is_visible().unwrap_or(false));
        if !already_visible {
            let c = center_from_ratio(f.side, f.ratio, f.screen);
            let side: &'static str = f.side;
            set_anchor(app, RECV, side, c, f.screen);
            placement = place(app, RECV, BUBBLE_WIN.0, BUBBLE_WIN.1, false);
        }
    }
    if f.state == "done" {
        if let (Some(p), Ok(mut x)) = (&f.path, st.xfer.lock()) {
            x.tray.push(TrayItem {
                id: f.id,
                path: PathBuf::from(p),
            });
            if x.tray.len() > MAX_TRAY {
                x.tray.remove(0);
            }
        }
    }
    let _ = app.emit_to(RECV, "xfer-recv", RecvPayload { file: f, placement });
}

#[tauri::command]
pub fn xfer_layout(
    app: AppHandle,
    label: String,
    w: i32,
    h: i32,
    interactive: bool,
) -> Option<Placement> {
    let label = match label.as_str() {
        SEND => SEND,
        RECV => RECV,
        _ => return None,
    };
    place(
        &app,
        label,
        w.clamp(1, 1_000),
        h.clamp(1, 1_000),
        interactive,
    )
}

#[tauri::command]
pub fn xfer_hide(app: AppHandle, label: String) {
    if label == RECV || label == SEND {
        hide(&app, &label);
    }
}

/// Fecha a bandeja sem apagar os arquivos (continuam em Downloads).
#[tauri::command]
pub fn xfer_dismiss(app: AppHandle, state: State<'_, App>) {
    if let Ok(mut x) = state.xfer.lock() {
        x.tray.clear();
    }
    hide(&app, RECV);
}

/// Começa o arrastar nativo do arquivo `id` da bandeja. Bloqueia até o usuário soltar;
/// devolve `true` se algum destino aceitou (a bandeja então remove o item).
#[tauri::command]
pub fn xfer_drag(state: State<'_, App>, id: u64) -> bool {
    let path = state
        .xfer
        .lock()
        .ok()
        .and_then(|x| x.tray.iter().find(|t| t.id == id).map(|t| t.path.clone()));
    let Some(path) = path else {
        return false;
    };
    let dropped = windows_input::windows_impl::start_file_drag(&[path]);
    if dropped {
        if let Ok(mut x) = state.xfer.lock() {
            x.tray.retain(|t| t.id != id);
        }
    }
    dropped
}

/// Flag de QA `--preview-xfer`: mostra, num só PC, o envio (borda direita, anel de 2 s e
/// animação de saída) e, 0,9 s depois, a chegada (borda esquerda, anel e bandeja com um arquivo
/// de teste arrastável).
pub fn run_preview(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let Ok(Some(m)) = app.primary_monitor() else {
            return;
        };
        let (p, s) = (m.position(), m.size());
        let screen = [p.x, p.y, p.x + s.width as i32, p.y + s.height as i32];
        let mid = (screen[1] + screen[3]) / 2;
        let sleep = |ms| tokio::time::sleep(Duration::from_millis(ms));

        set_anchor(&app, SEND, "right", mid, screen);
        let pl = place(&app, SEND, BUBBLE_WIN.0, BUBBLE_WIN.1, false);
        emit_send(&app, "catch", 0.0, pl.clone(), 0);
        let v = pl.as_ref().map_or(0, |p| p.v);
        for i in 0..=50 {
            emit_send(&app, "hold", f64::from(i) / 50.0, None, v);
            sleep(HOLD_MS / 50).await;
        }
        emit_send(&app, "commit", 1.0, None, v);
        sleep(COMMIT_MS + 200).await;
        hide(&app, SEND);
        emit_send(&app, "idle", 0.0, None, 0);

        let file = std::env::temp_dir().join("windows-flow-preview.txt");
        let _ = std::fs::write(&file, "arquivo de teste do Windows Flow");
        let size = 3_400_000u64;
        let ev = |state: &'static str, done: u64| FileEvent {
            dir: "in",
            state,
            id: 1,
            name: "relatorio-final.pdf".into(),
            size,
            index: 0,
            count: 1,
            batch_done: done,
            batch_bytes: size,
            ratio: 5_000,
            side: "left",
            screen,
            path: (state == "done").then(|| file.to_string_lossy().into_owned()),
        };
        on_file_event(&app, ev("offer", 0));
        for i in 1..=20u64 {
            sleep(120).await;
            on_file_event(&app, ev("progress", size * i / 20));
        }
        on_file_event(&app, ev("done", size));
    });
}
