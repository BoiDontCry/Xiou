//! 统一 SMTP 发送模块：GUI 与 `smtp-send` 共用同一请求模型与传输实现。
//!
//! `SendRequest` 在发送开始时构造并冻结，后续配置变化不影响进行中的任务。
//! 错误分类只携带服务器类别、执行阶段与数字码，不含凭据。

use crate::account_config::SmtpConfig;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::Error as SmtpError;
use lettre::{Message, SmtpTransport, Transport};

#[derive(Debug, Clone)]
pub struct SendRequest {
    pub account: SmtpConfig,
    pub to: Vec<Mailbox>,
    pub cc: Vec<Mailbox>,
    pub subject: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendReceipt {
    pub code: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    /// 未配置 SMTP：不建立任何网络连接。
    NotConfigured,
    /// 发件地址无效。
    InvalidFrom(String),
    /// 收件或抄送地址无效（含收件人为空）。
    InvalidRecipient(String),
    /// DNS 解析或 TCP 连接失败。
    Connection(String),
    /// TLS 协商失败。
    Tls(String),
    /// 服务器明确拒绝认证凭据（535）。
    Auth(String),
    /// 服务器拒绝（4xx/5xx，非认证）。
    ServerRejected(String),
    /// 连接或等待响应超时。
    Timeout(String),
    /// 其他未知错误。
    Unknown(String),
}

impl SendError {
    /// 稳定类别名，供日志记录；不含错误详情。
    pub fn category(&self) -> &'static str {
        match self {
            SendError::NotConfigured => "not-configured",
            SendError::InvalidFrom(_) => "invalid-from",
            SendError::InvalidRecipient(_) => "invalid-recipient",
            SendError::Connection(_) => "connection",
            SendError::Tls(_) => "tls",
            SendError::Auth(_) => "auth",
            SendError::ServerRejected(_) => "server-rejected",
            SendError::Timeout(_) => "timeout",
            SendError::Unknown(_) => "unknown",
        }
    }

    /// 用户可读消息；不含密码或授权码。
    pub fn message(&self) -> String {
        match self {
            SendError::NotConfigured => {
                "未配置发件账号，请在设置中填写 SMTP 信息后再发送".to_owned()
            }
            SendError::InvalidFrom(detail) => format!("发件地址无效：{detail}"),
            SendError::InvalidRecipient(detail) => format!("收件地址无效：{detail}"),
            SendError::Connection(detail) => format!("无法连接发信服务器：{detail}"),
            SendError::Tls(detail) => format!("TLS 协商失败：{detail}"),
            SendError::Auth(_) => "发信服务器拒绝了认证凭据，请检查 SMTP 用户名与密码".to_owned(),
            SendError::ServerRejected(detail) => format!("发信服务器拒绝：{detail}"),
            SendError::Timeout(_) => "发送超时，请检查网络后重试".to_owned(),
            SendError::Unknown(detail) => format!("发送失败：{detail}"),
        }
    }
}

impl SendRequest {
    /// 解析并校验地址列表，构造不可变发送请求。未配置 SMTP 时直接失败，不联网。
    pub fn build(
        account: Option<SmtpConfig>,
        to: &str,
        cc: &str,
        subject: &str,
        body: &str,
    ) -> Result<Self, SendError> {
        let account = account.ok_or(SendError::NotConfigured)?;
        account
            .from_address
            .trim()
            .parse::<Mailbox>()
            .map_err(|error| SendError::InvalidFrom(error.to_string()))?;
        let to = parse_address_list(to, "收件地址无效")?;
        if to.is_empty() {
            return Err(SendError::InvalidRecipient("收件人不能为空".to_owned()));
        }
        let cc = parse_address_list(cc, "抄送地址无效")?;
        Ok(Self {
            account,
            to,
            cc,
            subject: subject.to_owned(),
            body: body.to_owned(),
        })
    }
}

fn parse_address_list(text: &str, label: &'static str) -> Result<Vec<Mailbox>, SendError> {
    text.split(',')
        .map(str::trim)
        .filter(|address| !address.is_empty())
        .map(|address| {
            address
                .parse::<Mailbox>()
                .map_err(|error| SendError::InvalidRecipient(format!("{label}：{error}")))
        })
        .collect()
}

/// 同步发送冻结的请求。供后台线程调用；不做任何配置读取。
pub fn send(request: &SendRequest) -> Result<SendReceipt, SendError> {
    let from = request
        .account
        .from_address
        .trim()
        .parse::<Mailbox>()
        .map_err(|error| SendError::InvalidFrom(error.to_string()))?;
    let mut builder = Message::builder().from(from).subject(&request.subject);
    for mailbox in &request.to {
        builder = builder.to(mailbox.clone());
    }
    for mailbox in &request.cc {
        builder = builder.cc(mailbox.clone());
    }
    let email = builder
        .body(request.body.clone())
        .map_err(|error| SendError::Unknown(format!("构造邮件失败：{error}")))?;

    let host = request.account.host.trim().to_owned();
    let transport_builder = match request.account.security {
        crate::account_config::SmtpSecurity::ImplicitTls => SmtpTransport::relay(&host),
        crate::account_config::SmtpSecurity::StartTls => SmtpTransport::starttls_relay(&host),
    }
    .map_err(classify)?;
    let transport = transport_builder
        .port(request.account.port)
        .credentials(Credentials::new(
            request.account.auth_user.clone(),
            request.account.password.clone(),
        ))
        .build();

    match transport.send(&email) {
        Ok(response) => Ok(SendReceipt {
            code: u16::from(response.code()),
        }),
        Err(error) => Err(classify(error)),
    }
}

/// 只用 Display（不含凭据）；按 lettre 错误谓词映射到稳定类别。
fn classify(error: SmtpError) -> SendError {
    let detail = error.to_string();
    if error.is_timeout() {
        SendError::Timeout(detail)
    } else if error.is_tls() {
        SendError::Tls(detail)
    } else if let Some(code) = error.status() {
        if code.to_string() == "535" {
            SendError::Auth(detail)
        } else {
            SendError::ServerRejected(detail)
        }
    } else if error.is_response() || error.is_client() {
        SendError::Unknown(detail)
    } else {
        SendError::Connection(detail)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account_config::{SmtpSecurity, SCHEMA_VERSION};

    fn sample_account() -> Option<SmtpConfig> {
        Some(SmtpConfig {
            host: "smtp.example.com".into(),
            port: 465,
            security: SmtpSecurity::ImplicitTls,
            auth_user: "user@example.com".into(),
            password: "secret".into(),
            from_address: "user@example.com".into(),
        })
    }

    #[test]
    fn build_parses_to_and_cc_lists() {
        let request = SendRequest::build(
            sample_account(),
            "a@example.com, b@example.com",
            " c@example.com ,d@example.com",
            "主题",
            "正文",
        )
        .unwrap();
        assert_eq!(request.to.len(), 2);
        assert_eq!(request.cc.len(), 2);
        assert_eq!(request.subject, "主题");
    }

    #[test]
    fn build_without_smtp_fails_locally_without_network() {
        let error = SendRequest::build(None, "a@example.com", "", "主题", "正文").unwrap_err();
        assert_eq!(error, SendError::NotConfigured);
        assert!(error.message().contains("未配置发件账号"));
    }

    #[test]
    fn build_rejects_invalid_addresses_before_any_connection() {
        let error =
            SendRequest::build(sample_account(), "not-an-address", "", "主题", "正文").unwrap_err();
        assert_eq!(error.category(), "invalid-recipient");
        assert!(error.message().contains("收件地址无效"));

        let error = SendRequest::build(sample_account(), "", "", "主题", "正文").unwrap_err();
        assert!(error.message().contains("收件人不能为空"));

        let error = SendRequest::build(
            sample_account(),
            "good@example.com, bad",
            "",
            "主题",
            "正文",
        )
        .unwrap_err();
        assert_eq!(error.category(), "invalid-recipient");

        let error =
            SendRequest::build(sample_account(), "a@x.com", "bad", "主题", "正文").unwrap_err();
        assert_eq!(error.category(), "invalid-recipient");
    }

    #[test]
    fn invalid_from_is_rejected_as_invalid_from_category() {
        let mut account = sample_account().unwrap();
        account.from_address = "broken".into();
        let error =
            SendRequest::build(Some(account), "a@example.com", "", "主题", "正文").unwrap_err();
        assert_eq!(error.category(), "invalid-from");
    }

    #[test]
    fn error_messages_never_contain_passwords_or_hosts_with_credentials() {
        let error = SendError::Auth("permanent error (535)".to_owned());
        assert!(!error.message().contains("secret"));
        assert!(!error.message().contains("user@example.com"));
        for error in [
            SendError::NotConfigured,
            SendError::Timeout("timed out".into()),
            SendError::Connection("dns failure".into()),
            SendError::Unknown("boom".into()),
        ] {
            assert!(!error.message().contains("secret"));
        }
    }

    #[test]
    fn schema_marker_constant_is_single_sourced() {
        // 防呆：smtp.rs 与账号配置共用同一版本常量来源。
        assert_eq!(SCHEMA_VERSION, 2);
    }
}
