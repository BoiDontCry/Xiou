use foxmail_imap::account_config;
use foxmail_imap::backup;
use foxmail_imap::imap_errors::login_error;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let account = account_config::load()
        .map_err(|error| format!("读取账号配置失败：{error}"))?
        .ok_or("未找到邮箱账号配置，请先在 xiou 中配置账号")?;
    let incoming = &account.incoming;

    println!(
        "[M0] IMAP PoC: 连接 {}:{}，启用严格 TLS 校验 ...",
        incoming.host, incoming.port
    );
    let client = backup::connect(&incoming.host, incoming.port)?;
    println!("[M0] TLS 握手及证书校验成功");

    let mut session = client
        .login(&incoming.user, &incoming.password)
        .map_err(|(error, _client)| login_error(error))?;
    println!("[M0] LOGIN 成功");

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
