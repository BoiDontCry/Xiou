use foxmail_imap::credentials;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
use std::env;

const SMTP_HOST: &str = "smtp.yunyou.top";
const SMTP_PORT: u16 = 465;
const FROM_ADDR: &str = "zhangwei@szshuori.com";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let reset_password = std::env::args().any(|arg| arg == "--reset-password");
    let to_arg = env::args()
        .find(|a| !a.starts_with("--"))
        .filter(|a| a.contains('@'));
    let to_addr = to_arg.as_deref().unwrap_or("2vain0131@gmail.com");
    let subject = env::var("FOXMAIL_SMTP_SUBJECT")
        .unwrap_or_else(|_| "foxmail-rs SMTP PoC 测试邮件".to_string());
    let body = env::var("FOXMAIL_SMTP_BODY").unwrap_or_else(|_| {
        format!(
            "这是 foxmail-rs SMTP PoC 发送的测试邮件。\n发件人: {}\n收件人: {}\n时间标记: PoC 自动生成。",
            FROM_ADDR, to_addr
        )
    });

    let password = credentials::load_or_initialize(reset_password)?;
    println!(
        "[SMTP] 连接 {}:{} 隐式 SSL，FROM={}，TO={}",
        SMTP_HOST, SMTP_PORT, FROM_ADDR, to_addr
    );

    let from: Mailbox = FROM_ADDR.parse()?;
    let to: Mailbox = to_addr.parse()?;
    let email = Message::builder()
        .from(from)
        .to(to)
        .subject(&subject)
        .body(body)?;
    println!("[SMTP] 邮件构造完成，主题: {}", subject);

    let transport = SmtpTransport::relay(SMTP_HOST)?
        .port(SMTP_PORT)
        .credentials(Credentials::new(
            FROM_ADDR.to_string(),
            password.as_str().to_string(),
        ))
        .build();

    match transport.send(&email) {
        Ok(response) => {
            let msg: Vec<&str> = response.message().collect();
            println!(
                "[SMTP] 发送成功: code={}, message={:?}",
                response.code(),
                msg
            );
            println!("[SMTP] 验收通过: 邮件已投递给 {}。请检查收件箱。", to_addr);
            Ok(())
        }
        Err(error) => {
            eprintln!("[SMTP] 发送失败: {error}");
            Err(error.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lettre::Address;

    #[test]
    fn parses_addresses() {
        assert!(FROM_ADDR.parse::<Address>().is_ok());
        assert!("2vain0131@gmail.com".parse::<Address>().is_ok());
    }

    #[test]
    fn smtp_config_locked() {
        assert_eq!(SMTP_HOST, "smtp.yunyou.top");
        assert_eq!(SMTP_PORT, 465);
    }
}
