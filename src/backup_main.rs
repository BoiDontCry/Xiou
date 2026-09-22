use std::env;
use std::path::PathBuf;

use foxmail_imap::backup::{run_backup, BackupOptions, NoProgress};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = parse_options()?;
    let mut progress = NoProgress;
    run_backup(&options, &mut progress)
}

fn parse_options() -> Result<BackupOptions, Box<dyn std::error::Error>> {
    let mut output = env::var_os("FOXMAIL_BACKUP_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data"));
    let mut mailbox = None;
    let mut max_messages = None;
    let mut chunk_size = foxmail_imap::backup::DEFAULT_CHUNK_SIZE;
    let mut dry_run = false;

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
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            _ => return Err(format!("未知参数：{arg}，使用 --help 查看帮助").into()),
        }
    }

    Ok(BackupOptions {
        output,
        mailbox,
        max_messages,
        chunk_size,
        dry_run,
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
         --dry-run              只列 UID 和大小，不下载正文\n"
    )
}
