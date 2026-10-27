//! Implementação Windows de [`Platform`].
//!
//! Uma única thread com message loop hospeda o atalho de emergência
//! (`RegisterHotKey`) e, apenas enquanto a captura está ligada, os hooks
//! `WH_MOUSE_LL`/`WH_KEYBOARD_LL`. Os callbacks dos hooks fazem o mínimo: enviam
//! o evento por um canal sem bloqueio e engolem a entrada local.
//!
//! Salvaguardas: se o motor deixa de dar sinal de vida (`keepalive`) por 2 s, ou
//! o canal de eventos é fechado, a captura se desliga sozinha e a entrada volta
//! a fluir normalmente.

use std::collections::HashSet;
use std::mem::size_of;
use std::sync::atomic::{AtomicI32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS,
    HGDIOBJ, ROP_CODE, SRCCOPY,
};
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThreadId, ProcessPowerThrottling, SetProcessInformation,
    PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::{ButtonState, InputEvent, Platform, Point, PointerButton, Rect};

const DEADMAN_MS: u64 = 2_000;
/// Captura desligada: a entrada local flui normalmente.
const MODE_OFF: u8 = 0;
/// PC de origem: engole a entrada física e a encaminha ao par.
const MODE_FORWARD: u8 = 1;
/// PC controlado: engole a entrada física (só passa o que foi injetado). Clique/tecla retoma o controle.
const MODE_BLOCK: u8 = 2;
const WM_ENABLE: u32 = WM_APP + 1;
const WM_DISABLE: u32 = WM_APP + 2;
const HOTKEY_ID: i32 = 0x57F1;

// Bits de modificadores acompanhados dentro do hook.
const M_LSHIFT: u8 = 1;
const M_RSHIFT: u8 = 1 << 1;
const M_LCTRL: u8 = 1 << 2;
const M_RCTRL: u8 = 1 << 3;
const M_LALT: u8 = 1 << 4;
const M_RALT: u8 = 1 << 5;
const M_LWIN: u8 = 1 << 6;
const M_RWIN: u8 = 1 << 7;

const MOD_VKS: [(u16, u8); 8] = [
    (0xA0, M_LSHIFT),
    (0xA1, M_RSHIFT),
    (0xA2, M_LCTRL),
    (0xA3, M_RCTRL),
    (0xA4, M_LALT),
    (0xA5, M_RALT),
    (0x5B, M_LWIN),
    (0x5C, M_RWIN),
];

struct Shared {
    tx: Mutex<Option<UnboundedSender<InputEvent>>>,
    mode: AtomicU8,
    cx: AtomicI32,
    cy: AtomicI32,
    alive_ms: AtomicU64,
    mods: AtomicU8,
    entry_mods: AtomicU8,
}

static SHARED: Shared = Shared {
    tx: Mutex::new(None),
    mode: AtomicU8::new(MODE_OFF),
    cx: AtomicI32::new(0),
    cy: AtomicI32::new(0),
    alive_ms: AtomicU64::new(0),
    mods: AtomicU8::new(0),
    entry_mods: AtomicU8::new(0),
};

fn now_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

fn send_event(ev: InputEvent) -> bool {
    let guard = SHARED.tx.lock().unwrap_or_else(|e| e.into_inner());
    guard.as_ref().is_some_and(|tx| tx.send(ev).is_ok())
}

/// Modo atual da captura, já aplicando o deadman: se o motor ficou mudo por 2 s,
/// desliga tudo e avisa com `Emergency`.
fn current_mode() -> u8 {
    let mode = SHARED.mode.load(Ordering::Acquire);
    if mode == MODE_OFF {
        return MODE_OFF;
    }
    if now_ms().saturating_sub(SHARED.alive_ms.load(Ordering::Relaxed)) > DEADMAN_MS {
        SHARED.mode.store(MODE_OFF, Ordering::Release);
        send_event(InputEvent::Emergency);
        return MODE_OFF;
    }
    mode
}

/// Em modo BLOCK: pedir retomada do controle local e parar de engolir entrada.
fn takeover() {
    SHARED.mode.store(MODE_OFF, Ordering::Release);
    send_event(InputEvent::Takeover);
}
/// Entrega o evento; se ninguém recebe mais, desliga a captura e não engole nada.
fn deliver(ev: InputEvent) -> bool {
    if send_event(ev) {
        true
    } else {
        SHARED.mode.store(MODE_OFF, Ordering::Release);
        false
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let mode = if code == 0 { current_mode() } else { MODE_OFF };
    if mode != MODE_OFF {
        // SAFETY: para WH_MOUSE_LL com code == HC_ACTION, lparam aponta para um MSLLHOOKSTRUCT válido.
        let ms = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        const INJECTED: u32 = 0x01;
        // Entrada injetada (a que nós mesmos geramos) sempre passa.
        if ms.flags & INJECTED == 0 {
            if mode == MODE_FORWARD {
                if handle_mouse(wparam.0 as u32, ms) {
                    return LRESULT(1);
                }
            } else {
                let msg = wparam.0 as u32;
                if matches!(
                    msg,
                    WM_LBUTTONDOWN
                        | WM_RBUTTONDOWN
                        | WM_MBUTTONDOWN
                        | WM_XBUTTONDOWN
                        | WM_MOUSEWHEEL
                        | WM_MOUSEHWHEEL
                ) {
                    takeover();
                }
                return LRESULT(1);
            }
        }
    }
    // SAFETY: repassa os mesmos argumentos recebidos.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn handle_mouse(msg: u32, ms: &MSLLHOOKSTRUCT) -> bool {
    let hi = (ms.mouseData >> 16) as u16;
    let button = |b, s| InputEvent::Button {
        button: b,
        state: s,
    };
    let ev = match msg {
        WM_MOUSEMOVE => {
            let (cx, cy) = (
                SHARED.cx.load(Ordering::Relaxed),
                SHARED.cy.load(Ordering::Relaxed),
            );
            let (dx, dy) = (ms.pt.x - cx, ms.pt.y - cy);
            if dx == 0 && dy == 0 {
                return true;
            }
            // Mantém o cursor físico parado no centro; só o delta interessa.
            // SAFETY: SetCursorPos não tem pré-condições de memória.
            let _ = unsafe { SetCursorPos(cx, cy) };
            InputEvent::Move { dx, dy }
        }
        WM_LBUTTONDOWN => button(PointerButton::Left, ButtonState::Down),
        WM_LBUTTONUP => button(PointerButton::Left, ButtonState::Up),
        WM_RBUTTONDOWN => button(PointerButton::Right, ButtonState::Down),
        WM_RBUTTONUP => button(PointerButton::Right, ButtonState::Up),
        WM_MBUTTONDOWN => button(PointerButton::Middle, ButtonState::Down),
        WM_MBUTTONUP => button(PointerButton::Middle, ButtonState::Up),
        WM_XBUTTONDOWN | WM_XBUTTONUP => {
            let b = if hi == 1 {
                PointerButton::X1
            } else {
                PointerButton::X2
            };
            let s = if msg == WM_XBUTTONDOWN {
                ButtonState::Down
            } else {
                ButtonState::Up
            };
            button(b, s)
        }
        WM_MOUSEWHEEL => InputEvent::Scroll {
            dx: 0,
            dy: hi as i16 as i32,
        },
        WM_MOUSEHWHEEL => InputEvent::Scroll {
            dx: hi as i16 as i32,
            dy: 0,
        },
        _ => return false,
    };
    deliver(ev)
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let mode = if code == 0 { current_mode() } else { MODE_OFF };
    if mode != MODE_OFF {
        // SAFETY: para WH_KEYBOARD_LL com code == HC_ACTION, lparam aponta para um KBDLLHOOKSTRUCT válido.
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        const EXTENDED: u32 = 0x01;
        const INJECTED: u32 = 0x10;
        const UP: u32 = 0x80;
        let flags = kb.flags.0;
        if flags & INJECTED == 0 {
            let down = !matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);
            track_modifier(kb.vkCode as u16, down);
            if down && kb.vkCode == 0x7B && emergency_mods_down() {
                SHARED.mode.store(MODE_OFF, Ordering::Release);
                send_event(InputEvent::Emergency);
                return LRESULT(1);
            }
            if mode == MODE_BLOCK {
                if down {
                    takeover();
                }
                return LRESULT(1);
            }
            let ev = InputEvent::Key {
                scan_code: kb.scanCode as u16,
                extended: flags & EXTENDED != 0,
                state: if flags & UP != 0 || !down {
                    ButtonState::Up
                } else {
                    ButtonState::Down
                },
            };
            if deliver(ev) {
                return LRESULT(1);
            }
        }
    }
    // SAFETY: repassa os mesmos argumentos recebidos.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn track_modifier(vk: u16, down: bool) {
    if let Some((_, bit)) = MOD_VKS.iter().find(|(v, _)| *v == vk) {
        if down {
            SHARED.mods.fetch_or(*bit, Ordering::Relaxed);
        } else {
            SHARED.mods.fetch_and(!*bit, Ordering::Relaxed);
        }
    }
}

fn emergency_mods_down() -> bool {
    let m = SHARED.mods.load(Ordering::Relaxed);
    m & (M_LSHIFT | M_RSHIFT) != 0 && m & (M_LCTRL | M_RCTRL) != 0 && m & (M_LALT | M_RALT) != 0
}

fn async_down(vk: u16) -> bool {
    // SAFETY: GetAsyncKeyState não tem pré-condições de memória.
    unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 }
}

struct HookPair(HHOOK, HHOOK);

fn input_thread(ready: std::sync::mpsc::Sender<(u32, bool)>) {
    // SAFETY: chamadas Win32 sem pré-condições de memória além dos ponteiros locais válidos.
    unsafe {
        let mut msg = MSG::default();
        // Garante a criação da fila de mensagens antes de publicar o id da thread.
        let _ = PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE);
        let hotkey_ok = RegisterHotKey(
            None,
            HOTKEY_ID,
            MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT,
            VK_F12.0 as u32,
        )
        .is_ok();
        let _ = ready.send((GetCurrentThreadId(), hotkey_ok));

        let mut hooks: Option<HookPair> = None;
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            match msg.message {
                WM_HOTKEY => {
                    send_event(InputEvent::Emergency);
                }
                WM_ENABLE if hooks.is_none() => {
                    let hmod = GetModuleHandleW(None)
                        .ok()
                        .map(|m| windows::Win32::Foundation::HINSTANCE(m.0));
                    let m = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hmod, 0);
                    let k = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), hmod, 0);
                    match (m, k) {
                        (Ok(m), Ok(k)) => hooks = Some(HookPair(m, k)),
                        (m, k) => {
                            // Falha parcial: nunca ficar com um hook só.
                            SHARED.mode.store(MODE_OFF, Ordering::Release);
                            if let Ok(h) = m {
                                let _ = UnhookWindowsHookEx(h);
                            }
                            if let Ok(h) = k {
                                let _ = UnhookWindowsHookEx(h);
                            }
                            send_event(InputEvent::Emergency);
                        }
                    }
                }
                WM_DISABLE => {
                    if let Some(h) = hooks.take() {
                        let _ = UnhookWindowsHookEx(h.0);
                        let _ = UnhookWindowsHookEx(h.1);
                    }
                }
                _ => {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
        if let Some(h) = hooks.take() {
            let _ = UnhookWindowsHookEx(h.0);
            let _ = UnhookWindowsHookEx(h.1);
        }
        let _ = UnregisterHotKey(None, HOTKEY_ID);
    }
}

pub struct WinPlatform {
    thread_id: u32,
    join: Option<std::thread::JoinHandle<()>>,
    hotkey_registered: bool,
    high_res: bool,
    pressed_keys: HashSet<(u16, bool)>,
    pressed_buttons: HashSet<PointerButton>,
}

impl WinPlatform {
    /// Cria a plataforma. Eventos capturados e o atalho de emergência chegam em `events`.
    pub fn new(events: UnboundedSender<InputEvent>) -> std::io::Result<Self> {
        disable_power_throttling();
        *SHARED.tx.lock().unwrap_or_else(|e| e.into_inner()) = Some(events);
        SHARED.mode.store(MODE_OFF, Ordering::Release);
        let (rtx, rrx) = std::sync::mpsc::channel();
        let join = std::thread::Builder::new()
            .name("windows-flow-input".into())
            .spawn(move || input_thread(rtx))?;
        let (thread_id, hotkey_registered) = rrx
            .recv()
            .map_err(|_| std::io::Error::other("thread de entrada não iniciou"))?;
        Ok(Self {
            thread_id,
            join: Some(join),
            hotkey_registered,
            high_res: false,
            pressed_keys: HashSet::new(),
            pressed_buttons: HashSet::new(),
        })
    }

    /// Falso se outro programa já usa Ctrl+Alt+Shift+F12. O atalho continua
    /// funcionando durante a captura, pois também é detectado dentro do hook.
    pub fn hotkey_registered(&self) -> bool {
        self.hotkey_registered
    }

    fn post(&self, msg: u32) -> bool {
        // SAFETY: postar mensagem a uma thread própria não tem pré-condições de memória.
        unsafe { PostThreadMessageW(self.thread_id, msg, WPARAM(0), LPARAM(0)).is_ok() }
    }

    fn send(inputs: &[INPUT]) {
        // SAFETY: `inputs` é um slice válido de INPUT e o tamanho é o da estrutura.
        unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    }

    fn mouse_input(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: data,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn key_input(scan: u16, extended: bool, up: bool) -> INPUT {
        let mut flags = KEYEVENTF_SCANCODE;
        if extended {
            flags |= KEYEVENTF_EXTENDEDKEY;
        }
        if up {
            flags |= KEYEVENTF_KEYUP;
        }
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: scan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    fn button_flags(b: PointerButton, down: bool) -> (MOUSE_EVENT_FLAGS, u32) {
        match (b, down) {
            (PointerButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
            (PointerButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
            (PointerButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
            (PointerButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
            (PointerButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
            (PointerButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
            (PointerButton::X1, d) => (
                if d {
                    MOUSEEVENTF_XDOWN
                } else {
                    MOUSEEVENTF_XUP
                },
                1,
            ),
            (PointerButton::X2, d) => (
                if d {
                    MOUSEEVENTF_XDOWN
                } else {
                    MOUSEEVENTF_XUP
                },
                2,
            ),
        }
    }
}

impl Platform for WinPlatform {
    fn screen(&self) -> Rect {
        // SAFETY: GetSystemMetrics não tem pré-condições de memória.
        unsafe {
            let (l, t) = (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
            );
            Rect {
                left: l,
                top: t,
                right: l + GetSystemMetrics(SM_CXVIRTUALSCREEN),
                bottom: t + GetSystemMetrics(SM_CYVIRTUALSCREEN),
            }
        }
    }

    fn cursor(&self) -> Point {
        let mut p = POINT::default();
        // SAFETY: `p` é um POINT válido e gravável.
        let _ = unsafe { GetCursorPos(&mut p) };
        Point { x: p.x, y: p.y }
    }

    fn set_cursor(&mut self, p: Point) {
        // SAFETY: SetCursorPos não tem pré-condições de memória.
        let _ = unsafe { SetCursorPos(p.x, p.y) };
    }

    fn move_cursor(&mut self, p: Point) {
        let s = self.screen();
        let norm = |v: i32, lo: i32, len: i32| -> i32 {
            let span = (len - 1).max(1) as i64;
            (((v - lo) as i64 * 65_535 + span / 2) / span) as i32
        };
        let (nx, ny) = (norm(p.x, s.left, s.width()), norm(p.y, s.top, s.height()));
        Self::send(&[Self::mouse_input(
            nx,
            ny,
            0,
            MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        )]);
    }

    fn inject(&mut self, ev: &InputEvent) {
        match *ev {
            InputEvent::Button { button, state } => {
                let down = state == ButtonState::Down;
                let (flags, data) = Self::button_flags(button, down);
                Self::send(&[Self::mouse_input(0, 0, data, flags)]);
                if down {
                    self.pressed_buttons.insert(button);
                } else {
                    self.pressed_buttons.remove(&button);
                }
            }
            InputEvent::Scroll { dx, dy } => {
                if dy != 0 {
                    Self::send(&[Self::mouse_input(0, 0, dy as u32, MOUSEEVENTF_WHEEL)]);
                }
                if dx != 0 {
                    Self::send(&[Self::mouse_input(0, 0, dx as u32, MOUSEEVENTF_HWHEEL)]);
                }
            }
            InputEvent::Key {
                scan_code,
                extended,
                state,
            } => {
                let up = state == ButtonState::Up;
                Self::send(&[Self::key_input(scan_code, extended, up)]);
                if up {
                    self.pressed_keys.remove(&(scan_code, extended));
                } else {
                    self.pressed_keys.insert((scan_code, extended));
                }
            }
            InputEvent::Move { .. } | InputEvent::Emergency | InputEvent::Takeover => {}
        }
    }

    fn release_all(&mut self) {
        for b in std::mem::take(&mut self.pressed_buttons) {
            let (flags, data) = Self::button_flags(b, false);
            Self::send(&[Self::mouse_input(0, 0, data, flags)]);
        }
        for (scan, ext) in std::mem::take(&mut self.pressed_keys) {
            Self::send(&[Self::key_input(scan, ext, true)]);
        }
    }

    fn capture(&mut self, on: bool) {
        if on {
            let mut entry = 0u8;
            for (vk, bit) in MOD_VKS {
                if async_down(vk) {
                    entry |= bit;
                }
            }
            SHARED.entry_mods.store(entry, Ordering::Relaxed);
            SHARED.mods.store(entry, Ordering::Relaxed);
            let c = self.screen_primary_center();
            SHARED.cx.store(c.x, Ordering::Relaxed);
            SHARED.cy.store(c.y, Ordering::Relaxed);
            SHARED.alive_ms.store(now_ms(), Ordering::Relaxed);
            self.set_cursor(c);
            SHARED.mode.store(MODE_FORWARD, Ordering::Release);
            if !self.post(WM_ENABLE) {
                SHARED.mode.store(MODE_OFF, Ordering::Release);
            }
        } else {
            // Primeiro para de engolir (efeito imediato), depois remove os hooks.
            SHARED.mode.store(MODE_OFF, Ordering::Release);
            self.post(WM_DISABLE);
            // Modificadores que estavam pressionados na entrada e foram soltos
            // enquanto engolíamos: o sistema ainda os considera pressionados.
            let entry = SHARED.entry_mods.swap(0, Ordering::Relaxed);
            let now = SHARED.mods.load(Ordering::Relaxed);
            for (vk, bit) in MOD_VKS {
                if entry & bit != 0 && now & bit == 0 {
                    let input = INPUT {
                        r#type: INPUT_KEYBOARD,
                        Anonymous: INPUT_0 {
                            ki: KEYBDINPUT {
                                wVk: VIRTUAL_KEY(vk),
                                wScan: 0,
                                dwFlags: KEYEVENTF_KEYUP,
                                time: 0,
                                dwExtraInfo: 0,
                            },
                        },
                    };
                    Self::send(&[input]);
                }
            }
        }
    }

    fn block_local(&mut self, on: bool) {
        if on {
            let mut held = 0u8;
            for (vk, bit) in MOD_VKS {
                if async_down(vk) {
                    held |= bit;
                }
            }
            // Só para o atalho de emergência enxergar modificadores já pressionados.
            SHARED.mods.store(held, Ordering::Relaxed);
            SHARED.entry_mods.store(0, Ordering::Relaxed);
            SHARED.alive_ms.store(now_ms(), Ordering::Relaxed);
            SHARED.mode.store(MODE_BLOCK, Ordering::Release);
            if !self.post(WM_ENABLE) {
                SHARED.mode.store(MODE_OFF, Ordering::Release);
            }
        } else {
            SHARED.mode.store(MODE_OFF, Ordering::Release);
            self.post(WM_DISABLE);
        }
    }

    fn performance(&mut self, on: bool) {
        if on == self.high_res {
            return;
        }
        self.high_res = on;
        // SAFETY: pares timeBeginPeriod/timeEndPeriod com o mesmo valor.
        unsafe {
            if on {
                timeBeginPeriod(1);
            } else {
                timeEndPeriod(1);
            }
        }
    }

    fn keepalive(&self) {
        SHARED.alive_ms.store(now_ms(), Ordering::Relaxed);
    }
}

/// O Windows 11 reduz a prioridade e a resolução de timer de processos com janela
/// minimizada, o que atrasaria heartbeat e movimento. Este app precisa de timers pontuais.
fn disable_power_throttling() {
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED
            | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
        StateMask: 0,
    };
    // SAFETY: `state` é uma estrutura válida e o tamanho informado é o dela.
    unsafe {
        let _ = SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            &state as *const _ as *const std::ffi::c_void,
            size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
    }
}

impl WinPlatform {
    fn screen_primary_center(&self) -> Point {
        // SAFETY: GetSystemMetrics não tem pré-condições de memória.
        unsafe {
            Point {
                x: GetSystemMetrics(SM_CXSCREEN) / 2,
                y: GetSystemMetrics(SM_CYSCREEN) / 2,
            }
        }
    }
}

impl Drop for WinPlatform {
    fn drop(&mut self) {
        self.performance(false);
        SHARED.mode.store(MODE_OFF, Ordering::Release);
        // SAFETY: WM_QUIT à própria thread de entrada encerra o message loop.
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        *SHARED.tx.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

/// Captura um retângulo da tela (coordenadas físicas da tela virtual) como RGBA, linhas de
/// cima para baixo. Fica só em memória: é usado para distorcer a borda na animação de
/// transição e descartado em seguida. `None` se a captura falhar (ex.: conteúdo protegido).
pub fn capture_screen_rgba(x: i32, y: i32, w: i32, h: i32) -> Option<Vec<u8>> {
    if w <= 0 || h <= 0 || i64::from(w) * i64::from(h) > 16_000_000 {
        return None;
    }
    // SAFETY: handles GDI criados e liberados aqui; `buf` tem exatamente w*h*4 bytes.
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            return None;
        }
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, HGDIOBJ(bmp.0));
        let blt = BitBlt(
            mem,
            0,
            0,
            w,
            h,
            Some(screen),
            x,
            y,
            ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0),
        );
        let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // negativo: linhas de cima para baixo
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let lines = GetDIBits(
            mem,
            bmp,
            0,
            h as u32,
            Some(buf.as_mut_ptr().cast()),
            &mut bi,
            DIB_RGB_COLORS,
        );
        SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        if blt.is_err() || lines == 0 {
            return None;
        }
        for px in buf.as_chunks_mut::<4>().0.iter_mut() {
            px.swap(0, 2); // BGRA -> RGBA
            px[3] = 255;
        }
        Some(buf)
    }
}
