use rusqlite::ffi::ErrorCode;
use std::error::Error;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexProblem {
    Corrupt,
    CannotOpen,
    QueryFailed,
}

impl IndexProblem {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Corrupt => "corrupt",
            Self::CannotOpen => "cannot_open",
            Self::QueryFailed => "query_failed",
        }
    }
}

pub fn classify_index_error(error: &(dyn Error + 'static)) -> IndexProblem {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(error) = error.downcast_ref::<rusqlite::Error>() {
            return match error.sqlite_error_code() {
                Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase) => IndexProblem::Corrupt,
                Some(ErrorCode::CannotOpen) => IndexProblem::CannotOpen,
                _ => IndexProblem::QueryFailed,
            };
        }
        if let Some(error) = error.downcast_ref::<io::Error>() {
            return match error.kind() {
                io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied => {
                    IndexProblem::CannotOpen
                }
                _ => IndexProblem::QueryFailed,
            };
        }
        current = error.source();
    }
    IndexProblem::QueryFailed
}

pub fn index_message(problem: IndexProblem, operation: &str, index: &Path) -> String {
    let index = index
        .file_name()
        .unwrap_or(index.as_os_str())
        .to_string_lossy();
    match problem {
        IndexProblem::Corrupt => format!(
            "邮件索引已损坏或格式无效：{}。请重新运行 import-eml 生成索引。",
            index
        ),
        IndexProblem::CannotOpen => format!(
            "无法打开邮件索引：{}。请检查文件是否存在、是否有读取权限。",
            index
        ),
        IndexProblem::QueryFailed => {
            format!("{operation}失败，索引结构可能不完整。请重新生成索引并查看日志。")
        }
    }
}

pub fn with_log_path(message: String, log_hint: &str) -> String {
    format!("{message}\n日志：{log_hint}")
}

pub fn with_log_failure(message: String) -> String {
    format!("{message} 日志写入失败，请检查日志目录权限和磁盘空间。")
}

pub fn log_startup(log: &Path, source: &str) -> io::Result<()> {
    append(log, "INFO", "startup", &format!("index_source={source}"))
}

pub fn log_index_problem(
    log: &Path,
    event: &str,
    problem: IndexProblem,
    error: Option<&(dyn Error + 'static)>,
) -> io::Result<()> {
    let fingerprint = error.map_or_else(|| "none".to_owned(), error_fingerprint);
    append(
        log,
        "ERROR",
        event,
        &format!("problem={} {fingerprint}", problem.as_str()),
    )
}

/// Preserve non-index I/O failures without logging paths, message contents or
/// credentials from the error's Display string.
pub fn log_operation_error(
    log: &Path,
    event: &str,
    error: &(dyn Error + 'static),
) -> io::Result<()> {
    append(log, "ERROR", event, &error_fingerprint(error))
}

fn error_fingerprint(error: &(dyn Error + 'static)) -> String {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(error) = error.downcast_ref::<rusqlite::Error>() {
            return format!(
                "sqlite_code={:?} sqlite_extended={:?}",
                error.sqlite_error_code(),
                error.sqlite_extended_error_code()
            );
        }
        if let Some(error) = error.downcast_ref::<io::Error>() {
            return format!(
                "io_kind={:?} os_code={:?}",
                error.kind(),
                error.raw_os_error()
            );
        }
        current = error.source();
    }
    "error=unclassified".to_owned()
}

fn append(log: &Path, level: &str, event: &str, detail: &str) -> io::Result<()> {
    // ponytail: one append-only file; add rotation only if sustained log volume warrants it.
    if let Some(parent) = log.parent() {
        fs::create_dir_all(parent)?;
    }
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut file = OpenOptions::new().create(true).append(true).open(log)?;
    writeln!(
        file,
        "unix={timestamp} level={level} event={event} {detail}"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        classify_index_error, index_message, log_index_problem, with_log_failure, with_log_path,
        IndexProblem,
    };
    use rusqlite::ffi::{Error, ErrorCode, SQLITE_CORRUPT};
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn explains_corrupt_index_without_sqlite_details() {
        let message = index_message(
            IndexProblem::Corrupt,
            "打开",
            Path::new("private/data/mail-index.sqlite"),
        );
        assert!(message.contains("索引已损坏"));
        assert!(message.contains("import-eml"));
        assert!(!message.contains("private"));
    }

    #[test]
    fn classifies_real_corrupt_sqlite_without_matching_error_text() {
        let root = temporary_directory("corrupt");
        let path = root.join("mail-index.sqlite");
        fs::create_dir_all(&root).unwrap();
        fs::write(&path, b"this is not sqlite").unwrap();
        let connection = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let error = connection.prepare("SELECT * FROM mailbox").unwrap_err();
        assert_eq!(classify_index_error(&error), IndexProblem::Corrupt);
        assert!(
            index_message(IndexProblem::Corrupt, "读取", Path::new("index.sqlite"))
                .contains("索引已损坏")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn classifies_sql_query_errors_separately_from_corruption() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        let error = connection
            .query_row("SELECT value FROM missing_table", [], |_| Ok(()))
            .unwrap_err();
        let problem = classify_index_error(&error);
        assert_eq!(problem, IndexProblem::QueryFailed);
        assert!(
            index_message(problem, "搜索邮件", Path::new("index.sqlite")).contains("搜索邮件失败")
        );
    }

    #[test]
    fn classifies_a_directory_as_cannot_open() {
        let root = temporary_directory("directory");
        fs::create_dir_all(&root).unwrap();
        let error = rusqlite::Connection::open_with_flags(
            &root,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap_err();
        assert_eq!(classify_index_error(&error), IndexProblem::CannotOpen);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn log_records_only_structured_fingerprint() {
        let root = temporary_directory("log");
        let log = root.join("foxmail-rs.log");
        let error = rusqlite::Error::SqliteFailure(
            Error {
                code: ErrorCode::DatabaseCorrupt,
                extended_code: SQLITE_CORRUPT,
            },
            Some("secret@example.com password=do-not-log".to_owned()),
        );

        log_index_problem(
            &log,
            "index_open_failed",
            IndexProblem::Corrupt,
            Some(&error),
        )
        .unwrap();
        let contents = fs::read_to_string(&log).unwrap();
        assert!(contents.contains("event=index_open_failed"));
        assert!(contents.contains("sqlite_code=Some(DatabaseCorrupt)"));
        assert!(!contents.contains("secret@example.com"));
        assert!(!contents.contains("do-not-log"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn user_message_names_the_log_file() {
        let message = with_log_path("查询失败".to_owned(), r"程序目录\logs\foxmail-rs.log");
        assert!(message.contains("\n日志："));
        assert!(message.contains(r"程序目录\logs\foxmail-rs.log"));
        let message = with_log_failure("查询失败".to_owned());
        assert!(message.contains("日志写入失败"));
        assert!(message.contains("权限和磁盘空间"));
    }

    #[test]
    fn reports_when_the_log_parent_is_not_a_directory() {
        let root = temporary_directory("unwritable-log");
        fs::write(&root, b"not a directory").unwrap();
        let result = log_index_problem(
            &root.join("foxmail-rs.log"),
            "search_failed",
            IndexProblem::QueryFailed,
            None,
        );
        assert!(result.is_err());
        let _ = fs::remove_file(root);
    }

    fn temporary_directory(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "foxmail-rs-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }
}
