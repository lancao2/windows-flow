//! Ícone da bandeja com o estado da conexão e menu dinâmico (conectar, enviar, desconectar).

use std::net::Ipv4Addr;
use std::time::Duration;

use tauri::image::Image;
use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

use super::{auto, show_main_window, App};

const ID: &str = "main";

struct View {
    /// off | wait | ok
    tone: &'static str,
    title: String,
    connected: bool,
    active: bool,
    peers: Vec<(String, Ipv4Addr, u16)>,
}

fn view(app: &AppHandle) -> View {
    let Some(st) = app.try_state::<App>() else {
        return View {
            tone: "off",
            title: "Windows Flow".into(),
            connected: false,
            active: false,
            peers: Vec::new(),
        };
    };
    let status = st.status.lock().ok().and_then(|s| s.clone());
    let link = status.as_ref().map_or("idle", |s| s.link);
    let peers = st.auto.peers_snapshot();
    let settings = st.auto.settings();
    let peer_ip = status
        .as_ref()
        .and_then(|s| s.peer.clone())
        .and_then(|p| p.split(':').next().map(str::to_string));
    let name = peer_ip.as_deref().and_then(|ip| {
        peers
            .iter()
            .find(|p| p.ip.to_string() == ip)
            .map(|p| p.name.clone())
            .or_else(|| {
                settings
                    .known
                    .iter()
                    .find(|k| k.ip.as_deref() == Some(ip))
                    .map(|k| k.name.clone())
            })
    });
    let connected = link == "connected";
    let mut list: Vec<(String, Ipv4Addr, u16)> = peers
        .iter()
        .map(|p| (p.name.clone(), p.ip, p.port))
        .collect();
    for k in &settings.known {
        if let Some(ip) = k.ip.as_deref().and_then(|i| i.parse::<Ipv4Addr>().ok()) {
            if !list.iter().any(|(_, i, _)| *i == ip) {
                list.push((format!("{} (sem sinal)", k.name), ip, super::DEFAULT_PORT));
            }
        }
    }
    let title = match (connected, link) {
        (true, _) => format!("● Conectado a {}", name.as_deref().unwrap_or("outro PC")),
        (_, "idle") => "○ Sem conexão".to_string(),
        (_, "listening") => "◌ Aguardando conexão".to_string(),
        _ => "◌ Conectando…".to_string(),
    };
    View {
        tone: if connected {
            "ok"
        } else if link == "idle" {
            "off"
        } else {
            "wait"
        },
        title,
        connected,
        active: link != "idle" && link != "listening",
        peers: list,
    }
}

/// Ícone do app com uma bolinha de estado no canto (verde, âmbar ou cinza).
fn status_icon(base: &Image<'_>, tone: &str) -> Image<'static> {
    let (w, h) = (base.width() as i32, base.height() as i32);
    let mut px = base.rgba().to_vec();
    let color: [u8; 3] = match tone {
        "ok" => [0x3d, 0xdc, 0x84],
        "wait" => [0xff, 0xb7, 0x4d],
        _ => [0x8a, 0x96, 0xa3],
    };
    let r = (w.min(h) as f32 * 0.26).max(3.0);
    let (cx, cy) = (w as f32 - r - 0.5, h as f32 - r - 0.5);
    for y in 0..h {
        for x in 0..w {
            let d = (((x as f32 + 0.5) - cx).powi(2) + ((y as f32 + 0.5) - cy).powi(2)).sqrt();
            let i = ((y * w + x) * 4) as usize;
            if d <= r {
                // Contorno escuro de 1 px e miolo colorido.
                let c = if d > r - 1.2 {
                    [0x10, 0x14, 0x1c]
                } else {
                    color
                };
                px[i..i + 3].copy_from_slice(&c);
                px[i + 3] = 255;
            }
        }
    }
    Image::new_owned(px, w as u32, h as u32)
}

type Built = tauri::Result<(Menu<Wry>, Image<'static>, String, String)>;

fn build(app: &AppHandle) -> Built {
    let v = view(app);
    let status_item = MenuItem::with_id(app, "status", &v.title, false, None::<&str>)?;
    let show = MenuItem::with_id(app, "show", "Abrir Windows Flow", true, None::<&str>)?;
    let peer_items: Vec<MenuItem<Wry>> = if v.peers.is_empty() {
        vec![MenuItem::with_id(
            app,
            "none",
            "Nenhum computador encontrado",
            false,
            None::<&str>,
        )?]
    } else {
        v.peers
            .iter()
            .map(|(name, ip, port)| {
                MenuItem::with_id(app, format!("peer:{ip}:{port}"), name, true, None::<&str>)
            })
            .collect::<tauri::Result<_>>()?
    };
    let refs: Vec<&dyn IsMenuItem<Wry>> = peer_items
        .iter()
        .map(|i| i as &dyn IsMenuItem<Wry>)
        .collect();
    let connect = Submenu::with_id_and_items(
        app,
        "peers",
        "Conectar a…",
        !v.connected && !v.active,
        &refs,
    )?;
    let send_files = MenuItem::with_id(
        app,
        "send-files",
        "Enviar arquivo…",
        v.connected,
        None::<&str>,
    )?;
    let send_dir = MenuItem::with_id(
        app,
        "send-folder",
        "Enviar pasta…",
        v.connected,
        None::<&str>,
    )?;
    let disc = MenuItem::with_id(
        app,
        "disconnect",
        if v.connected {
            "Desconectar"
        } else {
            "Cancelar conexão"
        },
        v.connected || v.active,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Sair", true, None::<&str>)?;
    let (s1, s2) = (
        PredefinedMenuItem::separator(app)?,
        PredefinedMenuItem::separator(app)?,
    );
    let menu = Menu::with_items(
        app,
        &[
            &status_item,
            &s1,
            &show,
            &connect,
            &send_files,
            &send_dir,
            &disc,
            &s2,
            &quit,
        ],
    )?;
    let base = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("ícone padrão".into()))?;
    let icon = status_icon(&base, v.tone);
    let tip = format!(
        "Windows Flow — {}",
        v.title.trim_start_matches(['●', '○', '◌', ' '])
    );
    // Chave para só refazer o menu quando algo mudou.
    let key = format!(
        "{}|{}|{}|{}|{:?}",
        v.tone, v.title, v.connected, v.active, v.peers
    );
    Ok((menu, icon, tip, key))
}

pub fn create(app: &tauri::App) -> tauri::Result<()> {
    let handle = app.handle().clone();
    let (menu, icon, tip, key) = build(&handle)?;
    if let Some(st) = handle.try_state::<App>() {
        if let Ok(mut k) = st.tray_key.lock() {
            *k = key;
        }
    }
    TrayIconBuilder::with_id(ID)
        .icon(icon)
        .tooltip(tip)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu)
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    // Atualiza o ícone/menu quando algo muda (verificação barata a cada 2 s).
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            refresh(&handle);
        }
    });
    Ok(())
}

fn refresh(app: &AppHandle) {
    let Some(tray) = app.tray_by_id(ID) else {
        return;
    };
    let Ok((menu, icon, tip, key)) = build(app) else {
        return;
    };
    let Some(st) = app.try_state::<App>() else {
        return;
    };
    let changed = st.tray_key.lock().is_ok_and(|mut k| {
        let diff = *k != key;
        if diff {
            *k = key;
        }
        diff
    });
    if changed {
        let _ = tray.set_menu(Some(menu));
        let _ = tray.set_icon(Some(icon));
        let _ = tray.set_tooltip(Some(tip));
    }
}

fn on_menu(app: &AppHandle, event: tauri::menu::MenuEvent) {
    let id = event.id.as_ref();
    match id {
        "show" => show_main_window(app),
        "disconnect" => auto::disconnect(app),
        "send-files" => auto::pick_and_send(app.clone(), false),
        "send-folder" => auto::pick_and_send(app.clone(), true),
        "quit" => app.exit(0),
        _ => {
            if let Some(rest) = id.strip_prefix("peer:") {
                if let Some((ip, port)) = rest.rsplit_once(':') {
                    if let (Ok(ip), Ok(port)) = (ip.parse::<Ipv4Addr>(), port.parse::<u16>()) {
                        auto::connect_ip(app, ip, port, false);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_dot_changes_color_and_leaves_the_rest_alone() {
        let base = Image::new_owned(vec![0u8; 32 * 32 * 4], 32, 32);
        let px = |img: &Image<'_>, x: usize, y: usize| {
            let i = (y * 32 + x) * 4;
            img.rgba()[i..i + 4].to_vec()
        };
        let ok = status_icon(&base, "ok");
        let wait = status_icon(&base, "wait");
        let off = status_icon(&base, "off");
        // Centro da bolinha (canto inferior direito): colorida e opaca.
        assert_eq!(px(&ok, 26, 26), vec![0x3d, 0xdc, 0x84, 255]);
        assert_eq!(px(&wait, 26, 26), vec![0xff, 0xb7, 0x4d, 255]);
        assert_eq!(px(&off, 26, 26), vec![0x8a, 0x96, 0xa3, 255]);
        // O canto oposto continua transparente.
        assert_eq!(px(&ok, 2, 2), vec![0, 0, 0, 0]);
    }
}
