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
