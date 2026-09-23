use foxmail_imap::mail_store::{ImportSummary, IntegrityReport, MailStore, SearchHit};
use serde::Serialize;
use std::env;
use std::error::Error;
use std::path::PathBuf;

#[derive(Debug)]
struct Options {
    data: PathBuf,
    database: Option<PathBuf>,
    archive: Option<PathBuf>,
    /// 用户显式给了 --data 或 --archive；否则 --search 单独使用时跳过导入。
    explicit_import: bool,
    /// 归档归属账号（--account user@host）；多账号索引必填。
    account: Option<String>,
    search: Option<String>,
    limit: usize,
}

#[derive(Serialize)]
struct Output {
    database: String,
    import: ImportSummary,
    message_count: usize,
    integrity: IntegrityReport,
    search: Vec<SearchHit>,
}

fn main() -> Result<(), Box<dyn Error>> {
    let options = parse_options()?;
    let database = options
        .database
        .unwrap_or_else(|| options.data.join("mail-index.sqlite"));
    let mut store = MailStore::open(&database)?;
    let import = match options.archive.as_deref() {
        Some(archive) => {
            let target = options.account.as_deref().map(|user| {
                let (user, server) = user.split_once('@').unwrap_or((user, ""));
                (user, server)
            });
            store.import_archive(archive, target)?
        }
        None if options.explicit_import => store.import_backup(&options.data)?,
        // 仅查询：--search 且未显式要求导入时不动索引。
        None => ImportSummary::default(),
    };
    let message_count = store.message_count()?;
    let integrity = store.integrity_check()?;
    let search = match options.search {
        Some(query) => store.search(&query, options.limit)?,
        None => Vec::new(),
    };

    let output = Output {
        database: database.to_string_lossy().into_owned(),
        import,
        message_count,
        integrity,
        search,
    };
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn parse_options() -> Result<Options, Box<dyn Error>> {
    let mut data = env::var_os("FOXMAIL_BACKUP_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("data"));
    let mut database = None;
    let mut archive = None;
    let mut explicit_import = false;
    let mut account = None;
    let mut search = None;
    let mut limit = 20usize;
    let mut arguments = env::args().skip(1);

    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--data" => {
                data = PathBuf::from(arguments.next().ok_or("--data 缺少路径")?);
                explicit_import = true;
            }
            "--archive" => {
                archive = Some(PathBuf::from(arguments.next().ok_or("--archive 缺少路径")?));
                explicit_import = true;
            }
            "--database" => {
                database = Some(PathBuf::from(
                    arguments.next().ok_or("--database 缺少路径")?,
                ));
            }
            "--search" => search = Some(arguments.next().ok_or("--search 缺少查询词")?),
            "--account" => account = Some(arguments.next().ok_or("--account 缺少 user@host")?),
            "--limit" => {
                limit = arguments
                    .next()
                    .ok_or("--limit 缺少数量")?
                    .parse::<usize>()?;
                if limit == 0 || limit > 1_000 {
                    return Err("--limit 必须在 1..=1000".into());
                }
            }
            "--help" | "-h" => {
                println!(
                    "用法：import-eml [--data <备份目录>] [--archive <散装 .eml 目录>] \
                     [--database <SQLite 文件>] [--account <user@host>] [--search <FTS5 查询>] [--limit <1..=1000>]\n\
                     --archive 导入客户端导出的历史邮件（文件名非 UID），并入收件箱，与 --data 互斥。\n\
                     --account 指定归档归属账号；多账号索引导入归档时必填。\n\
                     单独使用 --search/--limit/--database 时只查询不导入。"
                );
                std::process::exit(0);
            }
            _ => return Err(format!("未知参数：{argument}").into()),
        }
    }

    Ok(Options {
        data,
        database,
        archive,
        explicit_import,
        account,
        search,
        limit,
    })
}
