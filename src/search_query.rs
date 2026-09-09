//! Gmail 风格搜索查询解析：`from:` / `to:` / `in:` / `has:` /
//! `subject:` / `body:` / `exclude:` 操作符，其余词作为 FTS5 术语。
//! `-term` / `exclude:term` 从结果中排除匹配的邮件。
//! 参考 Flectar Mail 的解析器，适配本项目 trigram 分词器
//! （子串语义，无 `*` 前缀；<3 字术语 trigram 索引不到，直接丢弃）。

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ParsedQuery {
    /// 全部术语 AND 连接的 FTS5 MATCH 串（trigram 短语写法）。
    pub fts: String,
    /// 同样术语 OR 连接——AND 无结果时的放宽回退。
    pub fts_or: String,
    /// 排除术语（`exclude:foo` / `-foo`）OR 连接的 MATCH 串。
    pub fts_not: String,
    /// trigram 索引不到的 <3 字术语（如两字中文词），作为 LIKE 子串条件。
    pub like_terms: Vec<String>,
    /// 自由文本术语（小写折叠），供发件人匹配加分。
    pub terms: Vec<String>,
    /// 发件人过滤（姓名/地址，大小写不敏感子串）。
    pub from: Option<String>,
    /// 收件人过滤。
    pub to: Option<String>,
    /// 限定邮箱角色：inbox / sent / drafts / trash / spam。
    pub in_role: Option<String>,
    /// 有附件过滤。
    pub has_attachment: bool,
}

/// 解析用户输入。操作符大小写不敏感；`12:30` 这类非操作符
/// 冒号词按普通文本处理；未闭合引号仍按短语处理。
pub fn parse(input: &str) -> ParsedQuery {
    let mut q = ParsedQuery::default();
    let mut fts_terms: Vec<String> = Vec::new();
    let mut not_terms: Vec<String> = Vec::new();

    for token in tokenize(input) {
        match token {
            Token::Phrase(col, phrase) => {
                let words: Vec<String> = phrase
                    .split_whitespace()
                    .map(clean_token)
                    .filter(|w| !w.is_empty())
                    .collect();
                if !words.is_empty() {
                    let phrase = words.join(" ");
                    match col {
                        Some(_) => fts_terms.push(fts_term(col.as_deref(), &phrase)),
                        None => {
                            push_term(&mut fts_terms, &mut q.like_terms, None, &phrase);
                            q.terms.push(phrase.to_lowercase());
                        }
                    }
                }
            }
            Token::Word(word) => {
                // Gmail 风格否定：`-draft` 排除，裸 `-` 丢弃。
                if let Some(rest) = word.strip_prefix('-') {
                    let clean = clean_token(rest.trim_start_matches('-'));
                    if !clean.is_empty() {
                        not_terms.push(fts_term(None, &clean));
                    }
                    continue;
                }
                let (key, value) = match word.split_once(':') {
                    Some((k, v)) => (k.to_ascii_lowercase(), v),
                    None => (String::new(), ""),
                };
                // 容忍操作符值后面的列表标点：`from:a@b.com, report`。
                let value = value.trim_matches([',', ';']);
                match key.as_str() {
                    "from" if !value.is_empty() => q.from = Some(value.to_string()),
                    "to" if !value.is_empty() => q.to = Some(value.to_string()),
                    "in" => {
                        q.in_role = match value.to_ascii_lowercase().as_str() {
                            "inbox" | "收件箱" => Some("inbox".into()),
                            "sent" | "已发送" => Some("sent".into()),
                            "drafts" | "草稿" | "草稿箱" => Some("drafts".into()),
                            "trash" | "已删除" | "删除" => Some("trash".into()),
                            "spam" | "垃圾" | "垃圾箱" | "垃圾邮件" => Some("spam".into()),
                            _ => None,
                        };
                    }
                    "has" if value.eq_ignore_ascii_case("attachment") => {
                        q.has_attachment = true;
                    }
                    // 列限定：`subject:foo` / `body:foo`，短语写法留给 FTS。
                    "subject" | "body" => {
                        let clean = clean_token(value);
                        if !clean.is_empty() {
                            fts_terms.push(fts_term(Some(key.as_str()), &clean));
                        }
                    }
                    "exclude" => {
                        let clean = clean_token(value);
                        if !clean.is_empty() {
                            not_terms.push(fts_term(None, &clean));
                        }
                    }
                    // 非操作符（含 "12:30" 这类冒号词）按普通文本。
                    _ => {
                        let clean = clean_token(&word);
                        if !clean.is_empty() {
                            push_term(&mut fts_terms, &mut q.like_terms, None, &clean);
                            q.terms.push(clean.to_lowercase());
                        }
                    }
                }
            }
        }
    }

    q.fts = fts_terms.join(" AND ");
    q.fts_or = fts_terms.join(" OR ");
    q.fts_not = not_terms.join(" OR ");
    q
}

enum Token {
    Word(String),
    Phrase(Option<String>, String),
}

/// `subject:` / `body:` 紧跟引号短语时，把列限定带给短语。
fn scoped_col(token: &str) -> Option<&'static str> {
    match token.strip_suffix(':')?.to_ascii_lowercase().as_str() {
        "subject" => Some("subject"),
        "body" => Some("body"),
        _ => None,
    }
}

fn tokenize(input: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut pending_col: Option<String> = None;
    for c in input.chars() {
        match c {
            '"' => {
                if in_quote {
                    if !cur.is_empty() {
                        out.push(Token::Phrase(pending_col.take(), std::mem::take(&mut cur)));
                    } else {
                        pending_col = None;
                    }
                    in_quote = false;
                } else {
                    if !cur.is_empty() {
                        if let Some(col) = scoped_col(&cur) {
                            pending_col = Some(col.to_string());
                            cur.clear();
                        } else {
                            out.push(Token::Word(std::mem::take(&mut cur)));
                        }
                    }
                    in_quote = true;
                }
            }
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() {
                    out.push(Token::Word(std::mem::take(&mut cur)));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(if in_quote {
            Token::Phrase(pending_col.take(), cur)
        } else {
            Token::Word(cur)
        });
    }
    out
}

/// 去掉 FTS 语法特殊字符，只保留文字类字符。
fn clean_token(token: &str) -> String {
    token
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '@' || *c == '.' || *c == '-' || *c == '_')
        .collect()
}

/// trigram 分词器的 FTS5 术语：短语即子串匹配，无 `*` 前缀。
/// 列限定形如 `subject:"foo"`。
fn fts_term(col: Option<&str>, clean: &str) -> String {
    match col {
        Some(c) => format!("{c}:\"{clean}\""),
        None => format!("\"{clean}\""),
    }
}

/// ≥3 字术语进 FTS（trigram 可索引）；<3 字术语（如"报表"）
/// 降级为 LIKE 子串条件。列限定术语必须走 FTS。
fn push_term(
    fts_terms: &mut Vec<String>,
    like_terms: &mut Vec<String>,
    col: Option<&str>,
    clean: &str,
) {
    if col.is_none() && clean.chars().count() < 3 {
        like_terms.push(clean.to_string());
    } else {
        fts_terms.push(fts_term(col, clean));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_operators() {
        let q = parse("from:alice has:attachment quarterly report");
        assert_eq!(q.from.as_deref(), Some("alice"));
        assert!(q.has_attachment);
        assert_eq!(q.fts, "\"quarterly\" AND \"report\"");
    }

    #[test]
    fn operators_are_case_insensitive() {
        let q = parse("IN:INBOX HAS:Attachment");
        assert_eq!(q.in_role.as_deref(), Some("inbox"));
        assert!(q.has_attachment);
        assert!(q.fts.is_empty());
    }

    #[test]
    fn chinese_folder_aliases() {
        assert_eq!(parse("in:已发送").in_role.as_deref(), Some("sent"));
        assert_eq!(parse("in:垃圾箱").in_role.as_deref(), Some("spam"));
        assert_eq!(parse("in:草稿箱").in_role.as_deref(), Some("drafts"));
    }

    #[test]
    fn subject_and_body_scope_to_their_column() {
        // 列限定术语始终走 FTS（即使 2 字）；自由文本 2 字降级 LIKE。
        let q = parse("subject:发票 body:逾期 报告");
        assert_eq!(q.fts, "subject:\"发票\" AND body:\"逾期\"");
        assert_eq!(q.like_terms, vec!["报告"]);
        let q = parse("subject:\"季度报告\" 预算");
        assert_eq!(q.fts, "subject:\"季度报告\"");
        assert_eq!(q.like_terms, vec!["预算"]);
    }

    #[test]
    fn dash_prefix_is_exclusion() {
        let q = parse("report -draft -old");
        assert_eq!(q.fts, "\"report\"");
        assert_eq!(q.fts_not, "\"draft\" OR \"old\"");
    }

    #[test]
    fn exclude_operator_collects_negated_terms() {
        let q = parse("report exclude:draft");
        assert_eq!(q.fts, "\"report\"");
        assert_eq!(q.fts_not, "\"draft\"");
    }

    #[test]
    fn bare_exclude_and_dash_are_noops() {
        let q = parse("hello exclude: -");
        assert_eq!(q.fts, "\"hello\"");
        assert!(q.fts_not.is_empty());
    }

    #[test]
    fn non_operator_colon_token_is_free_text() {
        // trigram 需 ≥3 字；"1230" 是 4 字，保留。
        let q = parse("12:30");
        assert_eq!(q.fts, "\"1230\"");
    }

    #[test]
    fn short_terms_are_dropped() {
        // <3 字术语 trigram 索引不到，从 FTS 串中丢弃。
        let q = parse("ab 超长术语");
        assert_eq!(q.fts, "\"超长术语\"");
        assert_eq!(q.like_terms, vec!["ab"]);
    }

    #[test]
    fn collects_terms_for_sender_boost() {
        let q = parse("alice Report 发票");
        assert_eq!(q.terms, vec!["alice", "report", "发票"]);
        assert_eq!(q.like_terms, vec!["发票"]);
    }

    #[test]
    fn operator_value_tolerates_trailing_comma() {
        let q = parse("from:a@b.com, report");
        assert_eq!(q.from.as_deref(), Some("a@b.com"));
        assert_eq!(q.fts, "\"report\"");
    }

    #[test]
    fn unterminated_quote_is_still_a_phrase() {
        let q = parse("\"hello world");
        assert_eq!(q.fts, "\"hello world\"");
    }
}
