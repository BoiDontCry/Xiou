//! Native Windows save-file dialog (IFileSaveDialog) for attachment export.

use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::path::PathBuf;
use windows::core::HSTRING;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::{
    FileSaveDialog, IFileSaveDialog, FILEOPENDIALOGOPTIONS, FOS_OVERWRITEPROMPT, SIGDN_FILESYSPATH,
};

/// Show a modal native "save as" dialog owned by the given window.
/// Returns the chosen path, or `None` if the user cancelled.
pub fn prompt_save_path(window: &Window, suggested_filename: &str) -> Option<PathBuf> {
    let hwnd = native_hwnd(window)?;
    // SAFETY: COM is initialized on this thread by GPUI (OleInitialize); dialog
    // shown modally against our own window handle.
    unsafe { show_dialog(hwnd, suggested_filename) }.ok()
}

fn native_hwnd(window: &Window) -> Option<HWND> {
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
