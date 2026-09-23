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
/// v3：多账号列表；v2/旧扁平为单账号（读取时自动升级为单元素列表）。
pub const SCHEMA_VERSION: u32 = 3;

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

#[derive(Clone, Serialize, Deserialize)]
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

    /// 账号身份键：收信账号 + 服务器，用于去重与查找。
    pub fn identity(&self) -> (String, String) {
        (
            self.incoming.user.trim().to_owned(),
            self.incoming.host.trim().to_owned(),
        )
    }

    /// 兼容旧单账号 API：取列表第一个账号（读取时 v2/扁平自动升级为单元素列表）。
    pub fn parse(json: &str) -> Result<Self, String> {
        Ok(parse_list(json)?
            .into_iter()
            .next()
            .unwrap_or_else(Self::blank))
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
struct ListShape {
    schema_version: u32,
    accounts: Vec<AccountConfig>,
}

/// 解析多账号配置：v3 直接用；v2/旧扁平升级为单元素列表。
/// 版本不匹配明确报错，不猜测。
pub fn parse_list(json: &str) -> Result<Vec<AccountConfig>, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|error| format!("账号配置 JSON 无效：{error}"))?;
    if let Ok(v3) = serde_json::from_value::<ListShape>(value.clone()) {
        if v3.schema_version == SCHEMA_VERSION {
            return Ok(v3.accounts);
        }
        return Err(format!(
            "不支持的账号配置版本 {}（当前支持 {SCHEMA_VERSION}，请更新程序）",
            v3.schema_version
        ));
    }
    if let Ok(v2) = serde_json::from_value::<NewShape>(value.clone()) {
        if v2.schema_version == 2 || v2.schema_version == SCHEMA_VERSION {
            return Ok(vec![AccountConfig {
                schema_version: SCHEMA_VERSION,
                incoming: v2.incoming,
                outgoing: v2.outgoing,
            }]);
        }
        return Err(format!(
            "不支持的账号配置版本 {}（当前支持 {SCHEMA_VERSION}，请更新程序）",
            v2.schema_version
        ));
    }
    if let Ok(legacy) = serde_json::from_value::<LegacyShape>(value) {
        return Ok(vec![AccountConfig {
            schema_version: SCHEMA_VERSION,
            incoming: ImapConfig {
                host: legacy.host,
                port: legacy.port,
                user: legacy.user,
                password: legacy.password,
            },
            outgoing: None,
        }]);
    }
    Err("无法识别账号配置格式".to_owned())
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

/// DPAPI 加密整个账号列表 JSON（含所有密码）落盘。复用 credentials 的加密原语。
pub fn save_all(accounts: &[AccountConfig]) -> io::Result<()> {
    save_all_to(&config_path()?, accounts)
}

fn save_all_to(path: &std::path::Path, accounts: &[AccountConfig]) -> io::Result<()> {
    let shape = ListShape {
        schema_version: SCHEMA_VERSION,
        accounts: accounts.to_vec(),
    };
    let json = serde_json::to_vec(&shape).map_err(io::Error::other)?;
    credentials::save_password(path, &json)
}

/// 兼容旧单账号保存：整体覆盖为单账号列表。
pub fn save(config: &AccountConfig) -> io::Result<()> {
    save_all(std::slice::from_ref(config))
}

/// 读取账号列表；旧 v2/扁平配置自动升级为单元素列表，不悄悄覆盖原文件。
pub fn list() -> Result<Vec<AccountConfig>, String> {
    list_from(&config_path().map_err(|error| error.to_string())?)
}

fn list_from(path: &std::path::Path) -> Result<Vec<AccountConfig>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    credentials::load_password(path)
        .map_err(|error| explain_credential_error(&error.to_string()))
        .and_then(|json| parse_list(&json))
}

/// DPAPI 失败多见于系统重装/换电脑（当前 Windows 用户与加密时不同）。
/// 给 GUI 用户可执行的恢复路径，而不是 CLI 参数。
fn explain_credential_error(message: &str) -> String {
    if message.contains("DPAPI") {
        format!(
            "{message}。这通常发生在系统重装或换电脑后（当前 Windows 用户无法解密原配置）。请在设置中重新添加账号；保存后会用当前用户的加密重建配置。"
        )
    } else {
        message.to_owned()
    }
}

#[cfg(test)]
mod credential_error_tests {
    use super::explain_credential_error;

    #[test]
    fn dpapi_failure_gets_recovery_guidance() {
        let message = explain_credential_error(
            "无法读取 Windows DPAPI 凭据：解密失败。请使用 --reset-password 在本机重新输入密码。",
        );
        assert!(message.contains("系统重装或换电脑"), "{message}");
        assert!(message.contains("重新添加账号"), "{message}");
    }

    #[test]
    fn other_credential_errors_stay_untouched() {
        let message = explain_credential_error("账号配置 JSON 无效：expected value");
        assert_eq!(message, "账号配置 JSON 无效：expected value");
    }
}

/// 兼容旧单账号读取：取第一个账号。
pub fn load() -> Result<Option<AccountConfig>, String> {
    Ok(list()?.into_iter().next())
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
    }

    #[test]
    fn v3_list_roundtrips_and_deduplicates_by_identity() {
        let a = AccountConfig {
            schema_version: SCHEMA_VERSION,
            incoming: sample_incoming(),
            outgoing: Some(sample_smtp()),
        };
        let mut b = AccountConfig {
            schema_version: SCHEMA_VERSION,
            incoming: sample_incoming(),
            outgoing: None,
        };
        b.incoming.user = "other@example.com".into();
        let saved = ListShape {
            schema_version: SCHEMA_VERSION,
            accounts: vec![a.clone(), b.clone()],
        };
        let json = serde_json::to_string(&saved).unwrap();
        let list = parse_list(&json).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].incoming.user, "user@example.com");
        assert_eq!(list[1].incoming.user, "other@example.com");
    }

    #[test]
    fn v2_single_account_upgrades_to_one_element_list() {
        let v2 = r#"{"schema_version":2,"incoming":{"host":"imap.example.com","port":993,"user":"user@example.com","password":"secret"},"outgoing":null}"#;
        let list = parse_list(v2).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].incoming.user, "user@example.com");
        assert!(list[0].outgoing.is_none());
        assert_eq!(list[0].schema_version, SCHEMA_VERSION);
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
    }

    #[test]
    fn unknown_schema_version_is_rejected_not_degraded() {
        let future = r#"{"schema_version":99,"incoming":{"host":"h","port":993,"user":"u","password":"p"},"outgoing":null}"#;
        let error = AccountConfig::parse(future).unwrap_err();
        assert!(error.contains("不支持的账号配置版本"), "{error}");
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

        // v3 列表整体走 DPAPI 往返。
        let second = AccountConfig {
            schema_version: SCHEMA_VERSION,
            incoming: ImapConfig {
                user: "other@example.com".into(),
                ..sample_incoming()
            },
            outgoing: None,
        };
        save_all_to(&path, &[config.clone(), second.clone()]).unwrap();
        let round = list_from(&path).unwrap();
        assert_eq!(round.len(), 2);
        assert_eq!(round[1].incoming.user, "other@example.com");
        assert!(round[0].outgoing.is_some());

        let _ = std::fs::remove_dir_all(&root);
    }
}
