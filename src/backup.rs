use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use foxmail_imap::credentials;
use foxmail_imap::imap_config::{HOST, PORT, USER};
use foxmail_imap::imap_errors::login_error;
use imap::{ClientBuilder, Session};
use imap_proto::NameAttribute;
use serde::Serialize;

const DEFAULT_CHUNK_SIZE: usize = 4 * 1024 * 1024;

#[derive(Debug)]
struct Options {
    output: PathBuf,
    mailbox: Option<String>,
    max_messages: Option<usize>,
    chunk_size: usize,
    dry_run: bool,
    reset_password: bool,
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
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_options()?;
    let password = credentials::load_or_initialize(options.reset_password)?;

    println!("[连接] {}:{}，严格 TLS 校验", HOST, PORT);
    let client = ClientBuilder::new(HOST, PORT).connect()?;
    let mut session = client
        .login(USER, password.as_str())
        .map_err(|(error, _client)| login_error(error))?;
    println!("[登录] {}", USER);

    let mailboxes = resolve_mailboxes(&mut session, options.mailbox.as_deref())?;
    println!("[邮箱] 待处理 {} 个文件夹", mailboxes.len());

    let account_root = options.output.join(safe_component(USER));
    let mut total_backed_up = 0usize;
    let mut total_skipped = 0usize;

    for mailbox_name in mailboxes {
        let mailbox = session.examine(&mailbox_name)?;
        let uid_validity = mailbox
            .uid_validity
            .ok_or_else(|| format!("邮箱 {mailbox_name} 未返回 UIDVALIDITY"))?;
        let mut uids: Vec<u32> = session.uid_search("ALL")?.into_iter().collect();
        uids.sort_unstable();

        if let Some(limit) = options.max_messages {
            if uids.len() > limit {
                uids = uids[uids.len() - limit..].to_vec();
            }
        }

        println!(
            "\n[文件夹] {} exists={} uid_validity={} 本次 UID={}",
            mailbox_name,
            mailbox.exists,
            uid_validity,
            uids.len()
        );

        let mailbox_root = account_root
            .join(safe_component(&mailbox_name))
            .join(format!("uidvalidity-{uid_validity}"));

        if !options.dry_run {
            fs::create_dir_all(&mailbox_root)?;
            write_metadata(
                &mailbox_root,
                &MailboxMetadata {
                    account: USER,
                    server: HOST,
                    mailbox: &mailbox_name,
                    uid_validity,
                    scanned_at_unix: unix_now(),
                    server_message_count: mailbox.exists,
                    discovered_uid_count: uids.len(),
                },
            )?;
        }

        for uid in uids {
            let size = fetch_size(&mut session, uid)?;
            let final_path = mailbox_root.join(format!("{uid:010}.eml"));

            if final_path.exists() && final_path.metadata()?.len() == u64::from(size) {
                println!(
                    "  [跳过] UID {uid} 已存在，{}",
                    format_bytes(u64::from(size))
                );
                total_skipped += 1;
                continue;
            }

            if options.dry_run {
                println!("  [计划] UID {uid}，{}", format_bytes(u64::from(size)));
                continue;
            }

            backup_message(&mut session, uid, size, &final_path, options.chunk_size)?;
            total_backed_up += 1;
        }
    }

    session.logout()?;
    println!(
        "\n[完成] 新增 {} 封，跳过 {} 封，输出目录：{}",
        total_backed_up,
        total_skipped,
        account_root.display()
    );
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

fn backup_message<T: Read + Write>(
    session: &mut Session<T>,
    uid: u32,
    expected_size: u32,
    final_path: &Path,
    chunk_size: usize,
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

    println!(
        "  [备份] UID {uid}，{}，从 {} 继续",
        format_bytes(expected),
        format_bytes(offset)
    );

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
        print!(
            "\r    进度：{:>6.2}% ({}/{})",
            percentage(offset, expected),
            format_bytes(offset),
            format_bytes(expected)
        );
        std::io::stdout().flush()?;
    }
    println!();

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

fn parse_options() -> Result<Options, Box<dyn std::error::Error>> {
    let mut output = env::var_os("FOXMAIL_BACKUP_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data"));
    let mut mailbox = None;
    let mut max_messages = None;
    let mut chunk_size = DEFAULT_CHUNK_SIZE;
    let mut dry_run = false;
    let mut reset_password = false;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output" => output = PathBuf::from(args.next().ok_or("--output 缺少路径")?),
            "--mailbox" => mailbox = Some(args.next().ok_or("--mailbox 缺少名称")?),
            "--max-messages" => {
                let value = args.next().ok_or("--max-messages 缺少数量")?;
                max_messages = Some(value.parse()?);
            }
            "--chunk-mib" => {
                let value: usize = args.next().ok_or("--chunk-mib 缺少数量")?.parse()?;
                if value == 0 || value > 64 {
                    return Err("--chunk-mib 必须在 1..=64".into());
                }
                chunk_size = value * 1024 * 1024;
            }
            "--dry-run" => dry_run = true,
            "--reset-password" => reset_password = true,
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            _ => return Err(format!("未知参数：{arg}，使用 --help 查看帮助").into()),
        }
    }

    Ok(Options {
        output,
        mailbox,
        max_messages,
        chunk_size,
        dry_run,
        reset_password,
    })
}

fn print_help() {
    println!(
        "foxmail backup\n\n\
         用法：backup [选项]\n\n\
         --output <目录>        备份根目录，默认 ./data\n\
         --mailbox <名称>       仅备份指定文件夹\n\
         --max-messages <数量>  每个文件夹仅处理最新 N 封\n\
         --chunk-mib <数量>     分块大小 MiB，默认 4，范围 1..=64\n\
         --dry-run              只列 UID 和大小，不下载正文\n\
         --reset-password       重新输入并用 DPAPI 保存密码\n"
    );
}

fn safe_component(value: &str) -> String {
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

fn percentage(done: u64, total: u64) -> f64 {
    if total == 0 {
        100.0
    } else {
        done as f64 * 100.0 / total as f64
    }
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    let value = bytes as f64;
    if value >= GIB {
        format!("{:.2} GiB", value / GIB)
    } else if value >= MIB {
        format!("{:.2} MiB", value / MIB)
    } else if value >= KIB {
        format!("{:.2} KiB", value / KIB)
    } else {
        format!("{bytes} B")
    }
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
    fn percentage_handles_zero() {
        assert_eq!(percentage(0, 0), 100.0);
    }
}
