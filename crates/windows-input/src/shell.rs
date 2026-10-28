//! Integração com o shell do Windows: pasta Downloads e arrastar arquivos (OLE) para fora
//! do app, como o Explorer faz, para qualquer janela que aceite soltar arquivos.

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::core::{implement, BOOL, HRESULT, PCWSTR};
use windows::Win32::Foundation::{
    DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, S_OK,
};
use windows::Win32::System::Com::{CoTaskMemFree, IDataObject};
use windows::Win32::System::Ole::{
    DoDragDrop, IDropSource, IDropSource_Impl, OleInitialize, DROPEFFECT, DROPEFFECT_COPY,
    DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MK_LBUTTON;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    FOLDERID_Downloads, SHCreateDataObject, SHGetKnownFolderPath, SHParseDisplayName,
    KF_FLAG_DEFAULT,
};

/// Pasta Downloads do usuário (respeita redirecionamentos); cai em `%USERPROFILE%\Downloads`.
pub fn downloads_dir() -> PathBuf {
    // SAFETY: o PWSTR devolvido é lido até o terminador e liberado com CoTaskMemFree.
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(&FOLDERID_Downloads, KF_FLAG_DEFAULT, None) {
            let s = p.to_string();
            CoTaskMemFree(Some(p.0.cast()));
            if let Ok(s) = s {
                if !s.is_empty() {
                    return PathBuf::from(s);
                }
            }
        }
    }
    std::env::var_os("USERPROFILE")
        .map(|h| PathBuf::from(h).join("Downloads"))
        .unwrap_or_else(|| PathBuf::from("Downloads"))
}

#[implement(IDropSource)]
struct Source;

#[allow(non_snake_case)]
impl IDropSource_Impl for Source_Impl {
    fn QueryContinueDrag(
        &self,
        fescapepressed: BOOL,
        grfkeystate: windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS,
    ) -> HRESULT {
        if fescapepressed.as_bool() {
            DRAGDROP_S_CANCEL
        } else if grfkeystate.0 & MK_LBUTTON.0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _dweffect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Começa um arrastar-e-soltar nativo dos `paths` (o botão esquerdo precisa estar pressionado).
/// Bloqueia até o usuário soltar; `true` se algum destino aceitou os arquivos.
pub fn start_file_drag(paths: &[PathBuf]) -> bool {
    if paths.is_empty() {
        return false;
    }
    // SAFETY: PIDLs criados por SHParseDisplayName são liberados com CoTaskMemFree depois de
    // usados; os ponteiros só vivem durante esta função.
    unsafe {
        let _ = OleInitialize(None);
        let mut pidls: Vec<*mut ITEMIDLIST> = Vec::new();
        for p in paths {
            let wide: Vec<u16> = Path::new(p)
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let mut pidl = std::ptr::null_mut();
            if SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None).is_ok()
                && !pidl.is_null()
            {
                pidls.push(pidl);
            }
        }
        let consts: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| p.cast_const()).collect();
        let data = if consts.is_empty() {
            None
        } else {
            SHCreateDataObject::<Option<&IDataObject>, IDataObject>(None, Some(&consts), None).ok()
        };
        for p in &pidls {
            CoTaskMemFree(Some(p.cast_const().cast()));
        }
        let Some(data) = data else {
            return false;
        };
        let source: IDropSource = Source.into();
        let mut effect = DROPEFFECT_NONE;
        let hr = DoDragDrop(
            &data,
            &source,
            DROPEFFECT(DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0 | DROPEFFECT_LINK.0),
            &mut effect,
        );
        hr == DRAGDROP_S_DROP && effect != DROPEFFECT_NONE
    }
}

// ---- janela nativa que recebe arquivos arrastados ------------------------------------------

use std::sync::mpsc;

use windows::core::{w, Ref};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINTL, WPARAM};
use windows::Win32::Graphics::Gdi::{GetStockObject, BLACK_BRUSH, HBRUSH};
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium, CF_HDROP,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW,
    SetLayeredWindowAttributes, SetWindowPos, ShowWindow, HWND_TOPMOST, LWA_ALPHA, MSG,
    SWP_NOACTIVATE, SW_HIDE, SW_SHOWNOACTIVATE, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

/// O que a janela de captura observa de um arrasto de arquivos.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropEvent {
    /// Arquivos entraram na janela.
    Enter(Vec<PathBuf>),
    /// Saíram sem soltar.
    Leave,
    /// Soltos sobre a janela.
    Drop,
}

type DropCallback = Box<dyn Fn(DropEvent) + Send + Sync>;

#[implement(IDropTarget)]
struct Target {
    cb: DropCallback,
}

fn read_paths(data: &IDataObject) -> Vec<PathBuf> {
    let fmt = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let mut out = Vec::new();
    // SAFETY: o HDROP pertence ao STGMEDIUM devolvido, liberado com ReleaseStgMedium; os
    // buffers têm o tamanho pedido ao próprio DragQueryFileW.
    unsafe {
        let Ok(mut med) = data.GetData(&fmt) else {
            return out;
        };
        let hdrop = HDROP(med.u.hGlobal.0);
        let n = DragQueryFileW(hdrop, u32::MAX, None);
        for i in 0..n.min(512) {
            let len = DragQueryFileW(hdrop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            let got = DragQueryFileW(hdrop, i, Some(&mut buf)) as usize;
            out.push(PathBuf::from(String::from_utf16_lossy(
                &buf[..got.min(len)],
            )));
        }
        ReleaseStgMedium(&mut med);
    }
    out
}

#[allow(non_snake_case)]
impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(
        &self,
        pdataobj: Ref<'_, IDataObject>,
        _key: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let paths = pdataobj.as_ref().map(read_paths).unwrap_or_default();
        // SAFETY: o OLE passa um ponteiro válido para o efeito.
        unsafe {
            *pdweffect = if paths.is_empty() {
                DROPEFFECT_NONE
            } else {
                // Só "copiar": o arquivo de origem nunca é movido nem apagado.
                DROPEFFECT_COPY
            };
        }
        if !paths.is_empty() {
            (self.cb)(DropEvent::Enter(paths));
        }
        Ok(())
    }

    fn DragOver(
        &self,
        _key: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        // SAFETY: ponteiro válido fornecido pelo OLE.
        unsafe { *pdweffect = DROPEFFECT_COPY };
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        (self.cb)(DropEvent::Leave);
        Ok(())
    }

    fn Drop(
        &self,
        _pdataobj: Ref<'_, IDataObject>,
        _key: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        pdweffect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        // SAFETY: ponteiro válido fornecido pelo OLE.
        unsafe { *pdweffect = DROPEFFECT_COPY };
        (self.cb)(DropEvent::Drop);
        Ok(())
    }
}

unsafe extern "system" fn catcher_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // SAFETY: repassa a mensagem sem tocar em memória.
    unsafe { DefWindowProcW(h, m, w, l) }
}

/// Janela nativa quase invisível (alfa 1/255, mas que recebe o mouse) registrada como alvo de
/// arrastar-e-soltar. O WebView2 engole o arrasto de arquivos antes de chegar ao app, então a
/// captura é feita por uma janela Win32 própria, como o Explorer faz.
pub struct DropCatcher {
    hwnd: isize,
}

impl DropCatcher {
    pub fn new(cb: impl Fn(DropEvent) + Send + Sync + 'static) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Option<isize>>();
        let cb: DropCallback = Box::new(cb);
        std::thread::Builder::new()
            .name("drop-catcher".into())
            .spawn(move || {
                // SAFETY: janela, classe e alvo OLE são criados e usados só nesta thread, que
                // também roda o loop de mensagens exigido pelo OLE.
                unsafe {
                    let _ = OleInitialize(None);
                    let class = w!("WindowsFlowDropCatcher");
                    let hi = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                        .unwrap_or_default();
                    let wc = WNDCLASSW {
                        lpfnWndProc: Some(catcher_proc),
                        hInstance: hi.into(),
                        lpszClassName: class,
                        hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                        ..Default::default()
                    };
                    RegisterClassW(&wc);
                    let hwnd = CreateWindowExW(
                        WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_LAYERED,
                        class,
                        w!("Windows Flow Drop"),
                        WS_POPUP,
                        0,
                        0,
                        10,
                        10,
                        None,
                        None,
                        Some(hi.into()),
                        None,
                    );
                    let Ok(hwnd) = hwnd else {
                        let _ = tx.send(None);
                        return;
                    };
                    let _ = SetLayeredWindowAttributes(hwnd, Default::default(), 1, LWA_ALPHA);
                    let target: IDropTarget = Target { cb }.into();
                    if RegisterDragDrop(hwnd, &target).is_err() {
                        let _ = tx.send(None);
                        return;
                    }
                    let _ = tx.send(Some(hwnd.0 as isize));
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                        DispatchMessageW(&msg);
                    }
                }
            })?;
        match rx.recv() {
            Ok(Some(hwnd)) => Ok(Self { hwnd }),
            _ => Err(std::io::Error::other("janela de captura não criada")),
        }
    }

    fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut _)
    }

    /// Mostra a janela (sem ativar) no retângulo físico dado, sempre no topo.
    pub fn place(&self, x: i32, y: i32, w: i32, h: i32) {
        // SAFETY: HWND criado por `new` e vivo durante todo o processo.
        unsafe {
            let _ = SetWindowPos(
                self.hwnd(),
                Some(HWND_TOPMOST),
                x,
                y,
                w.max(1),
                h.max(1),
                SWP_NOACTIVATE,
            );
            let _ = ShowWindow(self.hwnd(), SW_SHOWNOACTIVATE);
        }
    }

    pub fn hide(&self) {
        // SAFETY: HWND criado por `new` e vivo durante todo o processo.
        unsafe {
            let _ = ShowWindow(self.hwnd(), SW_HIDE);
        }
    }
}

/// Abre o arquivo (ou pasta) com o programa padrão do Windows, como um clique duplo no Explorer.
pub fn open_path(path: &Path) -> bool {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` termina em zero e vive durante a chamada.
    let r = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    r.0 as usize > 32
}

/// Abre o Explorer na pasta do item, com ele selecionado.
pub fn reveal_path(path: &Path) -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{}\"", path.display()))
        .spawn()
        .is_ok()
}
