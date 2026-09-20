//! Native Windows save-file dialog (IFileSaveDialog) for attachment export.

use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::path::PathBuf;
use windows::core::HSTRING;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{
    FileSaveDialog, IFileSaveDialog, FILEOPENDIALOGOPTIONS, FOS_OVERWRITEPROMPT, SIGDN_FILESYSPATH,
};

/// Show a modal native "save as" dialog owned by the given window.
/// `Ok(None)` = user cancelled; `Err` = the dialog genuinely failed (HRESULT
/// included) — callers must surface this instead of treating it as a cancel.
pub fn prompt_save_path(
    window: &Window,
    suggested_filename: &str,
) -> Result<Option<PathBuf>, String> {
    let Some(hwnd) = native_hwnd(window) else {
        return Err("无法获取主窗口句柄，无法打开保存对话框".into());
    };
    // Self-guard STA: GPUI initializes OLE on the UI thread (OleInitialize,
    // STA-compliant); S_FALSE/RPC_E_CHANGED_MODE must not be silently swallowed.
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let owns_init = hr.is_ok();
    let result = unsafe { show_dialog(hwnd, suggested_filename) };
    if owns_init {
        unsafe { CoUninitialize() };
    }
    match result {
        Ok(path) => Ok(Some(path)),
        // HRESULT_FROM_WIN32(ERROR_CANCELLED) = 0x800704C7
        Err(error) if error.code().0 == 0x8007_04C7u32 as i32 => Ok(None),
        Err(error) => Err(format!(
            "保存对话框打开失败（HRESULT {:08X}）：{error}",
            error.code().0 as u32
        )),
    }
}

pub(crate) fn native_hwnd(window: &Window) -> Option<HWND> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    if let RawWindowHandle::Win32(win32) = handle.as_raw() {
        Some(HWND(win32.hwnd.get() as *mut _))
    } else {
        None
    }
}

unsafe fn show_dialog(owner: HWND, suggested_filename: &str) -> windows::core::Result<PathBuf> {
    let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
    dialog.SetOptions(FILEOPENDIALOGOPTIONS(FOS_OVERWRITEPROMPT.0))?;
    if !suggested_filename.is_empty() {
        dialog.SetFileName(&HSTRING::from(suggested_filename))?;
    }
    dialog.Show(Some(owner))?;
    let shell_item = dialog.GetResult()?;
    let path = shell_item.GetDisplayName(SIGDN_FILESYSPATH)?;
    Ok(PathBuf::from(path.to_string()?))
}
