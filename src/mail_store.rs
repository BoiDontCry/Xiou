use crate::html;
use crate::imap_utf7;
use crate::search_query::{self, ParsedQuery};
use crate::mime_plan::{AttachmentSidecar, DeferredAttachment};
use mail_parser::{Address, MessageParser, MimeHeaders};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fs;
use std::io::Write;
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
    message_id TEXT NOT NULL DEFAULT '',
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

pub enum AttachmentPayload {
    Bytes(Vec<u8>),
    Deferred(DeferredAttachment),
}

/// 本地归档邮箱的 UIDVALIDITY 哨兵值。RFC 3501 规定真实 UIDVALIDITY 非零，故 0 可安全占用。
/// 归档邮件已改为并入收件箱，此值现在只用于识别并清理早期版本留下的独立归档邮箱。
const ARCHIVE_UID_VALIDITY: u32 = 0;

/// 归档邮件的 UID 基址。真实 IMAP UID 目前在 ~1.15e6 量级，3e9 起步不会相撞，
/// 且 3e9 + 归档数量仍在 u32 范围内。
const ARCHIVE_UID_BASE: u32 = 3_000_000_000;

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
    pub content_id: Option<String>,
}

struct MessageRow {
    id: i64,
    mailbox_id: i64,
    uid: u32,
    subject: String,
    sender: String,
    recipients: String,
    date_text: Option<String>,
    date_unix: Option<i64>,
    body: String,
    html_body: String,
    attachment_count: i64,
    size_bytes: i64,
    eml_path: String,
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
        migrate(&connection)?;
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

    /// 导入散装 `.eml` 目录（Foxmail 等客户端导出的历史邮件，文件名是主题而非 UID）。
    /// 邮件直接并入该账号的收件箱，与 IMAP 邮件同列表按日期排序；`eml_path` 存绝对路径，
    /// 因为归档目录通常不在索引目录下。
    pub fn import_archive(&mut self, archive_root: &Path) -> StoreResult<ImportSummary> {
        let root = if archive_root.is_absolute() {
            archive_root.to_path_buf()
        } else {
            std::env::current_dir()?.join(archive_root)
        };
        let (account, server) = self.sole_account()?;
        let files = list_eml_files_recursive(&root)?;
        let discovered = files.len();
        let source_root = self.source_root.clone();

        let transaction = self.connection.transaction()?;
        let mailbox_id = inbox_mailbox_id(&transaction, &account, &server)?;
        // 早期版本把归档挂成独立「本地归档」文件夹（uid_validity 哨兵 0）。改为并入收件箱后
        // 必须清掉遗留哨兵邮箱，否则同一批邮件会同时出现在两处。
        remove_archive_mailboxes(&transaction, &account, &server)?;
        // 归档行完全由归档目录派生：先清掉上一次导入的归档行，再整批重建。
        // 这样目录里删掉的邮件不会留残行，也不必猜哪一行还对应哪个文件。
        // 代价是每次都重解析整个归档目录（645 封 / 5 GB 约 25 秒），手动操作可接受。
        transaction.execute(
            "DELETE FROM message WHERE mailbox_id = ?1 AND uid >= ?2",
            params![mailbox_id, ARCHIVE_UID_BASE],
        )?;
        // 去重要靠 Message-ID 比对，已同步的 IMAP 邮件行可能是迁移前入库的空值，先补齐。
        backfill_message_ids(&transaction, mailbox_id, source_root.as_deref())?;
        let mut summary = ImportSummary {
            mailboxes: 1,
            discovered_messages: discovered,
            ..ImportSummary::default()
        };
        // ponytail: UID = 基址 + 排序后序号，散装文件没有服务器 UID；基址远离真实 UID 段以免撞键。
        for (index, eml_path) in files.iter().enumerate() {
            let offset = u32::try_from(index + 1)?;
            let uid = ARCHIVE_UID_BASE
                .checked_add(offset)
                .ok_or("归档邮件数超出 UID 空间")?;
            let source = absolute_fingerprint(eml_path)?;
            import_eml_file(
                &transaction,
                mailbox_id,
                uid,
                eml_path,
                source,
                true,
                &mut summary,
            )?;
        }
        transaction.commit()?;
        Ok(summary)
    }

    /// 归档要挂到哪个账号：取索引里已有的唯一 IMAP 账号（排除归档自身）。
    fn sole_account(&self) -> StoreResult<(String, String)> {
        let mut statement = self
            .connection
            .prepare("SELECT DISTINCT account, server FROM mailbox WHERE uid_validity <> 0")?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        match rows.len() {
            1 => Ok(rows.into_iter().next().expect("已确认恰好一行")),
            0 => Err("索引里还没有 IMAP 账号，请先完成一次同步再导入归档".into()),
            count => Err(format!("索引里有 {count} 个账号，归档归属不明确").into()),
        }
    }

    pub fn search(&self, query: &str, limit: usize) -> StoreResult<Vec<SearchHit>> {
        let parsed = search_query::parse(query);
        if !has_positive_selector(&parsed) {
            // 无任何匹配条件（含 <3 字纯词查询）→ LIKE 兜底。
            let limit = i64::try_from(limit.min(1_000))?;
            return self.search_like(query, limit);
        }
        let limit = i64::try_from(limit.min(1_000))?;
        let cap = (limit * 4).clamp(50, 400);

        let mut ranked = self.search_ranked_ids(&parsed, cap)?;
        // 全词 AND 无结果时放宽为 OR 重试（多术语查询才有意义）。
        if ranked.is_empty() && !parsed.fts.is_empty() && parsed.fts_or != parsed.fts {
            let relaxed = ParsedQuery {
                fts: parsed.fts_or.clone(),
                ..parsed.clone()
            };
            ranked = self.search_ranked_ids(&relaxed, cap)?;
        }
        self.hydrate_hits(ranked, &parsed, limit)
    }

    /// 词法检索：bm25 加权（标题 4 > 发件人/收件人 2 > 正文 1）+
    /// 操作符过滤，返回 (id, rank)，rank 越小越优。
    fn search_ranked_ids(&self, q: &ParsedQuery, cap: i64) -> StoreResult<Vec<(i64, f64)>> {
        let mut binds: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        let mut clauses: Vec<String> = Vec::new();

        // bm25() 只能在直接查询 FTS 表时求值，故在 MATERIALIZED CTE 内排序。
        let (cte, fts_join, rank_expr) = if q.fts.is_empty() {
            (String::new(), String::new(), "0.0".to_owned())
        } else {
            binds.push(Box::new(q.fts.clone()));
            (
                format!(
                    "WITH f AS MATERIALIZED (
                         SELECT rowid AS mid,
                                bm25(message_fts, 4.0, 2.0, 2.0, 1.0) AS fts_rank
                         FROM message_fts
                         WHERE message_fts MATCH ?1
                         ORDER BY fts_rank
                         LIMIT {cap})"
                ),
                "JOIN f ON f.mid = m.id".to_owned(),
                "f.fts_rank".to_owned(),
            )
        };

        if let Some(from) = &q.from {
            binds.push(Box::new(format!("%{}%", escape_like_pattern(from))));
            clauses.push(format!(
                "m.sender LIKE ?{n} ESCAPE char(92) COLLATE NOCASE",
                n = binds.len()
            ));
        }
        if let Some(to) = &q.to {
            binds.push(Box::new(format!("%{}%", escape_like_pattern(to))));
            clauses.push(format!(
                "m.recipients LIKE ?{n} ESCAPE char(92) COLLATE NOCASE",
                n = binds.len()
            ));
        }
        if q.has_attachment {
            clauses.push("m.attachment_count > 0".to_owned());
        }
        let role = mailbox_role_sql("mb");
        if let Some(role_value) = &q.in_role {
            binds.push(Box::new(role_value.clone()));
            clauses.push(format!("{role} = ?{}", binds.len()));
        } else {
            // 有正向匹配条件时默认排除已删除/垃圾箱；in:trash 显式指定才纳入。
            clauses.push(format!("{role} NOT IN ('trash','spam')"));
        }
        for term in &q.like_terms {
            binds.push(Box::new(format!("%{}%", escape_like_pattern(term))));
            let n = binds.len();
            clauses.push(format!(
                "(m.subject LIKE ?{n} ESCAPE char(92) COLLATE NOCASE
                  OR m.body LIKE ?{n} ESCAPE char(92) COLLATE NOCASE
                  OR m.sender LIKE ?{n} ESCAPE char(92) COLLATE NOCASE
                  OR m.recipients LIKE ?{n} ESCAPE char(92) COLLATE NOCASE)"
            ));
        }
        if !q.fts_not.is_empty() {
            binds.push(Box::new(q.fts_not.clone()));
            clauses.push(format!(
                "m.id NOT IN (SELECT rowid FROM message_fts WHERE message_fts MATCH ?{})",
                binds.len()
            ));
        }

        if clauses.is_empty() && q.fts.is_empty() {
            return Ok(Vec::new());
        }
        let where_sql = if clauses.is_empty() {
            String::new()
        } else {
            format!(" AND {}", clauses.join(" AND "))
        };

        let sql = format!(
            "{cte}
             SELECT m.id, {rank_expr}
             FROM message AS m
             JOIN mailbox AS mb ON mb.id = m.mailbox_id
             {fts_join}
             WHERE 1=1{where_sql}
             ORDER BY {rank_expr} ASC, m.date_unix DESC, m.uid DESC
             LIMIT {cap}"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            binds.iter().map(|b| b.as_ref()).collect();
        let rows = statement.query_map(params_ref.as_slice(), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
        })?;
        let out = rows.collect::<Result<Vec<_>, _>>()?;
        Ok(out)
    }

    /// 按 (id, rank) 水合为 SearchHit：发件人匹配 + 新鲜度加分后排序，
    /// 有 FTS 术语时附带正文 snippet。
    fn hydrate_hits(
        &self,
        ranked: Vec<(i64, f64)>,
        q: &ParsedQuery,
        limit: i64,
    ) -> StoreResult<Vec<SearchHit>> {
        if ranked.is_empty() {
            return Ok(Vec::new());
        }

        let mut scored = ranked;
        if !q.terms.is_empty() {
            let id_list = id_in_list(scored.iter().map(|(id, _)| *id));
            let sql = format!(
                "SELECT id, sender, COALESCE(date_unix, 0)
                 FROM message WHERE id IN ({id_list})"
            );
            let mut statement = self.connection.prepare(&sql)?;
            let info: HashMap<i64, (String, i64)> = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })?
                .filter_map(Result::ok)
                .map(|(id, sender, date_unix)| (id, (sender, date_unix)))
                .collect();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            for (id, rank) in scored.iter_mut() {
                let Some((sender, date_unix)) = info.get(id) else {
                    continue;
                };
                let age_days = now.saturating_sub((*date_unix).max(0) as u64) / 86400;
                *rank -= 1.5 * f64::from(sender_match(&q.terms, sender))
                    + 0.3 / (1.0 + f64::from(age_days as u32) / 30.0);
            }
            scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        }

        let top: Vec<i64> = scored
            .iter()
            .take(limit as usize)
            .map(|(id, _)| *id)
            .collect();
        let order: HashMap<i64, usize> =
            top.iter().enumerate().map(|(ix, id)| (*id, ix)).collect();

        let id_list = id_in_list(top.iter().copied());
        let sql = format!(
            "SELECT m.id,
                    m.mailbox_id,
                    mb.display_name,
                    m.uid,
                    m.subject,
                    m.sender,
                    m.recipients,
                    m.date_text,
                    m.eml_path
             FROM message AS m
             JOIN mailbox AS mb ON mb.id = m.mailbox_id
             WHERE m.id IN ({id_list})"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let mut hits: Vec<Option<SearchHit>> = vec![None; top.len()];
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                SearchHit {
                    id: row.get(0)?,
                    mailbox_id: row.get(1)?,
                    mailbox: row.get(2)?,
                    uid: row.get(3)?,
                    subject: row.get(4)?,
                    sender: row.get(5)?,
                    recipients: row.get(6)?,
                    date_text: row.get(7)?,
                    snippet: String::new(),
                    eml_path: row.get(8)?,
                },
            ))
        })?;
        for row in rows {
            let (id, hit) = row?;
            if let Some(&ix) = order.get(&id) {
                hits[ix] = Some(hit);
            }
        }

        if !q.fts.is_empty() {
            let match_ids = top.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT rowid, snippet(message_fts, 3, '[', ']', '…', 24)
                 FROM message_fts
                 WHERE message_fts MATCH ?1 AND rowid IN ({match_ids})"
            );
            let mut statement = self.connection.prepare(&sql)?;
            let snippets: HashMap<i64, String> = statement
                .query_map(params![q.fts], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })?
                .filter_map(Result::ok)
                .collect();
            for hit in hits.iter_mut().flatten() {
                if let Some(snippet) = snippets.get(&hit.id) {
                    hit.snippet = snippet.clone();
                }
            }
        }

        Ok(hits.into_iter().flatten().collect())
    }

    fn search_like(&self, query: &str, limit: i64) -> StoreResult<Vec<SearchHit>> {
        let pattern = format!("%{}%", escape_like_pattern(query));
        let mut statement = self.connection.prepare(
            "SELECT m.id,
                    m.mailbox_id,
                    mb.display_name,
                    m.uid,
                    m.subject,
                    m.sender,
                    m.recipients,
                    m.date_text,
                    m.eml_path
             FROM message AS m
             JOIN mailbox AS mb ON mb.id = m.mailbox_id
             WHERE m.subject LIKE ?1 ESCAPE char(92) COLLATE NOCASE
                OR m.sender LIKE ?1 ESCAPE char(92) COLLATE NOCASE
                OR m.recipients LIKE ?1 ESCAPE char(92) COLLATE NOCASE
                OR m.body LIKE ?1 ESCAPE char(92) COLLATE NOCASE
             ORDER BY m.date_unix DESC, m.uid DESC
             LIMIT ?2",
        )?;
        let rows = statement.query_map(params![pattern, limit], |row| {
            Ok(SearchHit {
                id: row.get(0)?,
                mailbox_id: row.get(1)?,
                mailbox: row.get(2)?,
                uid: row.get(3)?,
                subject: row.get(4)?,
                sender: row.get(5)?,
                recipients: row.get(6)?,
                date_text: row.get(7)?,
                snippet: String::new(),
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

    /// Return the position in the same stable order as message_page without
    /// loading every preceding row. NULL dates sort after dated messages.
    pub fn message_position(&self, mailbox_id: i64, message_id: i64) -> StoreResult<Option<usize>> {
        let target: Option<(Option<i64>, u32)> = self.connection.query_row(
            "SELECT date_unix, uid FROM message WHERE mailbox_id = ?1 AND id = ?2",
            params![mailbox_id,message_id], |row| Ok((row.get(0)?,row.get(1)?)),
        ).optional()?;
        let Some((date,uid))=target else { return Ok(None); };
        let position: i64=self.connection.query_row(
            "SELECT COUNT(*) FROM message WHERE mailbox_id = ?1 AND
             ((?2 IS NULL AND date_unix IS NOT NULL) OR date_unix > ?2 OR (date_unix IS ?2 AND uid > ?3))",
            params![mailbox_id,date,uid], |row| row.get(0),
        )?;
        Ok(Some(usize::try_from(position)?))
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
        let Some(row) = self.message_row(message_id)? else {
            return Ok(None);
        };

        let attachment_path = self.resolve_source_path(&row.eml_path);
        let (attachments, attachment_load_error) = match read_attachment_metadata(&attachment_path)
        {
            Ok(attachments) => (attachments, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };

        Ok(Some(Self::detail_from_row(
            row,
            attachments,
            attachment_load_error,
        )))
    }

    /// 打开邮件详情专用的单次解析路径：只读取并解析一次 `.eml`，
    /// 同时得到附件元数据与全部带 Content-ID 的内嵌资源载荷，
    /// 避免按附件逐个重复读盘解析（大邮件下是 N 次全量解析）。
    pub fn message_detail_with_assets(
        &self,
        message_id: i64,
    ) -> StoreResult<Option<(MessageDetail, HashMap<String, Vec<u8>>)>> {
        let Some(row) = self.message_row(message_id)? else {
            return Ok(None);
        };

        let attachment_path = self.resolve_source_path(&row.eml_path);
        let (attachments, cid_payloads, attachment_load_error) = match read_eml_assets(&attachment_path)
        {
            Ok((attachments, cid_payloads)) => (attachments, cid_payloads, None),
            Err(error) => (Vec::new(), HashMap::new(), Some(error.to_string())),
        };

        let detail = Self::detail_from_row(row, attachments, attachment_load_error);
        Ok(Some((detail, cid_payloads)))
    }

    fn message_row(&self, message_id: i64) -> StoreResult<Option<MessageRow>> {
        self.connection
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
                    Ok(MessageRow {
                        id: row.get(0)?,
                        mailbox_id: row.get(1)?,
                        uid: row.get(2)?,
                        subject: row.get(3)?,
                        sender: row.get(4)?,
                        recipients: row.get(5)?,
                        date_text: row.get(6)?,
                        date_unix: row.get(7)?,
                        body: row.get(8)?,
                        html_body: row.get(9)?,
                        attachment_count: row.get(10)?,
                        size_bytes: row.get(11)?,
                        eml_path: row.get(12)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    fn detail_from_row(
        row: MessageRow,
        attachments: Vec<AttachmentMeta>,
        attachment_load_error: Option<String>,
    ) -> MessageDetail {
        MessageDetail {
            id: row.id,
            mailbox_id: row.mailbox_id,
            uid: row.uid,
            subject: row.subject,
            sender: row.sender,
            recipients: row.recipients,
            date_text: row.date_text,
            date_unix: row.date_unix,
            body: row.body,
            html_body: row.html_body,
            attachment_count: usize::try_from(row.attachment_count).unwrap_or(0),
            attachments,
            attachment_load_error,
            size_bytes: usize::try_from(row.size_bytes).unwrap_or(0),
            eml_path: row.eml_path,
        }
    }

    pub fn save_attachment(
        &self,
        message_id: i64,
        attachment_index: usize,
        target_path: &Path,
    ) -> StoreResult<()> {
        let attachment = self.read_attachment(message_id, attachment_index)?;
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = fs::File::create(target_path)?;
        file.write_all(&attachment)?;
        file.sync_all()?;
        Ok(())
    }

    pub fn read_attachment(
        &self,
        message_id: i64,
        attachment_index: usize,
    ) -> StoreResult<Vec<u8>> {
        match self.attachment_payload(message_id, attachment_index)? {
            AttachmentPayload::Bytes(bytes) => Ok(bytes),
            AttachmentPayload::Deferred(_) => Err("附件未随同步下载，需要在线拉取".into()),
        }
    }

    pub fn attachment_payload(
        &self,
        message_id: i64,
        attachment_index: usize,
    ) -> StoreResult<AttachmentPayload> {
        let eml_path = self
            .message_detail(message_id)?
            .ok_or_else(|| format!("邮件不存在：id={message_id}"))?
            .eml_path;
        let path = self.resolve_source_path(&eml_path);
        let bytes = read_attachment_bytes(&path, attachment_index)?;
        if !bytes.is_empty() {
            return Ok(AttachmentPayload::Bytes(bytes));
        }
        let sidecar_path = path.with_extension("parts.json");
        let sidecar: AttachmentSidecar = serde_json::from_slice(&fs::read(&sidecar_path)?)?;
        let meta = read_attachment_metadata(&path).unwrap_or_default();
        let filename = meta
            .get(attachment_index)
            .map(|item| item.filename.as_str())
            .unwrap_or("");
        let entry = sidecar
            .attachments
            .iter()
            .find(|item| item.index == attachment_index || item.filename == filename)
            .or_else(|| sidecar.attachments.get(attachment_index))
            .ok_or_else(|| format!("parts.json 缺少附件 {attachment_index}"))?;
        Ok(AttachmentPayload::Deferred(DeferredAttachment {
            mailbox: sidecar.mailbox,
            uid: sidecar.uid,
            section: entry.section.clone(),
            encoding: entry.encoding.clone(),
        }))
    }

    fn resolve_source_path(&self, stored_path: &str) -> PathBuf {
        resolve_stored_path(self.source_root.as_deref(), stored_path)
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

fn read_attachment_bytes(path: &Path, attachment_index: usize) -> StoreResult<Vec<u8>> {
    let raw = fs::read(path)?;
    let message = MessageParser::default()
        .parse(&raw)
        .ok_or_else(|| format!("无法解析 RFC822/MIME 邮件：{}", path.display()))?;
    let part = message
        .attachment(attachment_index)
        .ok_or_else(|| format!("附件序号越界：index={attachment_index}"))?;

    part_payload(part).ok_or_else(|| {
        format!("附件 index={attachment_index} 是 multipart 容器，无独立载荷").into()
    })
}

fn read_attachment_metadata(path: &Path) -> StoreResult<Vec<AttachmentMeta>> {
    let raw = fs::read(path)?;
    let message = MessageParser::default()
        .parse(&raw)
        .ok_or_else(|| format!("无法解析 RFC822/MIME 邮件：{}", path.display()))?;

    Ok(message
        .attachments()
        .enumerate()
        .map(|(index, part)| attachment_meta(index, part))
        .collect())
}

/// 一次读取 + 解析 `.eml`，同时返回附件元数据与全部带 Content-ID 的载荷。
fn read_eml_assets(path: &Path) -> StoreResult<(Vec<AttachmentMeta>, HashMap<String, Vec<u8>>)> {
    let raw = fs::read(path)?;
    let message = MessageParser::default()
        .parse(&raw)
        .ok_or_else(|| format!("无法解析 RFC822/MIME 邮件：{}", path.display()))?;

    let mut metadata = Vec::new();
    let mut cid_payloads = HashMap::new();
    for (index, part) in message.attachments().enumerate() {
        let meta = attachment_meta(index, part);
        if let Some(cid) = &meta.content_id {
            if let Some(bytes) = part_payload(part) {
                cid_payloads.insert(cid.clone(), bytes);
            }
        }
        metadata.push(meta);
    }
    Ok((metadata, cid_payloads))
}

fn attachment_meta(index: usize, part: &mail_parser::MessagePart) -> AttachmentMeta {
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
        .is_some_and(|disposition| disposition.is_inline())
        || part.content_id().is_some();
    let content_id = part
        .content_id()
        .map(|value| value.trim_matches(['<', '>', ' ']).to_owned());

    AttachmentMeta {
        filename,
        content_type,
        size_bytes: part.len(),
        inline,
        content_id,
    }
}

fn part_payload(part: &mail_parser::MessagePart) -> Option<Vec<u8>> {
    match &part.body {
        mail_parser::PartType::Binary(bytes) | mail_parser::PartType::InlineBinary(bytes) => {
            Some(bytes.to_vec())
        }
        mail_parser::PartType::Text(text) | mail_parser::PartType::Html(text) => {
            Some(text.as_bytes().to_vec())
        }
        mail_parser::PartType::Message(nested) => Some(nested.raw_message.to_vec()),
        mail_parser::PartType::Multipart(_) => None,
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
        import_eml_file(
            transaction,
            mailbox_id,
            uid,
            &eml_path,
            source,
            false,
            summary,
        )?;
    }

    Ok(())
}

/// 把单个 `.eml` 落库。`source` 为 (存库路径, 字节数, 修改时间纳秒)：
/// IMAP 备份传相对路径，本地归档传绝对路径（归档目录不在索引目录下）。
/// `dedupe_by_message_id` 仅归档导入开启：导出的历史邮件常与已同步的服务器邮件重叠，
/// 同一 Message-ID 已在该邮箱里就跳过，保留服务器那份（有真实 UID）。
fn import_eml_file(
    transaction: &Transaction<'_>,
    mailbox_id: i64,
    uid: u32,
    eml_path: &Path,
    source: (String, i64, i64),
    dedupe_by_message_id: bool,
    summary: &mut ImportSummary,
) -> StoreResult<()> {
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
        return Ok(());
    }

    let raw = fs::read(eml_path)?;
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
    let html_body = html::sanitize_html(&collect_bodies(message.html_body_count(), |index| {
        message.body_html(index).map(|body| body.into_owned())
    }));
    let searchable_body = if text_body.trim().is_empty() {
        html::html_to_plain(&html_body)
    } else {
        text_body
    };
    let attachment_count = i64::try_from(message.attachment_count())?;
    let message_id = message.message_id().unwrap_or_default().trim().to_owned();

    if dedupe_by_message_id && !message_id.is_empty() {
        let duplicate = transaction
            .query_row(
                "SELECT id FROM message
                 WHERE mailbox_id = ?1 AND message_id = ?2 AND uid <> ?3
                 LIMIT 1",
                params![mailbox_id, message_id, uid],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if duplicate.is_some() {
            summary.skipped += 1;
            return Ok(());
        }
    }

    transaction.execute(
        "INSERT INTO message (
                 mailbox_id, uid, eml_path, size_bytes, modified_unix,
                 subject, sender, recipients, date_text, date_unix,
                 body, html_body, attachment_count, message_id
             ) VALUES (
                 ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14
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
                 attachment_count = excluded.attachment_count,
                 message_id = excluded.message_id",
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
            message_id,
        ],
    )?;

    if existing.is_some() {
        summary.updated += 1;
    } else {
        summary.inserted += 1;
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

/// 老索引缺 `message_id` 列时补上。只在可写连接调用；`open_read_only()` 不查该列，
/// 因此未迁移的旧索引仍能被只读打开。
fn migrate(connection: &Connection) -> StoreResult<()> {
    let has_message_id: i64 = connection.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('message') WHERE name = 'message_id'",
        [],
        |row| row.get(0),
    )?;
    if has_message_id == 0 {
        connection.execute(
            "ALTER TABLE message ADD COLUMN message_id TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS message_mailbox_message_id_idx
             ON message(mailbox_id, message_id);",
    )?;
    Ok(())
}

/// 存库路径 → 磁盘路径。IMAP 邮件存相对路径（按索引所在目录解析），归档存绝对路径。
fn resolve_stored_path(source_root: Option<&Path>, stored_path: &str) -> PathBuf {
    let path = Path::new(stored_path);
    if path.is_absolute() {
        path.to_owned()
    } else {
        source_root.map_or_else(|| path.to_owned(), |root| root.join(path))
    }
}

/// 为该邮箱里 `message_id` 为空的旧行补齐 Message-ID（读原 `.eml` 解析头）。
/// 单个文件读不出来就跳过，不让一封坏邮件挡住整次归档导入。
fn backfill_message_ids(
    transaction: &Transaction<'_>,
    mailbox_id: i64,
    source_root: Option<&Path>,
) -> StoreResult<usize> {
    let mut statement = transaction
        .prepare("SELECT id, eml_path FROM message WHERE mailbox_id = ?1 AND message_id = ''")?;
    let rows = statement
        .query_map(params![mailbox_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);

    let mut filled = 0usize;
    for (id, stored_path) in rows {
        let path = resolve_stored_path(source_root, &stored_path);
        let Ok(raw) = fs::read(&path) else { continue };
        let Some(message) = MessageParser::default().parse(&raw) else {
            continue;
        };
        let message_id = message.message_id().unwrap_or_default().trim().to_owned();
        if message_id.is_empty() {
            continue;
        }
        transaction.execute(
            "UPDATE message SET message_id = ?1 WHERE id = ?2",
            params![message_id, id],
        )?;
        filled += 1;
    }
    Ok(filled)
}

/// 归档并入哪个邮箱：该账号当前世代的收件箱（与 `mailbox_tree()` 的挑选规则一致）。
fn inbox_mailbox_id(
    transaction: &Transaction<'_>,
    account: &str,
    server: &str,
) -> StoreResult<i64> {
    transaction
        .query_row(
            "SELECT id FROM mailbox
             WHERE account = ?1 AND server = ?2 AND source_name COLLATE NOCASE = 'INBOX'
             ORDER BY scanned_at_unix DESC, uid_validity DESC, id DESC
             LIMIT 1",
            params![account, server],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| "索引里找不到收件箱，请先完成一次 IMAP 同步再导入归档".into())
}

/// 清理早期版本留下的独立归档邮箱。**先显式删 message 再删 mailbox**：直接 DELETE 一定触发
/// `message_ad`，保证 FTS5 external content 同步，不依赖外键 cascade 是否触发 trigger。
fn remove_archive_mailboxes(
    transaction: &Transaction<'_>,
    account: &str,
    server: &str,
) -> StoreResult<usize> {
    let mut statement = transaction.prepare(
        "SELECT id FROM mailbox WHERE account = ?1 AND server = ?2 AND uid_validity = ?3",
    )?;
    let ids = statement
        .query_map(params![account, server, ARCHIVE_UID_VALIDITY], |row| {
            row.get::<_, i64>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for id in &ids {
        transaction.execute("DELETE FROM message WHERE mailbox_id = ?1", params![id])?;
        transaction.execute("DELETE FROM mailbox WHERE id = ?1", params![id])?;
    }
    Ok(ids.len())
}

fn list_eml_files_recursive(root: &Path) -> StoreResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    visit_directories(root, &mut |directory| {
        files.extend(list_eml_files(directory)?);
        Ok(())
    })?;
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
    let relative = path.strip_prefix(data_root)?;
    // 相对路径统一成 `/`，跨平台可比。
    fingerprint(relative.to_string_lossy().replace('\\', "/"), path)
}

/// 归档存绝对路径，**不做分隔符替换**：Windows 绝对路径换成 `/` 后无法再打开。
fn absolute_fingerprint(path: &Path) -> StoreResult<(String, i64, i64)> {
    fingerprint(path.to_string_lossy().into_owned(), path)
}

fn fingerprint(stored_path: String, path: &Path) -> StoreResult<(String, i64, i64)> {
    let metadata = fs::metadata(path)?;
    let size = i64::try_from(metadata.len())?;
    let modified = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let modified_nanos = i64::try_from(modified.as_nanos())?;
    Ok((stored_path, size, modified_nanos))
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

/// Escape LIKE wildcards (`%` `_` `\`) so user input matches literally.
fn escape_like_pattern(query: &str) -> String {
    let mut out = String::with_capacity(query.len() + 4);
    for character in query.chars() {
        match character {
            '%' | '_' | '\\' => {
                out.push('\\');
                out.push(character);
            }
            _ => out.push(character),
        }
    }
    out
}

/// 查询是否携带真正的匹配条件（FTS 术语 / LIKE 术语 / 任一操作符），
/// 用于区分空查询与操作符限定查询。
fn has_positive_selector(q: &ParsedQuery) -> bool {
    !q.fts.is_empty()
        || !q.like_terms.is_empty()
        || q.from.is_some()
        || q.to.is_some()
        || q.in_role.is_some()
        || q.has_attachment
}

/// 由邮箱名推断角色。source_name 是 IMAP UTF-7 编码，display_name 是
/// 解码后的名称（INBOX 保留原文，其余如 已删除/垃圾箱/已发送/草稿箱）。
fn mailbox_role_sql(alias: &str) -> String {
    format!(
        "CASE
            WHEN {a}.source_name COLLATE NOCASE = 'INBOX' THEN 'inbox'
            WHEN {a}.display_name LIKE '%已删除%'
                OR {a}.display_name LIKE '%Deleted%'
                OR {a}.display_name LIKE '%Trash%' THEN 'trash'
            WHEN {a}.display_name LIKE '%垃圾%'
                OR {a}.display_name LIKE '%Junk%'
                OR {a}.display_name LIKE '%Spam%' THEN 'spam'
            WHEN {a}.display_name LIKE '%已发送%'
                OR {a}.display_name LIKE '%Sent%' THEN 'sent'
            WHEN {a}.display_name LIKE '%草稿%'
                OR {a}.display_name LIKE '%Draft%' THEN 'drafts'
            ELSE 'other'
         END",
        a = alias
    )
}

fn id_in_list(ids: impl Iterator<Item = i64>) -> String {
    ids.map(|id| id.to_string()).collect::<Vec<_>>().join(",")
}

/// 发件人匹配度 [0,1]：词首/地址本地部词首命中记 1.0，纯子串 0.4。
fn sender_match(terms: &[String], sender: &str) -> f32 {
    if terms.is_empty() {
        return 0.0;
    }
    let sender = sender.to_lowercase();
    let local = sender.split('@').next().unwrap_or("");
    let mut total = 0.0;
    for term in terms {
        total += if sender
            .split_whitespace()
            .chain(local.split(['.', '-', '_']))
            .any(|word| word.starts_with(term.as_str()))
        {
            1.0
        } else if sender.contains(term.as_str()) {
            0.4
        } else {
            0.0
        };
    }
    total / terms.len() as f32
}

/// Sanitize a MIME-supplied filename into a safe single file component.
///
/// Strips path separators/drive prefixes (traversal), Windows-reserved names,
/// and Windows-invalid/control characters; falls back to `attachment.bin`.
pub fn sanitized_attachment_filename(filename: &str) -> String {
    let name = filename
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim()
        .chars()
        .filter(|character| {
            !character.is_control() && !matches!(*character, ':' | '"' | '<' | '>' | '|' | '?' | '*')
        })
        .collect::<String>()
        .trim()
        .to_owned();
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = name.split('.').next().unwrap_or_default();
    if name.is_empty()
        || name.starts_with('.')
        || name.ends_with('.')
        || RESERVED.contains(&stem.to_ascii_uppercase().as_str())
    {
        return "attachment.bin".to_owned();
    }
    name
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentPreviewKind {
    Image,
    Pdf,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SniffedImageFormat {
    Png,
    Jpeg,
    Gif,
    Webp,
    Bmp,
    Tiff,
    Ico,
}

pub fn attachment_preview_kind(content_type: &str, filename: &str) -> AttachmentPreviewKind {
    if resolve_image_format(content_type, filename, None).is_some() {
        return AttachmentPreviewKind::Image;
    }
    let mime = normalized_mime(content_type);
    let extension = attachment_extension(filename).to_ascii_lowercase();
    if mime == "application/pdf" || extension == "pdf" {
        AttachmentPreviewKind::Pdf
    } else {
        AttachmentPreviewKind::Unsupported
    }
}

pub fn resolve_image_format(
    content_type: &str,
    filename: &str,
    bytes: Option<&[u8]>,
) -> Option<SniffedImageFormat> {
    bytes
        .and_then(sniff_image_format)
        .or_else(|| image_format_from_mime(content_type))
        .or_else(|| image_format_from_extension(filename))
}

fn normalized_mime(content_type: &str) -> &str {
    content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
}

fn attachment_extension(filename: &str) -> &str {
    filename
        .rsplit(['/', '\\'])
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, ext)| ext)
        .unwrap_or("")
}

fn image_format_from_mime(content_type: &str) -> Option<SniffedImageFormat> {
    match normalized_mime(content_type).to_ascii_lowercase().as_str() {
        "image/png" => Some(SniffedImageFormat::Png),
        "image/jpeg" | "image/jpg" => Some(SniffedImageFormat::Jpeg),
        "image/gif" => Some(SniffedImageFormat::Gif),
        "image/webp" => Some(SniffedImageFormat::Webp),
        "image/bmp" | "image/x-ms-bmp" => Some(SniffedImageFormat::Bmp),
        "image/tiff" | "image/tif" => Some(SniffedImageFormat::Tiff),
        "image/ico" | "image/x-icon" | "image/vnd.microsoft.icon" => Some(SniffedImageFormat::Ico),
        _ => None,
    }
}

fn image_format_from_extension(filename: &str) -> Option<SniffedImageFormat> {
    match attachment_extension(filename).to_ascii_lowercase().as_str() {
        "png" => Some(SniffedImageFormat::Png),
        "jpg" | "jpeg" => Some(SniffedImageFormat::Jpeg),
        "gif" => Some(SniffedImageFormat::Gif),
        "webp" => Some(SniffedImageFormat::Webp),
        "bmp" => Some(SniffedImageFormat::Bmp),
        "tif" | "tiff" => Some(SniffedImageFormat::Tiff),
        "ico" => Some(SniffedImageFormat::Ico),
        _ => None,
    }
}

fn sniff_image_format(bytes: &[u8]) -> Option<SniffedImageFormat> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some(SniffedImageFormat::Png)
    } else if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        Some(SniffedImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(SniffedImageFormat::Gif)
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some(SniffedImageFormat::Webp)
    } else if bytes.starts_with(b"BM") {
        Some(SniffedImageFormat::Bmp)
    } else if bytes.starts_with(&[0x49, 0x49, 0x2A, 0x00])
        || bytes.starts_with(&[0x4D, 0x4D, 0x00, 0x2A])
    {
        Some(SniffedImageFormat::Tiff)
    } else if bytes.starts_with(&[0x00, 0x00, 0x01, 0x00]) {
        Some(SniffedImageFormat::Ico)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{
        attachment_preview_kind, read_attachment_metadata, resolve_image_format,
        sanitized_attachment_filename, sniff_image_format, AttachmentPreviewKind, MailStore,
        SearchHit, SniffedImageFormat,
    };
    use rusqlite::params;
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

        // <3 字走 LIKE 兜底：trigram 切不出 trigram，LIKE 必须命中
        let short = store.search("中文", 10).unwrap();
        assert_eq!(short.len(), 1);
        assert_eq!(short[0].uid, 7);
        assert!(short[0].snippet.is_empty()); // LIKE 路径无 snippet
        assert!(store.search("不存", 10).unwrap().is_empty());

        let second = store.import_backup(&root).unwrap();
        assert_eq!(second.inserted, 0);
        assert_eq!(second.updated, 0);
        assert_eq!(second.skipped, 1);

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn escapes_like_wildcards_in_short_queries() {
        let root = temporary_directory("like-escape");
        write_mailbox(
            &root,
            "alpha@example.com",
            "INBOX",
            42,
            100,
            &[(7, "达标 0% 报告"), (8, "其余无关邮件")],
        );
        let database = root.join("mail.sqlite");
        let mut store = MailStore::open(&database).unwrap();
        store.import_backup(&root).unwrap();

        // `0%` 是 2 字走 LIKE 路径：`%` 必须转义为字面量，否则当通配符会误命中所有含 0 的邮件
        // 转义后只匹配字面 "0%"，不匹配 "其余无关邮件"
        let hits = store.search("0%", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].subject.contains("0%"));
        assert!(hits.iter().all(|h| h.subject.contains("0%")));

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn search_operators_relaxation_and_exclusion() {
        let root = temporary_directory("search-operators");
        write_mailbox(
            &root,
            "alpha@example.com",
            "INBOX",
            42,
            100,
            &[
                (7, "q3budget kickoff meeting"),
                (8, "q3budget final report"),
                (9, "random chatter here"),
            ],
        );
        write_mailbox(
            &root,
            "alpha@example.com",
            "&V4NXPnux-",
            42,
            100,
            &[(10, "q3budget spam newsletter")],
        );
        let database = root.join("mail.sqlite");
        let mut store = MailStore::open(&database).unwrap();
        store.import_backup(&root).unwrap();

        let uids = |hits: Vec<SearchHit>| -> Vec<u32> {
            hits.into_iter().map(|hit| hit.uid).collect()
        };

        // AND：两术语同时命中的才保留。
        let hits = store.search("q3budget meeting", 10).unwrap();
        assert_eq!(uids(hits), vec![7]);

        // 默认排除垃圾箱（&V4NXPnux- → 垃圾箱）。
        let hits = store.search("q3budget", 10).unwrap();
        let mut got = uids(hits);
        got.sort();
        assert_eq!(got, vec![7, 8]);

        // AND 无结果 → 放宽为 OR；垃圾箱仍被默认排除。
        let hits = store.search("q3budget meeting newsletter", 10).unwrap();
        let mut got = uids(hits);
        got.sort();
        assert_eq!(got, vec![7, 8]);

        // in:spam 显式纳入垃圾箱（&V4NXPnux- → 垃圾箱 → 角色 spam）。
        let hits = store.search("in:spam q3budget", 10).unwrap();
        assert_eq!(uids(hits), vec![10]);

        // in:trash 匹配不到：夹具没有"已删除"邮箱。
        assert!(store.search("in:trash q3budget", 10).unwrap().is_empty());

        // -term 排除：meeting 被排除后只剩 report。
        let hits = store.search("q3budget -meeting", 10).unwrap();
        assert_eq!(uids(hits), vec![8]);

        // from: 操作符。
        let hits = store.search("from:sender q3budget", 10).unwrap();
        let mut got = uids(hits);
        got.sort();
        assert_eq!(got, vec![7, 8]);

        // from: 不匹配的发件人。
        assert!(store.search("from:nobody q3budget", 10).unwrap().is_empty());

        // has:attachment：夹具没有附件。
        assert!(store.search("has:attachment q3budget", 10).unwrap().is_empty());

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

    #[test]
    fn saves_attachment_payload_to_target_path() {
        let root = temporary_directory("attachment-save");
        let mailbox = root
            .join(fixture_component("user@example.com"))
            .join(fixture_component("INBOX"))
            .join("uidvalidity-42");
        fs::create_dir_all(&mailbox).unwrap();
        fs::write(
            mailbox.join("mailbox.meta.json"),
            serde_json::to_vec_pretty(&json!({
                "account": "user@example.com",
                "server": "imap.example.com",
                "mailbox": "INBOX",
                "uid_validity": 42,
                "scanned_at_unix": 100,
                "server_message_count": 1,
                "discovered_uid_count": 1
            }))
            .unwrap(),
        )
        .unwrap();
        // base64 "AAE=" decodes to bytes [0x00, 0x01]
        fs::write(
            mailbox.join("0000000001.eml"),
            "From: sender@example.com\r\n\
To: receiver@example.com\r\n\
Date: Mon, 17 Aug 2026 09:00:00 +0800\r\n\
Subject: Attachment save\r\n\
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

        let database = root.join("mail.sqlite");
        let mut store = MailStore::open(&database).unwrap();
        store.import_backup(&root).unwrap();
        let mailbox_id = store.mailbox_tree().unwrap()[0].mailboxes[0].id;
        let message_id = store.message_page(mailbox_id, 0, 10).unwrap()[0].id;

        let target = root.join("out").join("saved.bin");
        store.save_attachment(message_id, 0, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), vec![0x00, 0x01]);

        drop(store);
        // read-only path also saves attachments
        let store = MailStore::open_read_only(&database).unwrap();
        let target2 = root.join("out").join("saved2.bin");
        store.save_attachment(message_id, 0, &target2).unwrap();
        assert_eq!(fs::read(&target2).unwrap(), vec![0x00, 0x01]);

        // out-of-range index must fail without creating the file
        let missing = root.join("out").join("missing.bin");
        assert!(store.save_attachment(message_id, 5, &missing).is_err());
        assert!(!missing.exists());

        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn attachment_names_strip_all_windows_invalid_characters() {
        for invalid in ['<', '>', ':', '"', '|', '?', '*', '\0', '\n'] {
            let input = format!("报告{invalid}最终版.txt");
            assert_eq!(sanitized_attachment_filename(&input), "报告最终版.txt");
        }
        assert_eq!(sanitized_attachment_filename("../../unsafe?.bin"), "unsafe.bin");
        assert_eq!(sanitized_attachment_filename("<>|?*"), "attachment.bin");
    }

    #[test]
    fn invalid_character_removal_cannot_disguise_device_names() {
        for input in ["C?ON.txt", "LPT*1.log", "A<U>X", "N|UL.bin"] {
            assert_eq!(sanitized_attachment_filename(input), "attachment.bin");
        }
        assert_eq!(sanitized_attachment_filename("COM10.txt"), "COM10.txt");
        assert_eq!(sanitized_attachment_filename("正常报告🙂.txt"), "正常报告🙂.txt");
    }

    #[test]
    fn sanitizes_mime_filenames_against_traversal_and_reserved_names() {
        assert_eq!(sanitized_attachment_filename("../../evil.exe"), "evil.exe");
        assert_eq!(sanitized_attachment_filename("C:\\temp\\x.pdf"), "x.pdf");
        assert_eq!(sanitized_attachment_filename("a/b\\c.txt"), "c.txt");
        assert_eq!(sanitized_attachment_filename("CON"), "attachment.bin");
        assert_eq!(sanitized_attachment_filename("com1.zip"), "attachment.bin");
        assert_eq!(sanitized_attachment_filename(".."), "attachment.bin");
        assert_eq!(sanitized_attachment_filename(""), "attachment.bin");
        assert_eq!(sanitized_attachment_filename(".hidden"), "attachment.bin");
        assert_eq!(
            sanitized_attachment_filename("报告\x08最终版.docx"),
            "报告最终版.docx"
        );
    }

    #[test]
    fn classifies_attachment_preview_kind_from_mime_and_filename() {
        assert_eq!(
            attachment_preview_kind("image/png", "photo.png"),
            AttachmentPreviewKind::Image
        );
        assert_eq!(
            attachment_preview_kind("application/octet-stream", "scan.JPG"),
            AttachmentPreviewKind::Image
        );
        assert_eq!(
            attachment_preview_kind("application/pdf", "合同.pdf"),
            AttachmentPreviewKind::Pdf
        );
        assert_eq!(
            attachment_preview_kind("application/octet-stream", "notes.pdf"),
            AttachmentPreviewKind::Pdf
        );
        assert_eq!(
            attachment_preview_kind("application/msword", "a.doc"),
            AttachmentPreviewKind::Unsupported
        );
        assert_eq!(
            sniff_image_format(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00]),
            Some(SniffedImageFormat::Png)
        );
        assert_eq!(
            sniff_image_format(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some(SniffedImageFormat::Jpeg)
        );
        assert_eq!(sniff_image_format(b"%PDF-1.4"), None);
        assert_eq!(
            resolve_image_format(
                "application/octet-stream",
                "x.bin",
                Some(&[0xFF, 0xD8, 0xFF])
            ),
            Some(SniffedImageFormat::Jpeg)
        );
    }

    #[test]
    fn imports_loose_eml_archive_into_inbox() {
        let root = temporary_directory("archive-backup");
        write_mailbox(
            &root,
            "user@example.com",
            "INBOX",
            42,
            1_700_000_000,
            &[(1, "服务器邮件")],
        );
        let database = temporary_directory("archive-db").join("mail-index.sqlite");
        let mut store = MailStore::open(&database).unwrap();
        store.import_backup(&root).unwrap();

        // 模拟早期版本留下的独立「本地归档」邮箱（uid_validity 哨兵 0），导入时必须被清掉。
        store
            .connection
            .execute(
                "INSERT INTO mailbox (
                     account, server, source_name, display_name, uid_validity,
                     scanned_at_unix, server_message_count, discovered_uid_count
                 ) VALUES ('user@example.com', 'imap.example.com', '本地归档', '本地归档', 0, 1, 1, 1)",
                [],
            )
            .unwrap();
        let legacy_id = store.connection.last_insert_rowid();
        store
            .connection
            .execute(
                "INSERT INTO message (
                     mailbox_id, uid, eml_path, size_bytes, modified_unix,
                     subject, sender, recipients, date_text, date_unix,
                     body, html_body, attachment_count
                 ) VALUES (?1, 1, 'legacy.eml', 1, 1, '遗留归档', '', '', NULL, NULL, '', '', 0)",
                params![legacy_id],
            )
            .unwrap();

        // 散装归档：文件名是主题而非 UID，且目录不在索引目录下。
        // 第三封与已同步的服务器邮件同 Message-ID，必须按重复跳过。
        let archive = temporary_directory("archive-loose");
        fs::create_dir_all(archive.join("子目录")).unwrap();
        write_loose_eml(
            &archive.join("工程变更跟进表 1.15.eml"),
            "工程变更跟进表",
            "archive-1@example.com",
        );
        write_loose_eml(
            &archive.join("子目录").join("Case ST0001.eml"),
            "Faulty relay",
            "archive-2@example.com",
        );
        write_loose_eml(
            &archive.join("服务器上已有的同一封.eml"),
            "服务器邮件",
            "server-1@example.com",
        );

        let first = store.import_archive(&archive).unwrap();
        assert_eq!(first.discovered_messages, 3);
        assert_eq!(first.inserted, 2);
        assert_eq!(first.skipped, 1, "与服务器邮件同 Message-ID 的那封必须跳过");

        let accounts = store.mailbox_tree().unwrap();
        assert_eq!(accounts.len(), 1, "归档必须挂在既有账号下，不新建账号");
        assert!(
            accounts[0]
                .mailboxes
                .iter()
                .all(|mailbox| mailbox.display_name != "本地归档"),
            "遗留的独立归档邮箱必须被清理"
        );
        let inbox = accounts[0]
            .mailboxes
            .iter()
            .find(|mailbox| mailbox.source_name.eq_ignore_ascii_case("INBOX"))
            .expect("邮箱树里应有收件箱");
        assert_eq!(inbox.message_count, 3, "1 封服务器邮件 + 2 封归档");
        assert_eq!(store.message_count().unwrap(), 3, "遗留归档邮件已删除");

        // 归档必须存绝对路径，否则附件读取会按索引目录解析而找不到文件；
        // UID 落在归档基址之上，不与真实 IMAP UID 相撞。
        let page = store.message_page(inbox.id, 0, 10).unwrap();
        assert_eq!(page.len(), 3);
        let archived = page
            .iter()
            .filter(|summary| summary.uid > super::ARCHIVE_UID_BASE)
            .count();
        assert_eq!(archived, 2);
        for summary in page.iter().filter(|s| s.uid > super::ARCHIVE_UID_BASE) {
            let detail = store.message_detail(summary.id).unwrap().unwrap();
            assert!(
                Path::new(&detail.eml_path).is_absolute(),
                "归档 eml_path 必须是绝对路径：{}",
                detail.eml_path
            );
            assert!(Path::new(&detail.eml_path).is_file());
        }

        let hits = store.search("工程变更", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].mailbox.eq_ignore_ascii_case("INBOX") || !hits[0].mailbox.is_empty());

        // 同一批文件重跑：归档行整批重建，结果稳定（inserted 恒等于去重后的数量）。
        let second = store.import_archive(&archive).unwrap();
        assert_eq!(second.inserted, 2);
        assert_eq!(second.updated, 0);
        assert_eq!(second.skipped, 1);
        assert_eq!(store.message_count().unwrap(), 3);
        assert_eq!(store.integrity_check().unwrap().fts5_external_content, "ok");
    }

    fn write_loose_eml(path: &Path, subject: &str, message_id: &str) {
        fs::write(
            path,
            format!(
                "From: Sender <sender@example.com>\r\n\
To: receiver@example.com\r\n\
Date: Mon, 17 Aug 2026 09:00:00 +0800\r\n\
Subject: {subject}\r\n\
Message-ID: <{message_id}>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
Content-Transfer-Encoding: 8bit\r\n\
\r\n\
归档正文。\r\n"
            ),
        )
        .unwrap();
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
Message-ID: <server-{uid}@example.com>\r\n\
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
