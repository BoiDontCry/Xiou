use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{LocalFree, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};
use windows_sys::Win32::System::Console::{
    GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_ECHO_INPUT, STD_INPUT_HANDLE,
};
use zeroize::{Zeroize, Zeroizing};

const APP_DIR: &str = "foxmail-rs";
const CREDENTIAL_DIR: &str = "credentials";
const CREDENTIAL_FILE: &str = "imap-password.dpapi";
const ENV_PASSWORD: &str = "FOXMAIL_IMAP_PASSWORD";
const ENTROPY: &[u8] = b"foxmail-rs/imap-password/v1";

pub fn credential_path() -> io::Result<PathBuf> {
    let base = env::var_os("LOCALAPPDATA")
        .or_else(|| env::var_os("APPDATA"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA/APPDATA 未设置"))?;
    Ok(PathBuf::from(base)
        .join(APP_DIR)
        .join(CREDENTIAL_DIR)
        .join(CREDENTIAL_FILE))
}

pub fn load_or_initialize(reset: bool) -> Result<Zeroizing<String>, Box<dyn std::error::Error>> {
    if let Ok(value) = env::var(ENV_PASSWORD) {
        if value.is_empty() {
            return Err(format!("环境变量 {ENV_PASSWORD} 不能为空").into());
        }
        return Ok(Zeroizing::new(value));
    }

    let path = credential_path()?;
    if reset || !path.exists() {
        let password = prompt_password("请输入邮箱密码（输入不会回显）：")?;
        save_password(&path, password.as_bytes())?;
        println!("[凭据] 已使用 Windows DPAPI 保存到当前用户安全存储。")
    }
    load_password(&path).map_err(|error| {
        format!(
            "无法读取 Windows DPAPI 凭据：{error}。请使用 --reset-password 在本机重新输入密码。"
        )
        .into()
    })
}

pub fn save_password(path: &Path, password: &[u8]) -> io::Result<()> {
    if password.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "密码不能为空"));
    }
    let encrypted = protect(password)?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "凭据路径缺少父目录"))?;
    fs::create_dir_all(parent)?;

    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = File::create(&temp)?;
    file.write_all(&encrypted)?;
    file.sync_all()?;
    drop(file);

    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temp, path)?;
    Ok(())
}

pub fn load_password(path: &Path) -> Result<Zeroizing<String>, Box<dyn std::error::Error>> {
    let mut encrypted = Vec::new();
    File::open(path)?.read_to_end(&mut encrypted)?;
    if encrypted.is_empty() {
        return Err("DPAPI 凭据文件为空".into());
    }

    let decrypted = unprotect(&encrypted)?;
    let password = String::from_utf8(decrypted)?;
    if password.is_empty() {
        return Err("DPAPI 解密结果为空".into());
    }
    Ok(Zeroizing::new(password))
}

fn protect(data: &[u8]) -> io::Result<Vec<u8>> {
    let input = blob(data)?;
    let entropy = blob(ENTROPY)?;
    let mut output = CRYPT_INTEGER_BLOB::default();

    let ok = unsafe {
        CryptProtectData(
            &input,
            null(),
            &entropy,
            null(),
            null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }

    copy_and_free(output)
}

fn unprotect(data: &[u8]) -> io::Result<Vec<u8>> {
    let input = blob(data)?;
    let entropy = blob(ENTROPY)?;
    let mut output = CRYPT_INTEGER_BLOB::default();

    let ok = unsafe {
        CryptUnprotectData(
            &input,
            null_mut(),
            &entropy,
            null(),
            null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }

    copy_and_free(output)
}

fn blob(data: &[u8]) -> io::Result<CRYPT_INTEGER_BLOB> {
    let len = u32::try_from(data.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "数据超过 DPAPI 长度上限"))?;
    Ok(CRYPT_INTEGER_BLOB {
        cbData: len,
        pbData: data.as_ptr() as *mut u8,
    })
}

fn copy_and_free(output: CRYPT_INTEGER_BLOB) -> io::Result<Vec<u8>> {
    if output.pbData.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "DPAPI 返回空指针",
        ));
    }

    let bytes =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
    unsafe {
        LocalFree(output.pbData as _);
    }
    Ok(bytes)
}

fn prompt_password(prompt: &str) -> io::Result<Zeroizing<String>> {
    print!("{prompt}");
    io::stdout().flush()?;

    let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }

    let mut original_mode = 0;
    if unsafe { GetConsoleMode(handle, &mut original_mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { SetConsoleMode(handle, original_mode & !ENABLE_ECHO_INPUT) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let _guard = ConsoleEchoGuard {
        handle,
        original_mode,
    };

    let mut password = Zeroizing::new(String::new());
    let read_result = io::stdin().read_line(&mut password);
    println!();
    read_result?;

    while matches!(password.chars().last(), Some('\r' | '\n')) {
        password.pop();
    }
    if password.is_empty() {
        password.zeroize();
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "密码不能为空"));
    }
    Ok(password)
}

struct ConsoleEchoGuard {
    handle: HANDLE,
    original_mode: u32,
}

impl Drop for ConsoleEchoGuard {
    fn drop(&mut self) {
        unsafe {
            SetConsoleMode(self.handle, self.original_mode);
        }
    }
}
