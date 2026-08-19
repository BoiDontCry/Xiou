use crate::imap_utf7;
use mail_parser::{Address, MessageParser, MimeHeaders};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Transaction};
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

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SearchHit {
    pub id: i64,
    pub mailbox_id: i64,
    pub mailbox: String,
    pub uid: u32,
    pub subject: String,
    pub sender: String,
    pub recipients: String,
    pub date_text: Option<String>,
    pub snippet: String,
    pub eml_path: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct MailboxAccount {
    pub account: String,
    pub server: String,
    pub mailboxes: Vec<MailboxSummary>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct MailboxSummary {
    pub id: i64,
    pub source_name: String,
    pub display_name: String,
    pub message_count: usize,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct MessageSummary {
    pub id: i64,
    pub uid: u32,
    pub subject: String,
    pub sender: String,
    pub date_text: Option<String>,
    pub date_unix: Option<i64>,
    pub attachment_count: usize,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct AttachmentMeta {
    pub filename: String,
    pub content_type: String,
    pub size_bytes: usize,
    pub inline: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct MessageDetail {
    pub id: i64,
    pub mailbox_id: i64,
    pub uid: u32,
    pub subject: String,
    pub sender: String,
    pub recipients: String,
    pub date_text: Option<String>,
    pub date_unix: Option<i64>,
    pub body: String,
    pub html_body: String,
    pub attachment_count: usize,
    pub attachments: Vec<AttachmentMeta>,
    pub attachment_load_error: Option<String>,
    pub size_bytes: usize,
    pub eml_path: String,
}

pub struct MailStore {
    connection: Connection,
    source_root: Option<PathBuf>,
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
        Ok(Self {
            connection,
            source_root: source_root(path),
        })
    }

    pub fn open_read_only(path: &Path) -> StoreResult<Self> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA query_only = ON;")?;
        Ok(Self {
            connection,
            source_root: source_root(path),
        })
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
                    m.mailbox_id,
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
                mailbox_id: row.get(1)?,
                mailbox: row.get(2)?,
                uid: row.get(3)?,
                subject: row.get(4)?,
                sender: row.get(5)?,
                recipients: row.get(6)?,
                date_text: row.get(7)?,
                snippet: row.get(8)?,
                eml_path: row.get(9)?,
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

    pub fn mailbox_tree(&self) -> StoreResult<Vec<MailboxAccount>> {
        let mut statement = self.connection.prepare(
            "WITH mailbox_generation AS (
                 SELECT mb.*,
                        ROW_NUMBER() OVER (
                            PARTITION BY mb.account,
                                         mb.server,
                                         CASE
                                             WHEN mb.source_name COLLATE NOCASE = 'INBOX'
                                             THEN 'INBOX'
                                             ELSE mb.source_name
                                         END
                            ORDER BY mb.scanned_at_unix DESC,
                                     mb.uid_validity DESC,
                                     mb.id DESC
                        ) AS generation_rank
                 FROM mailbox AS mb
             )
             SELECT mb.id,
                    mb.account,
                    mb.server,
                    mb.source_name,
                    mb.display_name,
                    COUNT(m.id)
             FROM mailbox_generation AS mb
             LEFT JOIN message AS m ON m.mailbox_id = mb.id
             WHERE mb.generation_rank = 1
             GROUP BY mb.id
             ORDER BY mb.account COLLATE NOCASE,
                      mb.account,
                      mb.server COLLATE NOCASE,
                      mb.server,
                      CASE WHEN mb.source_name COLLATE NOCASE = 'INBOX' THEN 0 ELSE 1 END,
                      mb.display_name COLLATE NOCASE,
                      mb.source_name COLLATE NOCASE",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })?;

        let mut accounts: Vec<MailboxAccount> = Vec::new();
        for row in rows {
            let (id, account, server, source_name, display_name, message_count) = row?;
            let starts_new_account = accounts
                .last()
                .is_none_or(|item| item.account != account || item.server != server);
            if starts_new_account {
                accounts.push(MailboxAccount {
                    account: account.clone(),
                    server: server.clone(),
                    mailboxes: Vec::new(),
                });
            }
            accounts
                .last_mut()
                .expect("account was inserted above")
                .mailboxes
                .push(MailboxSummary {
                    id,
                    source_name,
                    display_name,
                    message_count: usize::try_from(message_count)?,
                });
        }
        Ok(accounts)
    }

    pub fn message_page(
        &self,
        mailbox_id: i64,
        offset: usize,
        limit: usize,
    ) -> StoreResult<Vec<MessageSummary>> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let offset = i64::try_from(offset)?;
        let limit = i64::try_from(limit.min(200))?;
        let mut statement = self.connection.prepare(
            "SELECT id,
                    uid,
                    subject,
                    sender,
                    date_text,
                    date_unix,
                    attachment_count
             FROM message
             WHERE mailbox_id = ?1
             ORDER BY date_unix DESC, uid DESC
             LIMIT ?2 OFFSET ?3",
        )?;
        let rows = statement.query_map(params![mailbox_id, limit, offset], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })?;

        let mut messages = Vec::new();
        for row in rows {
            let (id, uid, subject, sender, date_text, date_unix, attachment_count) = row?;
            messages.push(MessageSummary {
                id,
                uid,
                subject,
                sender,
                date_text,
                date_unix,
                attachment_count: usize::try_from(attachment_count)?,
            });
        }
        Ok(messages)
    }

    pub fn message_detail(&self, message_id: i64) -> StoreResult<Option<MessageDetail>> {
        let row = self
            .connection
            .query_row(
                "SELECT id,
                        mailbox_id,
                        uid,
                        subject,
                        sender,
                        recipients,
                        date_text,
                        date_unix,
                        body,
                        html_body,
                        attachment_count,
                        size_bytes,
                        eml_path
                 FROM message
                 WHERE id = ?1",
                params![message_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, u32>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<i64>>(7)?,
                        row.get::<_, String>(8)?,
                        row.get::<_, String>(9)?,
                        row.get::<_, i64>(10)?,
                        row.get::<_, i64>(11)?,
                        row.get::<_, String>(12)?,
                    ))
                },
            )
            .optional()?;

        let Some((
            id,
            mailbox_id,
            uid,
            subject,
            sender,
            recipients,
            date_text,
            date_unix,
            body,
            html_body,
            attachment_count,
            size_bytes,
            eml_path,
        )) = row
        else {
            return Ok(None);
        };

        let attachment_path = self.resolve_source_path(&eml_path);
        let (attachments, attachment_load_error) = match read_attachment_metadata(&attachment_path)
        {
            Ok(attachments) => (attachments, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };

        Ok(Some(MessageDetail {
            id,
            mailbox_id,
            uid,
            subject,
            sender,
            recipients,
            date_text,
            date_unix,
            body,
            html_body,
            attachment_count: usize::try_from(attachment_count)?,
            attachments,
            attachment_load_error,
            size_bytes: usize::try_from(size_bytes)?,
            eml_path,
        }))
    }

    fn resolve_source_path(&self, stored_path: &str) -> PathBuf {
        let path = Path::new(stored_path);
        if path.is_absolute() {
            path.to_owned()
        } else {
            self.source_root
                .as_deref()
                .map_or_else(|| path.to_owned(), |root| root.join(path))
        }
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

fn source_root(database_path: &Path) -> Option<PathBuf> {
    database_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
}

fn read_attachment_metadata(path: &Path) -> StoreResult<Vec<AttachmentMeta>> {
    let raw = fs::read(path)?;
    let message = MessageParser::default()
        .parse(&raw)
        .ok_or_else(|| format!("无法解析 RFC822/MIME 邮件：{}", path.display()))?;

    Ok(message
        .attachments()
        .enumerate()
        .map(|(index, part)| {
            let content_type = part
                .content_type()
                .map(|content_type| {
                    format!(
                        "{}/{}",
                        content_type.ctype(),
                        content_type.subtype().unwrap_or("octet-stream")
                    )
                })
                .unwrap_or_else(|| "application/octet-stream".to_owned());
            let filename = part
                .attachment_name()
                .filter(|name| !name.trim().is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| format!("附件 {}", index + 1));
            let inline = part
                .content_disposition()
                .is_some_and(|disposition| disposition.is_inline());

            AttachmentMeta {
                filename,
                content_type,
                size_bytes: part.len(),
                inline,
            }
        })
        .collect())
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
    use super::{read_attachment_metadata, MailStore};
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
        assert!(hits[0].mailbox_id > 0);
        assert_eq!(hits[0].mailbox, "台北");
        assert_eq!(hits[0].uid, 7);
        assert!(store.search("不存在的词", 10).unwrap().is_empty());
        assert!(store.search("\"", 10).is_err());

        let second = store.import_backup(&root).unwrap();
        assert_eq!(second.inserted, 0);
        assert_eq!(second.updated, 0);
        assert_eq!(second.skipped, 1);

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn returns_mailboxes_grouped_by_account_with_inbox_first() {
        let root = temporary_directory("mailbox-tree");
        write_mailbox(&root, "zeta@example.com", "&XfJT0ZAB-", 42, 123, &[]);
        write_mailbox(
            &root,
            "alpha@example.com",
            "&XfJT0ZAB-",
            42,
            123,
            &[(7, "第一封"), (8, "第二封")],
        );
        write_mailbox(
            &root,
            "alpha@example.com",
            "Inbox",
            42,
            100,
            &[(9, "收件箱邮件")],
        );
        write_mailbox(
            &root,
            "alpha@example.com",
            "INBOX",
            43,
            200,
            &[(10, "新世代一"), (11, "新世代二")],
        );
        write_mailbox(
            &root,
            "Alpha@example.com",
            "inbox",
            42,
            123,
            &[(12, "大小写收件箱")],
        );
        write_mailbox(&root, "Alpha@example.com", "&XfJT0ZAB-", 42, 123, &[]);

        let database = root.join("mail.sqlite");
        let mut store = MailStore::open(&database).unwrap();
        store.import_backup(&root).unwrap();

        drop(store);
        let store = MailStore::open_read_only(&database).unwrap();
        let accounts = store.mailbox_tree().unwrap();
        assert_eq!(accounts.len(), 3);
        assert_eq!(accounts[0].account, "Alpha@example.com");
        assert_eq!(accounts[0].mailboxes.len(), 2);
        assert_eq!(accounts[0].mailboxes[0].source_name, "inbox");
        assert_eq!(accounts[0].mailboxes[0].message_count, 1);
        assert_eq!(accounts[1].account, "alpha@example.com");
        assert_eq!(accounts[1].mailboxes.len(), 2);
        assert_eq!(accounts[1].mailboxes[0].source_name, "INBOX");
        assert_eq!(accounts[1].mailboxes[0].message_count, 2);
        assert_eq!(accounts[1].mailboxes[1].display_name, "已发送");
        assert_eq!(accounts[1].mailboxes[1].message_count, 2);
        assert_ne!(accounts[1].mailboxes[0].id, accounts[1].mailboxes[1].id);
        assert_eq!(accounts[2].account, "zeta@example.com");
        assert_eq!(accounts[2].mailboxes[0].message_count, 0);

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pages_message_summaries_in_stable_newest_first_order() {
        let root = temporary_directory("message-page");
        write_mailbox(
            &root,
            "reader@example.com",
            "INBOX",
            42,
            123,
            &[(7, "第七封"), (9, "第九封"), (8, "第八封")],
        );

        let database = root.join("mail.sqlite");
        let mut store = MailStore::open(&database).unwrap();
        store.import_backup(&root).unwrap();
        let mailbox_id = store.mailbox_tree().unwrap()[0].mailboxes[0].id;

        drop(store);
        let store = MailStore::open_read_only(&database).unwrap();
        let first_page = store.message_page(mailbox_id, 0, 2).unwrap();
        assert_eq!(
            first_page
                .iter()
                .map(|message| message.uid)
                .collect::<Vec<_>>(),
            vec![9, 8]
        );
        assert_eq!(first_page[0].subject, "第九封");
        assert_eq!(first_page[0].sender, "Sender <sender@example.com>");
        assert!(first_page[0].date_unix.is_some());

        let detail = store
            .message_detail(first_page[0].id)
            .unwrap()
            .expect("message detail should exist");
        assert_eq!(detail.mailbox_id, mailbox_id);
        assert_eq!(detail.uid, 9);
        assert_eq!(detail.subject, "第九封");
        assert_eq!(detail.sender, "Sender <sender@example.com>");
        assert_eq!(detail.recipients, "receiver@example.com");
        assert!(detail.body.contains("正文。"));
        assert!(detail.html_body.contains("正文。"));
        assert_eq!(detail.attachment_count, 0);
        assert!(detail.attachments.is_empty());
        assert!(detail.attachment_load_error.is_none());
        assert!(detail.size_bytes > 0);
        assert!(detail.eml_path.ends_with("0000000009.eml"));

        let second_page = store.message_page(mailbox_id, 2, 2).unwrap();
        assert_eq!(
            second_page
                .iter()
                .map(|message| message.uid)
                .collect::<Vec<_>>(),
            vec![7]
        );
        assert!(store.message_page(mailbox_id, 0, 0).unwrap().is_empty());
        assert!(store.message_page(i64::MAX, 0, 10).unwrap().is_empty());
        assert!(store.message_detail(i64::MAX).unwrap().is_none());

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reads_attachment_metadata_from_multipart_eml() {
        let root = temporary_directory("attachment-metadata");
        fs::create_dir_all(&root).unwrap();
        let eml = root.join("attachment.eml");
        fs::write(
            &eml,
            "From: sender@example.com\r\n\
To: receiver@example.com\r\n\
Date: Mon, 17 Aug 2026 09:00:00 +0800\r\n\
Subject: Attachment metadata\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=foxmail-boundary\r\n\
\r\n\
--foxmail-boundary\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
hello\r\n\
--foxmail-boundary\r\n\
Content-Type: application/octet-stream\r\n\
Content-Disposition: attachment; filename=test.bin\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
AAE=\r\n\
--foxmail-boundary--\r\n",
        )
        .unwrap();

        let attachments = read_attachment_metadata(&eml).unwrap();
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].filename, "test.bin");
        assert_eq!(attachments[0].content_type, "application/octet-stream");
        assert_eq!(attachments[0].size_bytes, 2);
        assert!(!attachments[0].inline);

        fs::remove_dir_all(root).unwrap();
    }

    fn write_mailbox(
        root: &Path,
        account: &str,
        mailbox_name: &str,
        uid_validity: u32,
        scanned_at_unix: u64,
        messages: &[(u32, &str)],
    ) {
        let mailbox = root
            .join(fixture_component(account))
            .join(fixture_component(mailbox_name))
            .join(format!("uidvalidity-{uid_validity}"));
        fs::create_dir_all(&mailbox).unwrap();
        fs::write(
            mailbox.join("mailbox.meta.json"),
            serde_json::to_vec_pretty(&json!({
                "account": account,
                "server": "imap.example.com",
                "mailbox": mailbox_name,
                "uid_validity": uid_validity,
                "scanned_at_unix": scanned_at_unix,
                "server_message_count": messages.len(),
                "discovered_uid_count": messages.len()
            }))
            .unwrap(),
        )
        .unwrap();

        for (uid, subject) in messages {
            fs::write(
                mailbox.join(format!("{uid:010}.eml")),
                format!(
                    "From: Sender <sender@example.com>\r\n\
To: receiver@example.com\r\n\
Date: Mon, 17 Aug 2026 09:00:00 +0800\r\n\
Subject: {subject}\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
Content-Transfer-Encoding: 8bit\r\n\
\r\n\
正文。\r\n"
                ),
            )
            .unwrap();
        }
    }

    fn fixture_component(value: &str) -> String {
        value
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join("")
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
