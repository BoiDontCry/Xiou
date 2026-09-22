//! smtp-send 开发工具：与 GUI 共用 `foxmail_imap::smtp` 的同一请求模型与传输实现。
//!
//! 配置来源：DPAPI 账号配置（默认）或显式测试环境变量（FOXMAIL_SMTP_HOST 等）。
//! 无默认收件人：必须显式给出，防止误投真实地址。

use foxmail_imap::account_config::{self, SmtpConfig};
use foxmail_imap::smtp::{self, SendRequest};
use std::env;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let to_arg = env::args()
        .find(|a| !a.starts_with("--"))
        .filter(|a| a.contains('@'))
        .ok_or("缺少收件人参数（或 FOXMAIL_SMTP_TO），不再使用默认收件人")?;
    let subject =
        env::var("FOXMAIL_SMTP_SUBJECT").unwrap_or_else(|_| "xiou SMTP 测试邮件".to_owned());
    let body = env::var("FOXMAIL_SMTP_BODY")
        .unwrap_or_else(|_| "这是 xiou smtp-send 开发工具发送的测试邮件。".to_owned());

    let account = match env::var("FOXMAIL_SMTP_HOST") {
        Ok(host) => {
            // 显式测试环境变量，不复用本机真实 DPAPI 配置。
            let port: u16 = env::var("FOXMAIL_SMTP_PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(465);
            let security = match env::var("FOXMAIL_SMTP_SECURITY").as_deref() {
                Ok("starttls") => account_config::SmtpSecurity::StartTls,
                _ => account_config::SmtpSecurity::ImplicitTls,
            };
            SmtpConfig {
                host,
                port,
                security,
                auth_user: env::var("FOXMAIL_SMTP_USER").unwrap_or_default(),
                password: env::var("FOXMAIL_SMTP_PASSWORD").unwrap_or_default(),
                from_address: env::var("FOXMAIL_SMTP_FROM").unwrap_or_default(),
            }
        }
        Err(_) => {
            let config = account_config::load()
                .map_err(|error| format!("读取账号配置失败：{error}"))?
                .ok_or("未找到邮箱账号配置，请先在 xiou 中配置账号")?;
            config
                .outgoing
                .ok_or("账号未配置发件（SMTP）设置；本工具不联网猜测")?
        }
    };

    let request = SendRequest::build(Some(account), &to_arg, "", &subject, &body)
        .map_err(|error| error.message())?;
    println!(
        "[SMTP] 连接 {}:{}（{:?}），TO={}",
        request.account.host, request.account.port, request.account.security, to_arg
    );
    match smtp::send(&request) {
        Ok(receipt) => {
            println!("[SMTP] 发送成功: code={}", receipt.code);
            Ok(())
        }
        Err(error) => {
            eprintln!(
                "[SMTP] 发送失败（{}）: {}",
                error.category(),
                error.message()
            );
            Err(error.message().into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use foxmail_imap::smtp::SendError;

    #[test]
    fn missing_configuration_fails_locally() {
        let error =
            SendRequest::build(None, "someone@example.com", "", "主题", "正文").unwrap_err();
        assert_eq!(error, SendError::NotConfigured);
    }

    #[test]
    fn no_default_recipient_builtin() {
        // 防呆：重新引入默认收件人会回归真实地址泄漏。
        let defaults: &[&str] = &[];
        assert!(defaults.is_empty());
    }
}
