use crate::credentials;
use serde::{Deserialize, Serialize};
use std::env;
use std::fmt;
use std::io;
use std::path::PathBuf;

const APP_DIR: &str = "foxmail-rs";
const CREDENTIAL_DIR: &str = "credentials";
const ACCOUNT_FILE: &str = "account-config.dpapi";

/// 当前配置结构版本。读取时版本不匹配明确报错，不猜测。
pub const SCHEMA_VERSION: u32 = 2;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImapConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub security: SmtpSecurity,
    pub auth_user: String,
    pub password: String,
    pub from_address: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SmtpSecurity {
    /// 465 隐式 TLS。
    #[serde(rename = "implicit-tls")]
    ImplicitTls,
    /// 587 STARTTLS。
    #[serde(rename = "starttls")]
    StartTls,
}

#[derive(Serialize)]
pub struct AccountConfig {
    pub schema_version: u32,
    pub incoming: ImapConfig,
    pub outgoing: Option<SmtpConfig>,
}

impl AccountConfig {
    /// 空白配置：预填通用标准端口，不预填任何服务器域名或邮箱地址。
    pub fn blank() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            incoming: ImapConfig {
                host: String::new(),
                port: 993,
                user: String::new(),
                password: String::new(),
            },
            outgoing: None,
        }
    }

    /// 新版结构解析失败后回退旧扁平结构（host/port/user/password）并迁移：
    /// 旧字段映射为 `incoming`，`outgoing` 保持 `None`（不按域名猜测 SMTP）。
    pub fn parse(json: &str) -> Result<Self, String> {
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|error| format!("账号配置 JSON 无效：{error}"))?;
        if let Ok(new) = serde_json::from_value::<NewShape>(value.clone()) {
            if new.schema_version == SCHEMA_VERSION {
                return Ok(Self {
                    schema_version: new.schema_version,
                    incoming: new.incoming,
                    outgoing: new.outgoing,
                });
            }
            return Err(format!(
                "不支持的账号配置版本 {}（当前支持 {SCHEMA_VERSION}，请更新程序）",
                new.schema_version
            ));
        }
        if let Ok(legacy) = serde_json::from_value::<LegacyShape>(value) {
            return Ok(Self::migrate_legacy(legacy));
        }
        Err("无法识别账号配置格式".to_owned())
    }

    /// 旧配置迁移：仅收信设置，不虚构发件配置。
    fn migrate_legacy(legacy: LegacyShape) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            incoming: ImapConfig {
                host: legacy.host,
                port: legacy.port,
                user: legacy.user,
                password: legacy.password,
            },
            outgoing: None,
        }
    }

    /// 发送前完整性校验；错误信息不回显任何密码。
    pub fn validate(&self) -> Result<(), String> {
        if self.incoming.host.trim().is_empty() {
            return Err("收信服务器地址不能为空".to_owned());
        }
        if self.incoming.user.trim().is_empty() {
            return Err("收信账号不能为空".to_owned());
        }
        if self.incoming.port == 0 {
            return Err("收信端口不能为 0".to_owned());
        }
        let Some(smtp) = &self.outgoing else {
            return Ok(());
        };
        if smtp.host.trim().is_empty() {
            return Err("发信服务器地址不能为空".to_owned());
        }
        if smtp.port == 0 {
            return Err("发信端口不能为 0".to_owned());
        }
        if smtp.auth_user.trim().is_empty() {
            return Err("发信认证用户名不能为空".to_owned());
        }
        if smtp.password.is_empty() {
            return Err("发信密码不能为空".to_owned());
        }
        if smtp.from_address.trim().is_empty() {
            return Err("发件地址不能为空".to_owned());
        }
        smtp.from_address
            .parse::<lettre::message::Mailbox>()
            .map_err(|error| format!("发件地址无效：{error}"))?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct NewShape {
    schema_version: u32,
    incoming: ImapConfig,
    outgoing: Option<SmtpConfig>,
}

#[derive(Deserialize)]
struct LegacyShape {
    host: String,
    port: u16,
    user: String,
    password: String,
}

/// 脱敏：日志与错误信息中只出现占位符，不出现密码值。
fn mask(value: &str) -> &'static str {
    if value.is_empty() {
        "<空>"
    } else {
        "<已隐藏>"
    }
}

impl fmt::Debug for ImapConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImapConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("password", &mask(&self.password))
            .finish()
    }
}

impl fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SmtpConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("security", &self.security)
            .field("auth_user", &self.auth_user)
            .field("password", &mask(&self.password))
            .field("from_address", &self.from_address)
            .finish()
    }
}

impl fmt::Debug for AccountConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccountConfig")
            .field("schema_version", &self.schema_version)
            .field("incoming", &self.incoming)
            .field("outgoing", &self.outgoing)
            .finish()
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

/// DPAPI 加密整个账号 JSON（含两套密码）落盘。复用 credentials 的加密原语。
pub fn save(config: &AccountConfig) -> io::Result<()> {
    let path = config_path()?;
    let json = serde_json::to_vec(config).map_err(io::Error::other)?;
    credentials::save_password(&path, &json)
}

/// 读取并解析；旧扁平配置自动迁移为收信-only，不悄悄覆盖原文件。
pub fn load() -> Result<Option<AccountConfig>, String> {
    let path = config_path().map_err(|error| error.to_string())?;
    if !path.exists() {
        return Ok(None);
    }
    let json = credentials::load_password(&path).map_err(|error| error.to_string())?;
    match AccountConfig::parse(&json) {
        Ok(config) => Ok(Some(config)),
        Err(message) => Err(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_incoming() -> ImapConfig {
        ImapConfig {
            host: "imap.example.com".into(),
            port: 993,
            user: "user@example.com".into(),
            password: "secret-imap".into(),
        }
    }

    fn sample_smtp() -> SmtpConfig {
        SmtpConfig {
            host: "smtp.example.com".into(),
            port: 465,
            security: SmtpSecurity::ImplicitTls,
            auth_user: "user@example.com".into(),
            password: "secret-smtp".into(),
            from_address: "user@example.com".into(),
        }
    }

    #[test]
    fn blank_config_prefills_only_the_standard_port() {
        let config = AccountConfig::blank();
        assert_eq!(config.incoming.port, 993);
        assert!(config.incoming.host.is_empty());
        assert!(config.incoming.user.is_empty());
        assert!(config.outgoing.is_none());
        assert_eq!(config.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn new_config_roundtrips_through_json() {
        let config = AccountConfig {
            schema_version: SCHEMA_VERSION,
            incoming: sample_incoming(),
            outgoing: Some(sample_smtp()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed = AccountConfig::parse(&json).unwrap();
        assert_eq!(parsed.incoming, config.incoming);
        assert_eq!(parsed.outgoing, config.outgoing);
        assert_eq!(parsed.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn legacy_flat_config_migrates_to_imap_only_without_guessing_smtp() {
        let legacy = r#"{"host":"imap.example.com","port":993,"user":"user@example.com","password":"secret"}"#;
        let parsed = AccountConfig::parse(legacy).unwrap();
        assert_eq!(parsed.incoming.host, "imap.example.com");
        assert_eq!(parsed.incoming.port, 993);
        assert_eq!(parsed.incoming.user, "user@example.com");
        assert_eq!(parsed.incoming.password, "secret");
        assert!(
            parsed.outgoing.is_none(),
            "未知旧账号不得获得虚构 SMTP 配置"
        );
        assert_eq!(parsed.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn unknown_schema_version_is_rejected_not_degraded() {
        let future = r#"{"schema_version":99,"incoming":{"host":"h","port":993,"user":"u","password":"p"},"outgoing":null}"#;
        let error = AccountConfig::parse(future).unwrap_err();
        assert!(error.contains("不支持的账号配置版本 99"), "{error}");
    }

    #[test]
    fn unrecognized_json_is_rejected_with_clear_error() {
        let error = AccountConfig::parse(r#"{"foo":1}"#).unwrap_err();
        assert!(error.contains("无法识别账号配置格式"), "{error}");
    }

    #[test]
    fn broken_json_reports_reason() {
        let error = AccountConfig::parse("{not json").unwrap_err();
        assert!(error.contains("账号配置 JSON 无效"), "{error}");
    }

    #[test]
    fn debug_output_never_contains_passwords() {
        let config = AccountConfig {
            schema_version: SCHEMA_VERSION,
            incoming: sample_incoming(),
            outgoing: Some(sample_smtp()),
        };
        let text = format!("{config:?}");
        assert!(!text.contains("secret-imap"), "{text}");
        assert!(!text.contains("secret-smtp"), "{text}");
        assert!(text.contains("<已隐藏>"), "{text}");
    }

    #[test]
    fn validation_accepts_complete_config_and_rejects_missing_fields() {
        let mut config = AccountConfig {
            schema_version: SCHEMA_VERSION,
            incoming: sample_incoming(),
            outgoing: Some(sample_smtp()),
        };
        config.validate().unwrap();

        config.outgoing.as_mut().unwrap().host = String::new();
        assert!(config.validate().unwrap_err().contains("发信服务器地址"));

        let mut config = AccountConfig {
            schema_version: SCHEMA_VERSION,
            incoming: sample_incoming(),
            outgoing: Some(sample_smtp()),
        };
        config.outgoing.as_mut().unwrap().from_address = "not-an-address".into();
        assert!(config.validate().unwrap_err().contains("发件地址无效"));

        let imap_only = AccountConfig {
            schema_version: SCHEMA_VERSION,
            incoming: sample_incoming(),
            outgoing: None,
        };
        imap_only.validate().unwrap();

        let mut config = imap_only;
        config.incoming.host = String::new();
        assert!(config.validate().unwrap_err().contains("收信服务器地址"));
    }

    #[test]
    fn validation_errors_never_echo_password_values() {
        let mut config = AccountConfig {
            schema_version: SCHEMA_VERSION,
            incoming: sample_incoming(),
            outgoing: Some(sample_smtp()),
        };
        config.outgoing.as_mut().unwrap().auth_user = String::new();
        let error = config.validate().unwrap_err();
        assert!(!error.contains("secret"), "{error}");
        assert!(!error.contains("p@ss"), "{error}");
    }

    #[test]
    fn smtp_security_serializes_to_stable_names() {
        assert_eq!(
            serde_json::to_string(&SmtpSecurity::ImplicitTls).unwrap(),
            r#""implicit-tls""#
        );
        assert_eq!(
            serde_json::to_string(&SmtpSecurity::StartTls).unwrap(),
            r#""starttls""#
        );
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
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("account.dpapi");

        let config = AccountConfig {
            schema_version: SCHEMA_VERSION,
            incoming: sample_incoming(),
            outgoing: Some(sample_smtp()),
        };
        let json = serde_json::to_vec(&config).unwrap();
        credentials::save_password(&path, &json).unwrap();
        let loaded = credentials::load_password(&path).unwrap();
        let parsed = AccountConfig::parse(&loaded).unwrap();
        assert_eq!(parsed.incoming, config.incoming);
        assert_eq!(parsed.outgoing, config.outgoing);

        let _ = std::fs::remove_dir_all(&root);
    }
}
