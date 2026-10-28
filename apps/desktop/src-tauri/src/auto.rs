//! Conexão sem digitar IP: descobre PCs com o app na rede, avisa com uma notificação, lembra
//! computadores confiáveis (conectam sozinhos, sem pedir o código de novo) e guarda a posição
//! da tela de cada um.
//!
//! Descobrir não confia em ninguém: um computador só é reconhecido pela impressão digital do
//! certificado (a chave fixa dele); um nome igual com outra chave ainda pede o código.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use continuity_core::engine::Command;
use discovery::{DiscoveryEvent, Peer};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc;
use windows_input::Side;

use super::{local_ip, place_overlay, App, DEFAULT_PORT};

pub const TOAST: &str = "toast";
/// Tamanho lógico (px CSS) da notificação.
const TOAST_SIZE: (i32, i32) = (344, 148);
const KEEPER_TICK: Duration = Duration::from_secs(3);
/// Intervalo entre tentativas diretas de reconexão (por IP salvo).
const RETRY_LAST: Duration = Duration::from_secs(12);
/// O PC de maior IP só toma a iniciativa depois deste tempo (o de menor IP vai primeiro).
const FALLBACK_AFTER: Duration = Duration::from_secs(8);

/// Computador em que o usuário confia ("lembrar este computador").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KnownPc {
    pub name: String,
    /// Impressão digital (hex) do certificado: prova que é o mesmo computador.
    pub fingerprint: String,
    /// Último IP visto (para tentar conectar mesmo sem descoberta).
    pub ip: Option<String>,
    /// Lado em que ele fica em relação a este PC (left/right/top/bottom).
    pub side: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Settings {
    /// IP do último PC com o qual a conexão foi concluída.
    pub last_peer: Option<String>,
    /// Ficar visível na rede e aguardar conexões sozinho.
    pub visible: bool,
    /// Reconectar sozinho aos computadores conhecidos / ao último PC.
    pub auto_connect: bool,
    /// Computadores confiáveis: conectam sem pedir o código de novo.
    pub known: Vec<KnownPc>,
    /// Iniciar com o Windows (minimizado na bandeja).
    pub autostart: bool,
    /// O pedido de liberação do firewall já foi feito (não insiste a cada abertura).
    pub firewall_prompted: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            last_peer: None,
            visible: true,
            auto_connect: true,
            known: Vec::new(),
            autostart: true,
            firewall_prompted: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DiscStatus {
    listening: bool,
    interfaces: usize,
    received: u64,
}

pub struct Auto {
    path: PathBuf,
    settings: Mutex<Settings>,
    peers: Mutex<Vec<Peer>>,
    /// PCs cuja notificação o usuário dispensou (volta a avisar se sumirem e reaparecerem).
    ignored: Mutex<HashSet<String>>,
    /// Depois de "Desconectar" manual, não reconecta sozinho até uma ação do usuário.
    paused: AtomicBool,
    own_id: String,
    /// Impressão digital (hex) do PC conectado agora.
    current_fp: Mutex<Option<String>>,
    /// O usuário marcou "não perguntar de novo" no aviso: o diálogo do código já vem marcado.
    remember_next: AtomicBool,
    disc_status: Mutex<Option<DiscStatus>>,
    /// Último resultado (e quando) da checagem das regras de firewall.
    firewall: Mutex<Option<(Instant, bool)>>,
    discovery: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

impl Auto {
    pub fn load(path: PathBuf) -> Self {
        let settings = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            path,
            settings: Mutex::new(settings),
            peers: Mutex::new(Vec::new()),
            ignored: Mutex::new(HashSet::new()),
            paused: AtomicBool::new(false),
            own_id: random_id(),
            current_fp: Mutex::new(None),
            remember_next: AtomicBool::new(false),
            disc_status: Mutex::new(None),
            firewall: Mutex::new(None),
            discovery: Mutex::new(None),
        }
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn update(&self, f: impl FnOnce(&mut Settings)) -> Settings {
        let mut g = self.settings.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut g);
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(b) = serde_json::to_vec_pretty(&*g) {
            let _ = std::fs::write(&self.path, b);
        }
        g.clone()
    }

    /// O usuário agiu (conectou/aguardou): volta a permitir reconexão automática.
    pub fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
    }

    pub fn peers_snapshot(&self) -> Vec<Peer> {
        self.peers.lock().map(|p| p.clone()).unwrap_or_default()
    }

    /// Não há mais conexão: esquece qual computador era (a posição só vale para o conectado).
    pub fn clear_peer(&self) {
        if let Ok(mut c) = self.current_fp.lock() {
            *c = None;
        }
    }

    /// As regras de firewall existem? (guarda o resultado por 10 s: o `netsh` é lento.)
    pub fn firewall_ok(&self, force: bool) -> bool {
        if !force {
            if let Some((t, v)) = self.firewall.lock().ok().and_then(|f| *f) {
                if t.elapsed() < Duration::from_secs(10) {
                    return v;
                }
            }
        }
        let v = windows_input::windows_impl::firewall_rules_present();
        if let Ok(mut f) = self.firewall.lock() {
            *f = Some((Instant::now(), v));
        }
        v
    }

    /// Impressões digitais confiáveis para o motor.
    pub fn trusted_fps(&self) -> Vec<[u8; 32]> {
        self.settings()
            .known
            .iter()
            .filter_map(|k| unhex(&k.fingerprint))
            .collect()
    }

    /// Chamado a cada status: grava o IP do par quando a conexão se completa.
    pub fn on_connected(&self, app: &AppHandle, peer: Option<&str>) {
        let Some(ip) = peer
            .and_then(|p| p.parse::<SocketAddr>().ok())
            .map(|a| a.ip().to_string())
        else {
            return;
        };
        if self.settings().last_peer.as_deref() == Some(ip.as_str()) {
            return;
        }
        let s = self.update(|s| s.last_peer = Some(ip));
        let _ = app.emit("settings-changed", s);
    }

    /// O usuário mudou a posição do outro PC: guarda para o computador conhecido atual.
    pub fn save_side(&self, app: &AppHandle, side: &str) {
        let Some(fp) = self.current_fp.lock().ok().and_then(|f| f.clone()) else {
            return;
        };
        if !self.settings().known.iter().any(|k| k.fingerprint == fp) {
            return;
        }
        let side = side.to_string();
        let s = self.update(|s| {
            if let Some(k) = s.known.iter_mut().find(|k| k.fingerprint == fp) {
                k.side = Some(side);
            }
        });
        let _ = app.emit("settings-changed", s);
    }
}

pub fn unhex(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn random_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u32(std::process::id());
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    format!("{:016x}", h.finish())
}

fn device_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "este-pc".into())
}

/// Só um dos dois PCs toma a iniciativa primeiro (o de menor IP); o outro aguarda.
fn i_initiate(peer: Ipv4Addr) -> bool {
    match local_ip() {
        Some(IpAddr::V4(me)) => u32::from(me) < u32::from(peer),
        _ => false,
    }
}

fn link_of(app: &AppHandle) -> &'static str {
    app.try_state::<App>()
        .and_then(|s| {
            s.status
                .lock()
                .ok()
                .and_then(|s| s.as_ref().map(|s| s.link))
        })
        .unwrap_or("idle")
}

fn known_by_name<'a>(s: &'a Settings, name: &str) -> Option<&'a KnownPc> {
    s.known.iter().find(|k| k.name.eq_ignore_ascii_case(name))
}

/// Aplica a posição de tela salva do computador antes/depois de conectar.
fn apply_side(st: &App, side: Option<&str>) {
    if let Some(side) = side.and_then(Side::parse) {
        st.engine.send(Command::SetPeerSide(side));
    }
}

// ---- eventos do motor ------------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct ConfirmPayload {
    sas: String,
    peer: String,
    /// Nome do PC, se ele foi visto na rede.
    name: Option<String>,
    /// Marcar "lembrar este computador" por padrão.
    remember: bool,
}

pub fn confirm_payload(app: &AppHandle, sas: String, peer: String) -> ConfirmPayload {
    let (name, remember) = app
        .try_state::<App>()
        .map(|st| {
            let ip = peer.parse::<SocketAddr>().ok().map(|a| a.ip());
            let name = st.auto.peers.lock().ok().and_then(|p| {
                p.iter()
                    .find(|x| Some(IpAddr::V4(x.ip)) == ip)
                    .map(|x| x.name.clone())
            });
            (name, st.auto.remember_next.load(Ordering::Relaxed))
        })
        .unwrap_or((None, false));
    ConfirmPayload {
        sas,
        peer,
        name,
        remember,
    }
}

/// O par completou a conexão: guarda o computador (se pedido) e aplica a posição salva dele.
pub fn on_peer_hello(app: &AppHandle, name: String, fingerprint: String, remembered: bool) {
    let Some(st) = app.try_state::<App>() else {
        return;
    };
    if let Ok(mut c) = st.auto.current_fp.lock() {
        *c = Some(fingerprint.clone());
    }
    st.auto.remember_next.store(false, Ordering::Relaxed);
    let (ip, current_side) = {
        let status = st.status.lock().ok().and_then(|s| s.clone());
        (
            status
                .as_ref()
                .and_then(|s| s.peer.clone())
                .and_then(|p| p.parse::<SocketAddr>().ok())
                .map(|a| a.ip().to_string()),
            status.map(|s| s.peer_side.to_string()),
        )
    };
    let mut saved_side = None;
    let s = st.auto.update(|s| {
        if let Some(k) = s.known.iter_mut().find(|k| k.fingerprint == fingerprint) {
            k.name = name.clone();
            if ip.is_some() {
                k.ip = ip.clone();
            }
            saved_side = k.side.clone();
        } else if remembered {
            s.known.push(KnownPc {
                name: name.clone(),
                fingerprint: fingerprint.clone(),
                ip: ip.clone(),
                side: current_side,
            });
        }
    });
    st.engine.send(Command::SetTrusted(st.auto.trusted_fps()));
    apply_side(&st, saved_side.as_deref());
    let _ = app.emit("settings-changed", s);
}

// ---- descoberta ------------------------------------------------------------------------

/// Liga (ou religa) os anúncios e a escuta de outros PCs.
pub fn start_discovery(app: &AppHandle) {
    let Some(st) = app.try_state::<App>() else {
        return;
    };
    stop_discovery(app);
    let (tx, rx) = mpsc::unbounded_channel();
    let id = st.auto.own_id.clone();
    let proto = protocol::PROTOCOL_VERSION;
    let h = app.clone();
    let task = tauri::async_runtime::spawn(async move {
        tokio::select! {
            () = discovery::run(id, device_name(), DEFAULT_PORT, proto, tx) => {}
            () = handle_events(h, rx) => {}
        }
    });
    let mut d = st.auto.discovery.lock().unwrap_or_else(|e| e.into_inner());
    *d = Some(task);
}

pub fn stop_discovery(app: &AppHandle) {
    if let Some(st) = app.try_state::<App>() {
        if let Some(t) = st.auto.discovery.lock().ok().and_then(|mut d| d.take()) {
            t.abort();
        }
        if let Ok(mut p) = st.auto.peers.lock() {
            p.clear();
        }
        if let Ok(mut d) = st.auto.disc_status.lock() {
            *d = None;
        }
        let _ = app.emit("peers-changed", Vec::<Peer>::new());
    }
}

async fn handle_events(app: AppHandle, mut rx: mpsc::UnboundedReceiver<DiscoveryEvent>) {
    while let Some(ev) = rx.recv().await {
        let Some(st) = app.try_state::<App>() else {
            continue;
        };
        match ev {
            DiscoveryEvent::Found(peer) => {
                if let Ok(mut p) = st.auto.peers.lock() {
                    p.retain(|x| x.id != peer.id);
                    p.push(peer.clone());
                    let _ = app.emit("peers-changed", p.clone());
                }
                on_found(&app, &st, peer);
            }
            DiscoveryEvent::Lost(id) => {
                if let Ok(mut p) = st.auto.peers.lock() {
                    p.retain(|x| x.id != id);
                    let _ = app.emit("peers-changed", p.clone());
                }
                if let Ok(mut i) = st.auto.ignored.lock() {
                    i.remove(&id);
                }
            }
            DiscoveryEvent::Status {
                listening,
                interfaces,
                received,
            } => {
                let d = DiscStatus {
                    listening,
                    interfaces,
                    received,
                };
                if let Ok(mut s) = st.auto.disc_status.lock() {
                    *s = Some(d.clone());
                }
                let _ = app.emit("discovery-status", d);
            }
        }
    }
}

fn connect_to(st: &App, peer: &Peer, side: Option<&str>) {
    apply_side(st, side);
    st.engine.send(Command::Connect(SocketAddr::new(
        IpAddr::V4(peer.ip),
        peer.port,
    )));
}

fn on_found(app: &AppHandle, st: &App, peer: Peer) {
    // Já conectado ou em pareamento: só atualiza a lista.
    if !matches!(link_of(app), "idle" | "listening") {
        return;
    }
    let settings = st.auto.settings();
    let known_side = known_by_name(&settings, &peer.name).map(|k| k.side.clone());
    let is_last = settings.last_peer.as_deref() == Some(peer.ip.to_string().as_str());
    if settings.auto_connect && (known_side.is_some() || is_last) {
        // Computador conhecido: conecta sozinho, sem aviso. O de menor IP vai primeiro e o
        // outro entra depois de alguns segundos, caso a conexão ainda não tenha acontecido.
        if st.auto.paused.load(Ordering::Relaxed) {
            return;
        }
        let side = known_side.flatten();
        if i_initiate(peer.ip) {
            connect_to(st, &peer, side.as_deref());
        } else {
            let h = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(FALLBACK_AFTER).await;
                let Some(st) = h.try_state::<App>() else {
                    return;
                };
                let still_there = st
                    .auto
                    .peers
                    .lock()
                    .is_ok_and(|p| p.iter().any(|x| x.id == peer.id));
                if still_there
                    && matches!(link_of(&h), "idle" | "listening")
                    && !st.auto.paused.load(Ordering::Relaxed)
                {
                    connect_to(&st, &peer, side.as_deref());
                }
            });
        }
        return;
    }
    let ignored = st.auto.ignored.lock().is_ok_and(|i| i.contains(&peer.id));
    let main_focused = app
        .get_webview_window("main")
        .is_some_and(|w| w.is_focused().unwrap_or(false));
    if !ignored && !main_focused {
        show_toast(app, &peer);
    }
}

#[derive(Serialize, Clone)]
struct ToastPayload {
    id: String,
    name: String,
    ip: String,
    port: u16,
}

/// Notificação no canto inferior direito (acima da barra de tarefas) do monitor principal.
fn show_toast(app: &AppHandle, peer: &Peer) {
    let Some(win) = app.get_webview_window(TOAST) else {
        return;
    };
    let Ok(Some(m)) = app.primary_monitor() else {
        return;
    };
    let scale = m.scale_factor();
    let wa = m.work_area();
    let (w, h) = (
        (f64::from(TOAST_SIZE.0) * scale).round() as i32,
        (f64::from(TOAST_SIZE.1) * scale).round() as i32,
    );
    let margin = (16.0 * scale).round() as i32;
    let x = wa.position.x + wa.size.width as i32 - w - margin;
    let y = wa.position.y + wa.size.height as i32 - h - margin;
    let _ = win.set_ignore_cursor_events(false);
    place_overlay(&win, x, y, w, h);
    let _ = win.show();
    let _ = app.emit_to(
        TOAST,
        "toast",
        ToastPayload {
            id: peer.id.clone(),
            name: peer.name.clone(),
            ip: peer.ip.to_string(),
            port: peer.port,
        },
    );
}

fn hide_toast(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(TOAST) {
        let _ = w.hide();
    }
}

// ---- vigia: hospeda e reconecta sozinho -----------------------------------------------

/// A cada poucos segundos, se o app está ocioso: tenta reconectar pelo IP salvo dos
/// computadores conhecidos (mesmo quando a descoberta não funciona na rede) ou volta a aguardar
/// conexões, com recuo exponencial se a porta estiver ocupada.
pub fn start_keeper(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut hosted_last_tick = false;
        let mut failures = 0u32;
        let mut wait_ticks = 0u32;
        let mut idle_since: Option<Instant> = None;
        let mut last_try: Option<Instant> = None;
        let mut rotate = 0usize;
        loop {
            tokio::time::sleep(KEEPER_TICK).await;
            let Some(st) = app.try_state::<App>() else {
                continue;
            };
            if link_of(&app) != "idle" {
                hosted_last_tick = false;
                failures = 0;
                wait_ticks = 0;
                idle_since = None;
                continue;
            }
            let idle_for = idle_since.get_or_insert_with(Instant::now).elapsed();
            if hosted_last_tick {
                // Pedimos para aguardar e continua ocioso: a porta falhou.
                failures = (failures + 1).min(5);
                wait_ticks = (1 << failures).min(20);
                hosted_last_tick = false;
            }
            if wait_ticks > 0 {
                wait_ticks -= 1;
                continue;
            }
            let s = st.auto.settings();
            if s.auto_connect
                && !st.auto.paused.load(Ordering::Relaxed)
                && last_try.is_none_or(|t| t.elapsed() >= RETRY_LAST)
            {
                // IPs candidatos: computadores conhecidos e o último PC.
                let mut cands: Vec<(Ipv4Addr, Option<String>)> = s
                    .known
                    .iter()
                    .filter_map(|k| Some((k.ip.as_deref()?.parse().ok()?, k.side.clone())))
                    .collect();
                if let Some(ip) = s.last_peer.as_deref().and_then(|i| i.parse().ok()) {
                    if !cands.iter().any(|(c, _)| *c == ip) {
                        cands.push((ip, None));
                    }
                }
                if !cands.is_empty() {
                    let (ip, side) = cands[rotate % cands.len()].clone();
                    rotate += 1;
                    if i_initiate(ip) || idle_for >= FALLBACK_AFTER * 2 {
                        last_try = Some(Instant::now());
                        // Sondagem rápida: só conecta se o outro PC está de fato aguardando.
                        // (Uma conexão a um PC desligado travaria o app por ~10 s sem aguardar.)
                        let addr = SocketAddr::new(IpAddr::V4(ip), DEFAULT_PORT);
                        let up = tokio::time::timeout(
                            Duration::from_millis(1_200),
                            tokio::net::TcpStream::connect(addr),
                        )
                        .await
                        .is_ok_and(|r| r.is_ok());
                        if up && link_of(&app) == "idle" {
                            apply_side(&st, side.as_deref());
                            st.engine.send(Command::Connect(addr));
                            continue;
                        }
                    }
                }
            }
            if s.visible {
                st.engine.send(Command::Host(SocketAddr::new(
                    IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                    DEFAULT_PORT,
                )));
                hosted_last_tick = true;
            }
        }
    });
}

// ---- comandos --------------------------------------------------------------------------

#[tauri::command]
pub fn get_settings(state: State<'_, App>) -> Settings {
    state.auto.settings()
}

#[tauri::command]
pub fn get_peers(state: State<'_, App>) -> Vec<Peer> {
    state
        .auto
        .peers
        .lock()
        .map(|p| p.clone())
        .unwrap_or_default()
}

#[tauri::command]
pub fn get_discovery(state: State<'_, App>) -> Option<DiscStatus> {
    state.auto.disc_status.lock().ok().and_then(|d| d.clone())
}

#[tauri::command]
pub fn set_visible(app: AppHandle, state: State<'_, App>, visible: bool) {
    let s = state.auto.update(|s| s.visible = visible);
    let _ = app.emit("settings-changed", s);
    if visible {
        start_discovery(&app);
    } else {
        stop_discovery(&app);
        // Para de aguardar conexões, mas não derruba uma conexão ativa.
        if link_of(&app) == "listening" {
            state.engine.send(Command::Disconnect);
        }
    }
}

#[tauri::command]
pub fn set_auto_connect(app: AppHandle, state: State<'_, App>, enabled: bool) {
    let s = state.auto.update(|s| s.auto_connect = enabled);
    let _ = app.emit("settings-changed", s);
}

#[tauri::command]
pub fn forget_last_peer(app: AppHandle, state: State<'_, App>) {
    let s = state.auto.update(|s| s.last_peer = None);
    let _ = app.emit("settings-changed", s);
}

/// Esquece um computador conhecido: volta a pedir o código nas próximas conexões com ele.
#[tauri::command]
pub fn forget_known(app: AppHandle, state: State<'_, App>, fingerprint: String) {
    let s = state
        .auto
        .update(|s| s.known.retain(|k| k.fingerprint != fingerprint));
    state
        .engine
        .send(Command::SetTrusted(state.auto.trusted_fps()));
    let _ = app.emit("settings-changed", s);
}

/// Conecta a um IP (descoberto, conhecido ou escolhido no menu da bandeja).
pub fn connect_ip(app: &AppHandle, ip: Ipv4Addr, port: u16, remember: bool) {
    let Some(st) = app.try_state::<App>() else {
        return;
    };
    st.auto.resume();
    st.auto.remember_next.store(remember, Ordering::Relaxed);
    st.engine
        .send(Command::Connect(SocketAddr::new(IpAddr::V4(ip), port)));
    hide_toast(app);
}

/// Desconexão pedida pelo usuário: não reconecta sozinho até ele conectar de novo.
pub fn disconnect(app: &AppHandle) {
    if let Some(st) = app.try_state::<App>() {
        st.auto.pause();
        st.engine.send(Command::Disconnect);
    }
}

/// Abre o seletor de arquivos (ou de pasta) do Windows e envia o que for escolhido.
pub fn pick_and_send(app: AppHandle, folder: bool) {
    std::thread::spawn(move || {
        let dialog = rfd::FileDialog::new().set_title(if folder {
            "Escolha a pasta para enviar"
        } else {
            "Escolha os arquivos para enviar"
        });
        let paths = if folder {
            dialog.pick_folder().map(|p| vec![p])
        } else {
            dialog.pick_files()
        };
        if let (Some(paths), Some(st)) = (paths, app.try_state::<App>()) {
            // Sem arrastar até a borda: a animação de chegada sai do meio da borda.
            st.engine.send(Command::SendFiles {
                paths,
                ratio: 5_000,
            });
        }
    });
}

/// Aplica o início com o Windows e, uma vez, pede a liberação do firewall (UAC).
/// Execuções de teste (`--preview*`, `--xfer-qa*`) não mexem no sistema.
pub fn system_setup(app: &AppHandle) {
    let qa = std::env::args().any(|a| a.starts_with("--preview") || a.starts_with("--xfer-qa"));
    let Some(st) = app.try_state::<App>() else {
        return;
    };
    if qa {
        return;
    }
    apply_autostart(st.auto.settings().autostart);
    if std::env::var_os("WF_SKIP_FIREWALL").is_some() {
        return;
    }
    let h = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(st) = h.try_state::<App>() else {
            return;
        };
        if st.auto.firewall_ok(true) || st.auto.settings().firewall_prompted {
            return;
        }
        st.auto.update(|s| s.firewall_prompted = true);
        windows_input::windows_impl::request_firewall_rules();
    });
}

fn apply_autostart(enabled: bool) {
    use windows_input::windows_impl::{autostart_command, set_autostart};
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let cmd = format!("\"{}\" --minimized", exe.display());
    let current = autostart_command();
    if enabled && current.as_deref() != Some(cmd.as_str()) {
        set_autostart(true, &cmd);
    } else if !enabled && current.is_some() {
        set_autostart(false, "");
    }
}

#[tauri::command]
pub fn set_autostart_pref(app: AppHandle, state: State<'_, App>, enabled: bool) {
    let s = state.auto.update(|s| s.autostart = enabled);
    apply_autostart(enabled);
    let _ = app.emit("settings-changed", s);
}

/// Pede ao Windows para liberar as portas do app no firewall (abre o aviso do UAC).
#[tauri::command]
pub fn fix_firewall(app: AppHandle, state: State<'_, App>) {
    state.auto.update(|s| s.firewall_prompted = true);
    windows_input::windows_impl::request_firewall_rules();
    // Confere de novo depois que o usuário responder ao aviso.
    tauri::async_runtime::spawn(async move {
        for _ in 0..20 {
            tokio::time::sleep(Duration::from_secs(2)).await;
            if let Some(st) = app.try_state::<App>() {
                if st.auto.firewall_ok(true) {
                    let _ = app.emit("firewall-changed", true);
                    return;
                }
            }
        }
    });
}

#[tauri::command]
pub fn cancel_transfers(state: State<'_, App>) {
    state.engine.send(Command::CancelTransfers);
}

#[tauri::command(async)]
pub fn pick_and_send_cmd(app: AppHandle, folder: bool) {
    pick_and_send(app, folder);
}

/// Conecta a um PC descoberto. `remember`: o aviso foi aceito com "não perguntar de novo".
#[tauri::command]
pub fn connect_peer(
    app: AppHandle,
    ip: String,
    port: Option<u16>,
    remember: Option<bool>,
) -> Result<(), String> {
    let ip: Ipv4Addr = ip.parse().map_err(|_| "endereço inválido".to_string())?;
    connect_ip(
        &app,
        ip,
        port.unwrap_or(DEFAULT_PORT),
        remember.unwrap_or(false),
    );
    Ok(())
}

/// Fecha a notificação; `id` é lembrado para não incomodar de novo enquanto o PC estiver na rede.
#[tauri::command]
pub fn toast_dismiss(app: AppHandle, state: State<'_, App>, id: String) {
    if let Ok(mut i) = state.auto.ignored.lock() {
        i.insert(id);
    }
    hide_toast(&app);
}

pub fn create_toast_window(app: &tauri::App) -> tauri::Result<()> {
    let win = tauri::WebviewWindowBuilder::new(
        app,
        TOAST,
        tauri::WebviewUrl::App("index.html#toast".into()),
    )
    .title("Windows Flow Aviso")
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .shadow(false)
    .resizable(false)
    .focused(false)
    .focusable(false)
    .visible(false)
    .disable_drag_drop_handler()
    .build()?;
    win.set_ignore_cursor_events(false)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip_and_validation() {
        let fp: String = (0..32u8).map(|b| format!("{b:02x}")).collect();
        let bytes = unhex(&fp).unwrap();
        assert_eq!(bytes[31], 31);
        assert!(unhex("zz").is_none());
        assert!(unhex(&"g".repeat(64)).is_none());
    }

    #[test]
    fn settings_default_and_partial_files() {
        let d = Settings::default();
        assert!(d.visible && d.auto_connect && d.known.is_empty());
        // Arquivos de versões anteriores (sem `known`) continuam válidos.
        let old: Settings = serde_json::from_str(r#"{"last_peer":"10.0.0.2"}"#).unwrap();
        assert_eq!(old.last_peer.as_deref(), Some("10.0.0.2"));
        assert!(old.visible && old.known.is_empty());
    }

    #[test]
    fn known_lookup_is_case_insensitive() {
        let s = Settings {
            known: vec![KnownPc {
                name: "SALA-PC".into(),
                fingerprint: "0".repeat(64),
                ip: None,
                side: Some("left".into()),
            }],
            ..Settings::default()
        };
        assert!(known_by_name(&s, "sala-pc").is_some());
        assert!(known_by_name(&s, "outro").is_none());
    }
}
