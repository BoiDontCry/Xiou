//! Windows single-instance guard for the desktop application.

const WINDOW_TITLE: &str = "Foxmail Reader";
const MUTEX_NAME: &str = r"Local\Foxmail.Reader.SingleInstance";

pub enum AcquireResult {
    Acquired(SingleInstance),
    AlreadyRunning,
    Unavailable,
}

pub struct SingleInstance {
    #[cfg(windows)]
    handle: Option<windows_sys::Win32::Foundation::HANDLE>,
}

impl SingleInstance {
    pub(crate) fn noop() -> Self {
        #[cfg(windows)]
        {
            return Self { handle: None };
        }

        #[cfg(not(windows))]
        {
            Self {}
        }
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        #[cfg(windows)]
        if let Some(handle) = self.handle.take() {
            unsafe {
                let _ = windows_sys::Win32::Foundation::CloseHandle(handle);
            }
        }
    }
}

pub fn acquire() -> AcquireResult {
    #[cfg(windows)]
    {
        return acquire_windows();
    }

    #[cfg(not(windows))]
    {
        AcquireResult::Acquired(SingleInstance::noop())
    }
}

#[cfg(windows)]
fn acquire_windows() -> AcquireResult {
    use std::ptr::null;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;

    let mutex_name = wide(MUTEX_NAME);
    let handle = unsafe { CreateMutexW(null(), 0, mutex_name.as_ptr()) };
    if handle.is_null() {
        return AcquireResult::Unavailable;
    }

    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            let _ = CloseHandle(handle);
        }
        focus_existing_window();
        AcquireResult::AlreadyRunning
    } else {
        AcquireResult::Acquired(SingleInstance {
            handle: Some(handle),
        })
    }
}

#[cfg(windows)]
fn focus_existing_window() {
    use std::ptr::null;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE,
    };

    let title = wide(WINDOW_TITLE);
    let hwnd = unsafe { FindWindowW(null(), title.as_ptr()) };
    if !hwnd.is_null() {
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
