use crate::credentials;
use crate::imap_config;
use serde::{Deserialize, Serialize};
use std::env;
use std::io;
use std::path::PathBuf;

const APP_DIR: &str = "foxmail-rs";
const CREDENTIAL_DIR: &str = "credentials";
const ACCOUNT_FILE: &str = "account-config.dpapi";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
}

impl AccountConfig {
    /// 默认账号：取自 imap_config 常量（历史锁定值），密码留空待填。
    pub fn defaults() -> Self {
        Self {
            host: imap_config::HOST.to_owned(),
            port: imap_config::PORT,
            user: imap_config::USER.to_owned(),
            password: String::new(),
        }
    }
}

pub fn config_path() -> io::Result<PathBuf> {
    let base = env::var_os("LOCALAPPDATA")
        .or_else(|| env::var_os("APPDATA"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA/APPDATA 未设置"))?;
    Ok(PathBuf::from(base)
        .join(APP_DIR)
        .join(CREDENTIAL_DIR)
        .join(ACCOUNT_FILE))
}

/// DPAPI 加密整个账号 JSON（含密码）落盘。复用 credentials 的加密原语，
/// 与 imap-password.dpapi 共用同一 entropy 但文件名不同，互不干扰。
pub fn save(config: &AccountConfig) -> io::Result<()> {
    let path = config_path()?;
    let json = serde_json::to_vec(config).map_err(io::Error::other)?;
    credentials::save_password(&path, &json)
}

pub fn load() -> Result<Option<AccountConfig>, Box<dyn std::error::Error>> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let json = credentials::load_password(&path)?;
    let config: AccountConfig = serde_json::from_str(json.as_str())?;
    Ok(Some(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn defaults_match_imap_config_constants() {
        let defaults = AccountConfig::defaults();
        assert_eq!(defaults.host, imap_config::HOST);
        assert_eq!(defaults.port, imap_config::PORT);
        assert_eq!(defaults.user, imap_config::USER);
        assert!(defaults.password.is_empty());
    }

    #[test]
    fn account_config_roundtrips_through_dpapi() {
        // ponytail: 复用 credentials::save_password/load_password 做加密往返，
        // 避开真实 config_path()（写死 LOCALAPPDATA）以免污染本机凭据目录。
        let root = std::env::temp_dir().join(format!(
            "foxmail-rs-account-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let path: PathBuf = root.join("account.dpapi");

        let config = AccountConfig {
            host: "imap.example.com".into(),
            port: 993,
            user: "user@example.com".into(),
            password: "p@ssw0rd".into(),
        };
        let json = serde_json::to_vec(&config).unwrap();
        credentials::save_password(&path, &json).unwrap();
        let loaded = credentials::load_password(&path).unwrap();
        let parsed: AccountConfig = serde_json::from_str(&loaded).unwrap();
        assert_eq!(parsed, config);

        let _ = fs::remove_dir_all(&root);
    }
}
