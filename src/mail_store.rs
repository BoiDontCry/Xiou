use crate::imap_utf7;
use mail_parser::{Address, MessageParser};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;

CREATE TABLE IF NOT EXISTS mailbox (
    id INTEGER PRIMARY KEY,
    account TEXT NOT NULL,
    server TEXT NOT NULL,
    source_name TEXT NOT NULL,
    display_name TEXT NOT NULL,
    uid_validity INTEGER NOT NULL,
    scanned_at_unix INTEGER NOT NULL,
    server_message_count INTEGER NOT NULL,
    discovered_uid_count INTEGER NOT NULL,
    UNIQUE(account, server, source_name, uid_validity)
);

CREATE TABLE IF NOT EXISTS message (
    id INTEGER PRIMARY KEY,
    mailbox_id INTEGER NOT NULL REFERENCES mailbox(id) ON DELETE CASCADE,
    uid INTEGER NOT NULL,
    eml_path TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    modified_unix INTEGER NOT NULL,
    subject TEXT NOT NULL,
    sender TEXT NOT NULL,
    recipients TEXT NOT NULL,
    date_text TEXT,
    date_unix INTEGER,
    body TEXT NOT NULL,
    html_body TEXT NOT NULL,
    attachment_count INTEGER NOT NULL,
    UNIQUE(mailbox_id, uid)
);

CREATE INDEX IF NOT EXISTS message_mailbox_date_idx
    ON message(mailbox_id, date_unix DESC, uid DESC);

CREATE VIRTUAL TABLE IF NOT EXISTS message_fts USING fts5(
    subject,
    sender,
    recipients,
    body,
    content='message',
    content_rowid='id',
    tokenize='trigram'
);

CREATE TRIGGER IF NOT EXISTS message_ai AFTER INSERT ON message BEGIN
    INSERT INTO message_fts(rowid, subject, sender, recipients, body)
    VALUES (new.id, new.subject, new.sender, new.recipients, new.body);
END;

CREATE TRIGGER IF NOT EXISTS message_ad AFTER DELETE ON message BEGIN
    INSERT INTO message_fts(message_fts, rowid, subject, sender, recipients, body)
    VALUES ('delete', old.id, old.subject, old.sender, old.recipients, old.body);
END;

CREATE TRIGGER IF NOT EXISTS message_au AFTER UPDATE ON message BEGIN
    INSERT INTO message_fts(message_fts, rowid, subject, sender, recipients, body)
    VALUES ('delete', old.id, old.subject, old.sender, old.recipients, old.body);
    INSERT INTO message_fts(rowid, subject, sender, recipients, body)
    VALUES (new.id, new.subject, new.sender, new.recipients, new.body);
END;
"#;

pub type StoreResult<T> = Result<T, Box<dyn Error>>;

#[derive(Debug, Deserialize)]
struct MailboxMetadata {
    account: String,
    server: String,
    mailbox: String,
    uid_validity: u32,
    scanned_at_unix: u64,
    server_message_count: u32,
    discovered_uid_count: usize,
}

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct ImportSummary {
    pub mailboxes: usize,
    pub discovered_messages: usize,
    pub inserted: usize,
    pub updated: usize,
    pub skipped: usize,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct IntegrityReport {
    pub sqlite: String,
    pub fts5_external_content: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SearchHit {
    pub id: i64,
    pub mailbox: String,
    pub uid: u32,
    pub subject: String,
    pub sender: String,
    pub recipients: String,
    pub date_text: Option<String>,
    pub snippet: String,
    pub eml_path: String,
}

pub struct MailStore {
    connection: Connection,
}

impl MailStore {
    pub fn open(path: &Path) -> StoreResult<Self> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.execute_batch(SCHEMA)?;
        Ok(Self { connection })
    }

    pub fn import_backup(&mut self, data_root: &Path) -> StoreResult<ImportSummary> {
        let canonical_root = fs::canonicalize(data_root)?;
        let metadata_files = find_metadata_files(&canonical_root)?;
        let transaction = self.connection.transaction()?;
        let mut summary = ImportSummary::default();

        for metadata_path in metadata_files {
            import_mailbox(&transaction, &canonical_root, &metadata_path, &mut summary)?;
        }

        transaction.commit()?;
        Ok(summary)
    }

    pub fn search(&self, query: &str, limit: usize) -> StoreResult<Vec<SearchHit>> {
        let limit = i64::try_from(limit.min(1_000))?;
        let mut statement = self.connection.prepare(
            "SELECT m.id,
                    mb.display_name,
                    m.uid,
                    m.subject,
                    m.sender,
                    m.recipients,
                    m.date_text,
                    snippet(message_fts, 3, '[', ']', '…', 18),
                    m.eml_path
             FROM message_fts
             JOIN message AS m ON m.id = message_fts.rowid
             JOIN mailbox AS mb ON mb.id = m.mailbox_id
             WHERE message_fts MATCH ?1
             ORDER BY bm25(message_fts), m.date_unix DESC, m.uid DESC
             LIMIT ?2",
        )?;
        let rows = statement.query_map(params![query, limit], |row| {
            Ok(SearchHit {
                id: row.get(0)?,
                mailbox: row.get(1)?,
                uid: row.get(2)?,
                subject: row.get(3)?,
                sender: row.get(4)?,
                recipients: row.get(5)?,
                date_text: row.get(6)?,
                snippet: row.get(7)?,
                eml_path: row.get(8)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn message_count(&self) -> StoreResult<usize> {
        let count: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM message", [], |row| row.get(0))?;
        Ok(usize::try_from(count)?)
    }

    pub fn integrity_check(&self) -> StoreResult<IntegrityReport> {
        let sqlite = self
            .connection
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))?;
        if sqlite != "ok" {
            return Err(format!("SQLite integrity_check 失败：{sqlite}").into());
        }
        self.connection.execute(
            "INSERT INTO message_fts(message_fts, rank) VALUES('integrity-check', 1)",
            [],
        )?;
        Ok(IntegrityReport {
            sqlite,
            fts5_external_content: "ok".to_owned(),
        })
    }
}

fn import_mailbox(
    transaction: &Transaction<'_>,
    data_root: &Path,
    metadata_path: &Path,
    summary: &mut ImportSummary,
) -> StoreResult<()> {
    let metadata: MailboxMetadata = serde_json::from_slice(&fs::read(metadata_path)?)?;
    let display_name = imap_utf7::decode(&metadata.mailbox)?;
    let mailbox_id = upsert_mailbox(transaction, &metadata, &display_name)?;
    summary.mailboxes += 1;

    let mailbox_directory = metadata_path
        .parent()
        .ok_or_else(|| format!("metadata 缺少父目录：{}", metadata_path.display()))?;
    let eml_files = list_eml_files(mailbox_directory)?;
    summary.discovered_messages += eml_files.len();

    for eml_path in eml_files {
        let uid = parse_uid(&eml_path)?;
        let source = source_fingerprint(data_root, &eml_path)?;
        let existing = transaction
            .query_row(
                "SELECT eml_path, size_bytes, modified_unix
                 FROM message
                 WHERE mailbox_id = ?1 AND uid = ?2",
                params![mailbox_id, uid],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()?;

        if existing
            .as_ref()
            .is_some_and(|existing| existing == &source)
        {
            summary.skipped += 1;
            continue;
        }

        let raw = fs::read(&eml_path)?;
        let message = MessageParser::default()
            .parse(&raw)
            .ok_or_else(|| format!("无法解析 RFC822/MIME 邮件：{}", eml_path.display()))?;
        let subject = message.subject().unwrap_or_default().trim().to_owned();
        let sender = format_addresses(message.from());
        let recipients = join_nonempty([
            format_addresses(message.to()),
            format_addresses(message.cc()),
        ]);
        let date_text = message.date().map(ToString::to_string);
        let date_unix = message.date().map(|date| date.to_timestamp());
        let text_body = collect_bodies(message.text_body_count(), |index| {
            message.body_text(index).map(|body| body.into_owned())
        });
        let html_body = collect_bodies(message.html_body_count(), |index| {
            message.body_html(index).map(|body| body.into_owned())
        });
        let searchable_body = if text_body.trim().is_empty() {
            html_body.clone()
        } else {
            text_body
        };
        let attachment_count = i64::try_from(message.attachment_count())?;

        transaction.execute(
            "INSERT INTO message (
                 mailbox_id, uid, eml_path, size_bytes, modified_unix,
                 subject, sender, recipients, date_text, date_unix,
                 body, html_body, attachment_count
             ) VALUES (
                 ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13
             )
             ON CONFLICT(mailbox_id, uid) DO UPDATE SET
                 eml_path = excluded.eml_path,
                 size_bytes = excluded.size_bytes,
                 modified_unix = excluded.modified_unix,
                 subject = excluded.subject,
                 sender = excluded.sender,
                 recipients = excluded.recipients,
                 date_text = excluded.date_text,
                 date_unix = excluded.date_unix,
                 body = excluded.body,
                 html_body = excluded.html_body,
                 attachment_count = excluded.attachment_count",
            params![
                mailbox_id,
                uid,
                source.0,
                source.1,
                source.2,
                subject,
                sender,
                recipients,
                date_text,
                date_unix,
                searchable_body,
                html_body,
                attachment_count,
            ],
        )?;

        if existing.is_some() {
            summary.updated += 1;
        } else {
            summary.inserted += 1;
        }
    }

    Ok(())
}

fn upsert_mailbox(
    transaction: &Transaction<'_>,
    metadata: &MailboxMetadata,
    display_name: &str,
) -> StoreResult<i64> {
    let uid_validity = i64::from(metadata.uid_validity);
    let scanned_at_unix = i64::try_from(metadata.scanned_at_unix)?;
    let server_message_count = i64::from(metadata.server_message_count);
    let discovered_uid_count = i64::try_from(metadata.discovered_uid_count)?;

    transaction.execute(
        "INSERT INTO mailbox (
             account, server, source_name, display_name, uid_validity,
             scanned_at_unix, server_message_count, discovered_uid_count
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(account, server, source_name, uid_validity) DO UPDATE SET
             display_name = excluded.display_name,
             scanned_at_unix = excluded.scanned_at_unix,
             server_message_count = excluded.server_message_count,
             discovered_uid_count = excluded.discovered_uid_count",
        params![
            metadata.account,
            metadata.server,
            metadata.mailbox,
            display_name,
            uid_validity,
            scanned_at_unix,
            server_message_count,
            discovered_uid_count,
        ],
    )?;

    Ok(transaction.query_row(
        "SELECT id FROM mailbox
         WHERE account = ?1 AND server = ?2 AND source_name = ?3 AND uid_validity = ?4",
        params![
            metadata.account,
            metadata.server,
            metadata.mailbox,
            uid_validity,
        ],
        |row| row.get(0),
    )?)
}

fn find_metadata_files(root: &Path) -> StoreResult<Vec<PathBuf>> {
    let mut found = Vec::new();
    visit_directories(root, &mut |path| {
        let candidate = path.join("mailbox.meta.json");
        if candidate.is_file() {
            found.push(candidate);
        }
        Ok(())
    })?;
    found.sort();
    Ok(found)
}

fn visit_directories(
    directory: &Path,
    visitor: &mut impl FnMut(&Path) -> StoreResult<()>,
) -> StoreResult<()> {
    visitor(directory)?;
    let mut child_directories = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    child_directories.sort();
    for child in child_directories {
        if child.is_dir() {
            visit_directories(&child, visitor)?;
        }
    }
    Ok(())
}

fn list_eml_files(directory: &Path) -> StoreResult<Vec<PathBuf>> {
    let mut files = fs::read_dir(directory)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("eml"))
        })
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn parse_uid(path: &Path) -> StoreResult<u32> {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| format!("邮件文件名不是有效 UTF-8：{}", path.display()))?;
    Ok(stem
        .parse::<u32>()
        .map_err(|error| format!("无法从邮件文件名提取 UID（{stem}）：{error}"))?)
}

fn source_fingerprint(data_root: &Path, path: &Path) -> StoreResult<(String, i64, i64)> {
    let metadata = fs::metadata(path)?;
    let relative = path.strip_prefix(data_root)?;
    let normalized_path = relative.to_string_lossy().replace('\\', "/");
    let size = i64::try_from(metadata.len())?;
    let modified = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let modified_nanos = i64::try_from(modified.as_nanos())?;
    Ok((normalized_path, size, modified_nanos))
}

fn format_addresses(addresses: Option<&Address<'_>>) -> String {
    addresses
        .into_iter()
        .flat_map(Address::iter)
        .filter_map(|address| match (address.name(), address.address()) {
            (Some(name), Some(email)) if !name.trim().is_empty() => {
                Some(format!("{} <{}>", name.trim(), email.trim()))
            }
            (_, Some(email)) if !email.trim().is_empty() => Some(email.trim().to_owned()),
            (Some(name), _) if !name.trim().is_empty() => Some(name.trim().to_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn join_nonempty<const N: usize>(parts: [String; N]) -> String {
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

fn collect_bodies(mut count: usize, mut get: impl FnMut(usize) -> Option<String>) -> String {
    let mut bodies = Vec::with_capacity(count);
    let mut index = 0;
    while count > 0 {
        if let Some(body) = get(index).filter(|body| !body.trim().is_empty()) {
            bodies.push(body);
        }
        index += 1;
        count -= 1;
    }
    bodies.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::MailStore;
    use serde_json::json;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn imports_searches_and_skips_unchanged_messages() {
        let root = temporary_directory("mail-store");
        let mailbox = root.join("account").join("mailbox").join("uidvalidity-42");
        fs::create_dir_all(&mailbox).unwrap();
        fs::write(
            mailbox.join("mailbox.meta.json"),
            serde_json::to_vec_pretty(&json!({
                "account": "user@example.com",
                "server": "imap.example.com",
                "mailbox": "&U,BTFw-",
                "uid_validity": 42,
                "scanned_at_unix": 123,
                "server_message_count": 1,
                "discovered_uid_count": 1
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            mailbox.join("0000000007.eml"),
            "From: Sender <sender@example.com>\r\n\
To: receiver@example.com\r\n\
Date: Mon, 17 Aug 2026 09:00:00 +0800\r\n\
Subject: Import test\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
Content-Transfer-Encoding: 8bit\r\n\
\r\n\
这是一封用于中文全文检索的邮件。\r\n",
        )
        .unwrap();

        let database = root.join("mail.sqlite");
        let mut store = MailStore::open(&database).unwrap();
        let first = store.import_backup(&root).unwrap();
        assert_eq!(first.mailboxes, 1);
        assert_eq!(first.inserted, 1);
        assert_eq!(first.skipped, 0);
        assert_eq!(store.message_count().unwrap(), 1);
        let integrity = store.integrity_check().unwrap();
        assert_eq!(integrity.sqlite, "ok");
        assert_eq!(integrity.fts5_external_content, "ok");

        let hits = store.search("中文全文", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].mailbox, "台北");
        assert_eq!(hits[0].uid, 7);

        let second = store.import_backup(&root).unwrap();
        assert_eq!(second.inserted, 0);
        assert_eq!(second.updated, 0);
        assert_eq!(second.skipped, 1);

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    fn temporary_directory(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("foxmail-rs-{label}-{unique}"));
        assert!(!Path::new(&path).exists());
        path
    }
}
