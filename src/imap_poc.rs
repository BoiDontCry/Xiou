use foxmail_imap::credentials;
use foxmail_imap::imap_config::{HOST, PORT, USER};
use foxmail_imap::imap_errors::login_error;
use imap::ClientBuilder;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let reset_password = std::env::args().any(|arg| arg == "--reset-password");
    let password = credentials::load_or_initialize(reset_password)?;

    println!(
        "[M0] IMAP PoC: 连接 {}:{}，启用严格 TLS 校验 ...",
        HOST, PORT
    );
    let client = ClientBuilder::new(HOST, PORT).connect()?;
    println!("[M0] TLS 握手及证书校验成功");

    let mut session = client
        .login(USER, password.as_str())
        .map_err(|(error, _client)| login_error(error))?;
    println!("[M0] LOGIN 成功: {}", USER);

    let inbox = session.examine("INBOX")?;
    println!(
        "[M0] EXAMINE INBOX: exists={} recent={} readonly=true uid_validity={:?}",
        inbox.exists, inbox.recent, inbox.uid_validity
    );

    let uids = session.uid_search("ALL")?;
    println!("[M0] UID SEARCH ALL: {} 封", uids.len());
    println!("[M0] 验收通过：未下载正文、未修改邮件状态。");

    session.logout()?;
    Ok(())
}
