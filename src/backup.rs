use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::imap_errors::login_error;
use crate::mime_plan::{self, DeferredAttachment, MimePlan};
use imap::Session;
use imap_proto::types::SectionPath;
use imap_proto::NameAttribute;
use mail_parser::MessageParser;
use native_tls::TlsConnector;
use serde::{Deserialize, Serialize};

pub const DEFAULT_CHUNK_SIZE: usize = 4 * 1024 * 1024;
pub const IMAP_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
pub const IMAP_IO_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub struct BackupOptions {
    pub output: PathBuf,
    pub mailbox: Option<String>,
    pub max_messages: Option<usize>,
    pub chunk_size: usize,
    pub dry_run: bool,
}

impl Default for BackupOptions {
    fn default() -> Self {
        Self {
            output: PathBuf::from("data"),
            mailbox: None,
            max_messages: None,
            chunk_size: DEFAULT_CHUNK_SIZE,
            dry_run: false,
        }
    }
}

/// 拉取进度回调。所有方法返回 `()`；实现者自行把事件桥接到 UI/日志。
/// 实现者不应在回调里做长耗时操作或持有锁——它在 IMAP 网络线程的调用栈里同步执行。
pub trait Progress {
    fn connecting(&mut self, host: &str, port: u16);
    fn logged_in(&mut self, user: &str);
    fn mailboxes_planned(&mut self, count: usize);
    /// 进入某文件夹。`exists` = 服务器返回的邮件总数，`uids` = 本次计划处理的 UID 数。
    fn mailbox_start(&mut self, name: &str, exists: u32, uids: usize);
    /// UID 被跳过（已存在且大小匹配）。
    fn message_skipped(&mut self, uid: u32, bytes: u64);
    /// UID 开始下载，`total` = RFC822.SIZE，`resume` = 断点续传起始偏移。
    fn message_start(&mut self, uid: u32, total: u64, resume: u64);
    /// UID 下载块到达 `done_bytes`（≤ total）。
    fn message_progress(&mut self, uid: u32, done: u64, total: u64);
    fn mailbox_done(&mut self, name: &str, backed_up: usize, skipped: usize);
    fn finished(&mut self, backed_up: usize, skipped: usize, output: &Path);
}

/// 无操作进度实现，给 CLI / 无需进度的调用方用。
pub struct NoProgress;
impl Progress for NoProgress {
    fn connecting(&mut self, _host: &str, _port: u16) {}
    fn logged_in(&mut self, _user: &str) {}
    fn mailboxes_planned(&mut self, _count: usize) {}
    fn mailbox_start(&mut self, _name: &str, _exists: u32, _uids: usize) {}
    fn message_skipped(&mut self, _uid: u32, _bytes: u64) {}
    fn message_start(&mut self, _uid: u32, _total: u64, _resume: u64) {}
    fn message_progress(&mut self, _uid: u32, _done: u64, _total: u64) {}
    fn mailbox_done(&mut self, _name: &str, _backed_up: usize, _skipped: usize) {}
    fn finished(&mut self, _backed_up: usize, _skipped: usize, _output: &Path) {}
}

/// 进度事件，GUI 把它跨线程桥接到主线程更新 UI。
#[derive(Clone, Debug)]
pub enum ProgressEvent {
    Connecting {
        host: String,
        port: u16,
    },
    LoggedIn {
        user: String,
    },
    MailboxesPlanned {
        count: usize,
    },
    MailboxStart {
        name: String,
        exists: u32,
        uids: usize,
    },
    MessageSkipped {
        uid: u32,
        bytes: u64,
    },
    MessageStart {
        uid: u32,
        total: u64,
        resume: u64,
    },
    MessageProgress {
        uid: u32,
        done: u64,
        total: u64,
    },
    MailboxDone {
        name: String,
        backed_up: usize,
        skipped: usize,
    },
    Finished {
        backed_up: usize,
        skipped: usize,
    },
    Failed {
        message: String,
    },
}

/// 把 `Progress` 事件转成 `mpsc::Sender<ProgressEvent>` 的桥接实现。
/// GUI 用它把 IMAP 线程的事件泵到主线程。
pub struct ChannelProgress {
    pub sender: std::sync::mpsc::Sender<ProgressEvent>,
    pub state: ChannelState,
}

#[derive(Default)]
pub struct ChannelState {
    pub index: usize,
    pub total_mailboxes: usize,
    pub mailbox_name: String,
    pub mailbox_backed_up: usize,
    pub mailbox_skipped: usize,
}

impl ChannelProgress {
    pub fn new(sender: std::sync::mpsc::Sender<ProgressEvent>) -> Self {
        Self {
            sender,
            state: ChannelState::default(),
        }
    }

    /// 拉取失败时发一个 Failed 事件（由调用方在错误路径上调用）。
    pub fn send_failed(&mut self, message: String) {
        let _ = self.sender.send(ProgressEvent::Failed { message });
    }
}

impl Progress for ChannelProgress {
    fn connecting(&mut self, host: &str, port: u16) {
        let _ = self.sender.send(ProgressEvent::Connecting {
            host: host.to_owned(),
            port,
        });
    }
    fn logged_in(&mut self, user: &str) {
        let _ = self.sender.send(ProgressEvent::LoggedIn {
            user: user.to_owned(),
        });
    }
    fn mailboxes_planned(&mut self, count: usize) {
        self.state.total_mailboxes = count;
        let _ = self.sender.send(ProgressEvent::MailboxesPlanned { count });
    }
    fn mailbox_start(&mut self, name: &str, exists: u32, uids: usize) {
        self.state.index += 1;
        self.state.mailbox_name = name.to_owned();
        self.state.mailbox_backed_up = 0;
        self.state.mailbox_skipped = 0;
        let _ = self.sender.send(ProgressEvent::MailboxStart {
            name: name.to_owned(),
            exists,
            uids,
        });
    }
    fn message_skipped(&mut self, uid: u32, bytes: u64) {
        self.state.mailbox_skipped += 1;
        let _ = self
            .sender
            .send(ProgressEvent::MessageSkipped { uid, bytes });
    }
    fn message_start(&mut self, uid: u32, total: u64, resume: u64) {
        self.state.mailbox_backed_up += 1;
        let _ = self
            .sender
            .send(ProgressEvent::MessageStart { uid, total, resume });
    }
    fn message_progress(&mut self, uid: u32, done: u64, total: u64) {
        let _ = self
            .sender
            .send(ProgressEvent::MessageProgress { uid, done, total });
    }
    fn mailbox_done(&mut self, name: &str, backed_up: usize, skipped: usize) {
        let _ = self.sender.send(ProgressEvent::MailboxDone {
            name: name.to_owned(),
            backed_up,
            skipped,
        });
    }
    fn finished(&mut self, backed_up: usize, skipped: usize, _output: &Path) {
        let _ = self
            .sender
            .send(ProgressEvent::Finished { backed_up, skipped });
    }
}

#[derive(Serialize)]
struct MailboxMetadata<'a> {
    account: &'a str,
    server: &'a str,
    mailbox: &'a str,
    uid_validity: u32,
    scanned_at_unix: u64,
    server_message_count: u32,
    discovered_uid_count: usize,
    last_uid: Option<u32>,
}

#[derive(Deserialize)]
struct MailboxMetadataFile {
    #[serde(default)]
    last_uid: Option<u32>,
}

fn uid_search_criteria(last_uid: Option<u32>) -> String {
    match last_uid {
        Some(uid) if uid < u32::MAX => format!("UID {}:*", uid + 1),
        _ => "ALL".to_owned(),
    }
}

fn read_last_uid(mailbox_root: &Path) -> Option<u32> {
    let bytes = fs::read(mailbox_root.join("mailbox.meta.json")).ok()?;
    serde_json::from_slice::<MailboxMetadataFile>(&bytes)
        .ok()?
        .last_uid
}

fn has_local_eml(final_path: &Path) -> bool {
    final_path.is_file()
}

pub fn connect(
    host: &str,
    port: u16,
) -> Result<imap::Client<native_tls::TlsStream<TcpStream>>, Box<dyn std::error::Error>> {
    let mut last_error: Option<Box<dyn std::error::Error>> = None;
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, IMAP_CONNECT_TIMEOUT) {
            Ok(tcp) => {
                tcp.set_read_timeout(Some(IMAP_IO_TIMEOUT))?;
                tcp.set_write_timeout(Some(IMAP_IO_TIMEOUT))?;
                let tls = match TlsConnector::builder().build() {
                    Ok(tls) => tls,
                    Err(error) => return Err(crate::imap_errors::connect_error(error).into()),
                };
                let stream = match tls.connect(host, tcp) {
                    Ok(stream) => stream,
                    Err(error) => return Err(crate::imap_errors::connect_error(error).into()),
                };
                let mut client = imap::Client::new(stream);
                client.read_greeting()?;
                return Ok(client);
            }
            Err(error) => last_error = Some(crate::imap_errors::connect_error(error).into()),
        }
    }
    Err(last_error.unwrap_or_else(|| format!("无法解析 {host}:{port}").into()))
}

/// 用已保存账号配置（DPAPI）的收信设置跑全量备份。供 CLI 复用。
/// `progress` 在 IMAP 网络线程的调用栈里同步回调。
pub fn run_backup(
    options: &BackupOptions,
    progress: &mut dyn Progress,
) -> Result<(), Box<dyn std::error::Error>> {
    let account = crate::account_config::load()
        .map_err(|error| format!("读取账号配置失败：{error}"))?
        .ok_or("未找到邮箱账号配置，请先在 xiou 中配置账号")?;
    run_backup_with(
        options,
        &account.incoming.host,
        account.incoming.port,
        &account.incoming.user,
        &account.incoming.password,
        progress,
    )
}

/// 显式传入账号凭据跑备份。GUI 用这个传入用户填写的 host/port/user/password。
pub fn run_backup_with(
    options: &BackupOptions,
    host: &str,
    port: u16,
    user: &str,
    password: &str,
    progress: &mut dyn Progress,
) -> Result<(), Box<dyn std::error::Error>> {
    progress.connecting(host, port);
    let client = connect(host, port)?;
    let mut session = client
        .login(user, password)
        .map_err(|(error, _client)| login_error(error))?;
    progress.logged_in(user);

    let mailboxes = resolve_mailboxes(&mut session, options.mailbox.as_deref())?;
    progress.mailboxes_planned(mailboxes.len());

    let account_root = options.output.join(safe_component(user));
    let mut total_backed_up = 0usize;
    let mut total_skipped = 0usize;

    for mailbox_name in mailboxes {
        let mailbox = session.examine(&mailbox_name)?;
        let uid_validity = mailbox
            .uid_validity
            .ok_or_else(|| format!("邮箱 {mailbox_name} 未返回 UIDVALIDITY"))?;

        let mailbox_root = account_root
            .join(safe_component(&mailbox_name))
            .join(format!("uidvalidity-{uid_validity}"));
        let last_uid = read_last_uid(&mailbox_root);
        let mut uids: Vec<u32> = session
            .uid_search(uid_search_criteria(last_uid))?
            .into_iter()
            .collect();
        uids.sort_unstable();

        if let Some(limit) = options.max_messages {
            if uids.len() > limit {
                uids = uids[uids.len() - limit..].to_vec();
            }
        }

        progress.mailbox_start(&mailbox_name, mailbox.exists, uids.len());

        if !options.dry_run {
            fs::create_dir_all(&mailbox_root)?;
        }

        let mut mailbox_backed_up = 0usize;
        let mut mailbox_skipped = 0usize;

        for uid in &uids {
            let uid = *uid;
            let final_path = mailbox_root.join(format!("{uid:010}.eml"));

            // 本地已有完整 eml：旧信不打 SIZE、不重下。
            if has_local_eml(&final_path) {
                mailbox_skipped += 1;
                total_skipped += 1;
                continue;
            }

            let (size, plan) = fetch_size_and_plan(&mut session, uid)?;

            if options.dry_run {
                mailbox_skipped += 1;
                total_skipped += 1;
                continue;
            }

            match plan.filter(|plan| plan.omit_large_attachments()) {
                Some(plan) => backup_partial(
                    &mut session,
                    uid,
                    size,
                    &mailbox_name,
                    &plan,
                    &final_path,
                    progress,
                )?,
                None => backup_message(
                    &mut session,
                    uid,
                    size,
                    &final_path,
                    options.chunk_size,
                    progress,
                )?,
            }
            mailbox_backed_up += 1;
            total_backed_up += 1;
        }

        if !options.dry_run {
            let next_last_uid = uids.last().copied().or(last_uid);
            write_metadata(
                &mailbox_root,
                &MailboxMetadata {
                    account: user,
                    server: host,
                    mailbox: &mailbox_name,
                    uid_validity,
                    scanned_at_unix: unix_now(),
                    server_message_count: mailbox.exists,
                    discovered_uid_count: uids.len(),
                    last_uid: next_last_uid,
                },
            )?;
        }

        progress.mailbox_done(&mailbox_name, mailbox_backed_up, mailbox_skipped);
    }

    session.logout()?;
    progress.finished(total_backed_up, total_skipped, &account_root);
    Ok(())
}

fn resolve_mailboxes<T: Read + Write>(
    session: &mut Session<T>,
    only: Option<&str>,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    if let Some(name) = only {
        return Ok(vec![name.to_owned()]);
    }

    let names = session.list(None, Some("*"))?;
    let mut result: Vec<String> = names
        .iter()
        .filter(|name| {
            !name
                .attributes()
                .iter()
                .any(|attribute| matches!(attribute, NameAttribute::NoSelect))
        })
        .map(|name| name.name().to_owned())
        .collect();

    result.sort_by_key(|name| {
        if name.eq_ignore_ascii_case("INBOX") {
            0
        } else {
            1
        }
    });
    Ok(result)
}

fn fetch_size_and_plan<T: Read + Write>(
    session: &mut Session<T>,
    uid: u32,
) -> Result<(u32, Option<MimePlan>), Box<dyn std::error::Error>> {
    match session.uid_fetch(uid.to_string(), "(UID RFC822.SIZE BODYSTRUCTURE)") {
        Ok(fetches) => {
            let fetch = fetches
                .iter()
                .find(|fetch| fetch.uid == Some(uid))
                .ok_or_else(|| format!("UID {uid} 未返回 FETCH 响应"))?;
            let size = fetch
                .size
                .ok_or_else(|| format!("UID {uid} 未返回 RFC822.SIZE"))?;
            let plan = fetch.bodystructure().map(MimePlan::from_bodystructure);
            Ok((size, plan))
        }
        Err(_) => Ok((fetch_size(session, uid)?, None)),
    }
}

fn fetch_size<T: Read + Write>(
    session: &mut Session<T>,
    uid: u32,
) -> Result<u32, Box<dyn std::error::Error>> {
    let fetches = session.uid_fetch(uid.to_string(), "(UID RFC822.SIZE)")?;
    let fetch = fetches
        .iter()
        .find(|fetch| fetch.uid == Some(uid))
        .ok_or_else(|| format!("UID {uid} 未返回 FETCH 响应"))?;
    fetch
        .size
        .ok_or_else(|| format!("UID {uid} 未返回 RFC822.SIZE").into())
}

fn backup_partial<T: Read + Write>(
    session: &mut Session<T>,
    uid: u32,
    expected_size: u32,
    mailbox: &str,
    plan: &MimePlan,
    final_path: &Path,
    progress: &mut dyn Progress,
) -> Result<(), Box<dyn std::error::Error>> {
    let expected = u64::from(expected_size);
    progress.message_start(uid, expected, 0);

    let mut query = String::from("(UID BODY.PEEK[HEADER]");
    for part in &plan.texts {
        query.push_str(" BODY.PEEK[");
        query.push_str(&part.section);
        query.push(']');
    }
    query.push(')');

    let fetches = session.uid_fetch(uid.to_string(), query)?;
    let fetch = fetches
        .iter()
        .find(|fetch| fetch.uid == Some(uid))
        .ok_or_else(|| format!("UID {uid} 未返回 HEADER/正文"))?;
    let header = fetch
        .header()
        .ok_or_else(|| format!("UID {uid} 未返回 HEADER"))?
        .to_vec();
    let mut texts = Vec::new();
    let mut done = header.len() as u64;
    for part in &plan.texts {
        let path = mime_plan::section_path(&part.section)
            .ok_or_else(|| format!("UID {uid} 无效 MIME 段 {}", part.section))?;
        let bytes = fetch
            .section(&path)
            .ok_or_else(|| format!("UID {uid} 未返回 BODY[{}]", part.section))?
            .to_vec();
        done += bytes.len() as u64;
        texts.push(bytes);
    }
    progress.message_progress(uid, done.min(expected), expected);

    let eml = mime_plan::build_partial_eml(&header, plan, &texts);
    let part_path = final_path.with_extension("eml.part");
    fs::write(&part_path, &eml)?;
    let sidecar = serde_json::to_vec_pretty(&plan.sidecar(mailbox, uid))?;
    fs::write(final_path.with_extension("parts.json"), sidecar)?;
    if final_path.exists() {
        fs::remove_file(final_path)?;
    }
    fs::rename(part_path, final_path)?;
    progress.message_progress(uid, expected, expected);
    Ok(())
}

pub fn fetch_deferred_attachment(
    deferred: &DeferredAttachment,
    host: &str,
    port: u16,
    user: &str,
    password: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let client = connect(host, port)?;
    let mut session = client
        .login(user, password)
        .map_err(|(error, _client)| login_error(error))?;
    session.examine(&deferred.mailbox)?;
    let path = mime_plan::section_path(&deferred.section)
        .ok_or_else(|| format!("无效 MIME 段 {}", deferred.section))?;
    let raw = fetch_section(&mut session, deferred.uid, &path)?;
    session.logout()?;
    decode_transfer(&raw, &deferred.encoding)
}

fn fetch_section<T: Read + Write>(
    session: &mut Session<T>,
    uid: u32,
    path: &SectionPath,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let section = match path {
        SectionPath::Part(parts, None) => parts
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("."),
        _ => return Err("不支持的 MIME 段".into()),
    };
    let query = format!("(UID BODY.PEEK[{section}])");
    let fetches = session.uid_fetch(uid.to_string(), query)?;
    let fetch = fetches
        .iter()
        .find(|fetch| fetch.uid == Some(uid))
        .ok_or_else(|| format!("UID {uid} 未返回 BODY[{section}]"))?;
    Ok(fetch
        .section(path)
        .ok_or_else(|| format!("UID {uid} BODY[{section}] 为空"))?
        .to_vec())
}

fn decode_transfer(data: &[u8], encoding: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    match encoding.to_ascii_lowercase().as_str() {
        "" | "7bit" | "8bit" | "binary" => Ok(data.to_vec()),
        encoding => {
            let mut wrapped = format!(
                "From: decoder@local\r\nMIME-Version: 1.0\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=part.bin\r\nContent-Transfer-Encoding: {encoding}\r\n\r\n"
            )
            .into_bytes();
            wrapped.extend_from_slice(data);
            let message = MessageParser::default()
                .parse(&wrapped)
                .ok_or("无法解码附件传输编码")?;
            let part = message.attachment(0).ok_or("解码后没有附件载荷")?;
            Ok(match &part.body {
                mail_parser::PartType::Binary(bytes)
                | mail_parser::PartType::InlineBinary(bytes) => bytes.to_vec(),
                mail_parser::PartType::Text(text) | mail_parser::PartType::Html(text) => {
                    text.as_bytes().to_vec()
                }
                mail_parser::PartType::Message(nested) => nested.raw_message.to_vec(),
                mail_parser::PartType::Multipart(_) => return Err("附件是 multipart 容器".into()),
            })
        }
    }
}

fn backup_message<T: Read + Write>(
    session: &mut Session<T>,
    uid: u32,
    expected_size: u32,
    final_path: &Path,
    chunk_size: usize,
    progress: &mut dyn Progress,
) -> Result<(), Box<dyn std::error::Error>> {
    let part_path = final_path.with_extension("eml.part");
    let expected = u64::from(expected_size);

    if final_path.exists() {
        let invalid = final_path.with_extension(format!("eml.invalid-{}", unix_now()));
        fs::rename(final_path, invalid)?;
    }

    if part_path.exists() && part_path.metadata()?.len() > expected {
        fs::remove_file(&part_path)?;
    }

    let mut offset = part_path.metadata().map(|meta| meta.len()).unwrap_or(0);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&part_path)?;

    progress.message_start(uid, expected, offset);

    while offset < expected {
        let remaining = expected - offset;
        let requested = remaining.min(chunk_size as u64) as usize;
        let query = format!("(UID BODY.PEEK[]<{}.{requested}>)", offset);
        let fetches = session.uid_fetch(uid.to_string(), query)?;
        let fetch = fetches
            .iter()
            .find(|fetch| fetch.uid == Some(uid))
            .ok_or_else(|| format!("UID {uid} offset {offset} 未返回 FETCH 响应"))?;
        let body = fetch
            .body()
            .ok_or_else(|| format!("UID {uid} offset {offset} 未返回正文分块"))?;

        if body.is_empty() {
            return Err(format!("UID {uid} offset {offset} 返回空分块").into());
        }
        if body.len() > requested {
            return Err(format!(
                "UID {uid} offset {offset} 返回 {} 字节，超过请求的 {requested} 字节",
                body.len()
            )
            .into());
        }

        file.write_all(body)?;
        offset += body.len() as u64;
        progress.message_progress(uid, offset, expected);
    }

    file.sync_all()?;
    drop(file);

    let actual = part_path.metadata()?.len();
    if actual != expected {
        return Err(
            format!("UID {uid} 文件大小校验失败：expected={expected}, actual={actual}").into(),
        );
    }
    fs::rename(part_path, final_path)?;
    Ok(())
}

fn write_metadata(
    path: &Path,
    metadata: &MailboxMetadata<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    let final_path = path.join("mailbox.meta.json");
    let temp_path = path.join("mailbox.meta.json.part");
    let bytes = serde_json::to_vec_pretty(metadata)?;
    let mut file = File::create(&temp_path)?;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    drop(file);
    if final_path.exists() {
        fs::remove_file(&final_path)?;
    }
    fs::rename(temp_path, final_path)?;
    Ok(())
}

pub fn safe_component(value: &str) -> String {
    let mut result = String::with_capacity(value.len() + 17);
    for ch in value.chars() {
        if ch.is_alphanumeric() || matches!(ch, '-' | '_' | '.' | ' ') {
            result.push(ch);
        } else {
            result.push('_');
        }
    }
    while matches!(result.chars().last(), Some('.' | ' ')) {
        result.pop();
    }
    if result.is_empty() {
        result.push_str("mailbox");
    }
    result.push('-');
    result.push_str(&format!("{:016x}", fnv1a64(value.as_bytes())));
    result
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_component_is_stable_and_windows_safe() {
        let value = safe_component("Sent/项目:2026");
        assert!(!value.contains('/'));
        assert!(!value.contains(':'));
        assert_eq!(value, safe_component("Sent/项目:2026"));
    }

    #[test]
    fn uid_search_is_incremental_after_last_uid() {
        assert_eq!(uid_search_criteria(None), "ALL");
        assert_eq!(uid_search_criteria(Some(0)), "UID 1:*");
        assert_eq!(uid_search_criteria(Some(100)), "UID 101:*");
        assert_eq!(uid_search_criteria(Some(u32::MAX)), "ALL");
    }

    #[test]
    fn imap_timeouts_are_bounded() {
        assert_eq!(IMAP_CONNECT_TIMEOUT, Duration::from_secs(30));
        assert_eq!(IMAP_IO_TIMEOUT, Duration::from_secs(60));
        assert!(IMAP_CONNECT_TIMEOUT <= IMAP_IO_TIMEOUT);
    }

    #[test]
    fn local_eml_counts_as_already_backed_up() {
        let dir = std::env::temp_dir().join(format!("foxmail-rs-eml-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("0000000042.eml");
        assert!(!has_local_eml(&path));
        fs::write(&path, b"From: test\r\n\r\n").unwrap();
        assert!(has_local_eml(&path));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_progress_is_a_no_op_sink() {
        let mut sink = NoProgress;
        sink.connecting("h", 1);
        sink.logged_in("u");
        sink.mailboxes_planned(0);
        sink.mailbox_start("n", 0, 0);
        sink.message_skipped(1, 0);
        sink.message_start(1, 0, 0);
        sink.message_progress(1, 0, 0);
        sink.mailbox_done("n", 0, 0);
        sink.finished(0, 0, Path::new("x"));
    }
}
