//! Safe, bounded input for the component TextView. Never pass mail HTML or
//! image/link URIs to its resource loader. MIME/CID resolution stays in MailStore.
use foxmail_imap::html::{self, BodyBlock, InlineSpan, TableCell};

pub(crate) const TEXT_CHUNK_BYTES: usize = 2048;
pub(crate) const TEXT_CHUNK_LINES: usize = 32;
pub(crate) const TABLE_COLUMNS_PER_BLOCK: usize = 16;
const MERGED_HTML_MAX_BYTES: usize = 512 * 1024;

// GPUI 0.3.4 has no white-space: pre-wrap and no inline-style parser for
// block elements beyond the themed StyleRefinement fields on TextViewStyle,
// so these inline `style="..."` attributes are best-effort: future TextView
// upgrades will pick them up, and right now they document the intent without
// confusing any current renderer. paragraph_gap / line_height / padding are
// driven from app.rs render_body_block to get the visible effect today.
const PARA_STYLE: &str = r#" style="margin:0 0 0.4em 0;line-height:1.65""#;
const HEADING_STYLE: &str = r#" style="margin:0.6em 0 0.3em 0;line-height:1.3;font-weight:600""#;
const BLOCKQUOTE_STYLE: &str = r#" style="margin:0.4em 0;padding:0.2em 0 0.2em 0.8em;border-left:3px solid currentColor;opacity:0.85""#;
const PRE_STYLE: &str = r#" style="margin:0.4em 0;padding:0.6em 0.8em;background:rgba(127,127,127,0.08);border-radius:4px;line-height:1.45""#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MailBodyBlock {
    Text { html: String, monospace: bool },
    Image { cid: String, alt: String },
    // The locked TextView table parser drops empty cells and ignores th/span
    // semantics. Keep the already expanded mail grid, rendering its text with
    // TextView. One bounded continuation row is one outer virtual-list item.
    TableRow { cells: Vec<MailBodyCell> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MailBodyCell {
    pub header: bool,
    pub content: Option<MailBodyBlock>,
}

#[derive(Clone, Copy)]
enum Role {
    Paragraph,
    Literal,
    Code,
    Heading(u8),
    Quote,
    List(Option<usize>),
}

pub(crate) fn normalize_plain_body(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_end()
        .to_owned()
}

pub(crate) fn prepare_blocks(html_body: &str, plain_body: &str) -> Vec<MailBodyBlock> {
    let trimmed_html = html_body.trim();
    let source = if trimmed_html.is_empty() {
        Vec::new()
    } else {
        html::layout_html(trimmed_html)
    };
    if source.is_empty() {
        let plain = normalize_plain_body(plain_body);
        return text_parts(
            &plain,
            Role::Literal,
            TEXT_CHUNK_BYTES,
            TEXT_CHUNK_LINES,
            false,
        );
    }
    let mut out = Vec::new();
    let mut ordinal = 0;
    for block in source {
        if !matches!(block, BodyBlock::ListItem { ordered: true, .. }) {
            ordinal = 0;
        }
        let (spans, role) = match block {
            BodyBlock::Heading { level, spans } => (spans, Role::Heading(level.clamp(1, 6))),
            BodyBlock::Paragraph { spans } => (spans, Role::Paragraph),
            BodyBlock::Quote { spans } => (spans, Role::Quote),
            BodyBlock::ListItem { ordered, spans } => {
                ordinal += usize::from(ordered);
                (spans, Role::List(ordered.then_some(ordinal)))
            }
            BodyBlock::Pre { text } => {
                out.extend(text_parts(
                    &text,
                    Role::Code,
                    TEXT_CHUNK_BYTES,
                    TEXT_CHUNK_LINES,
                    false,
                ));
                continue;
            }
            BodyBlock::Table { rows } => {
                prepare_table(rows, &mut out);
                continue;
            }
        };
        out.extend(inline_parts(
            spans,
            role,
            TEXT_CHUNK_BYTES,
            TEXT_CHUNK_LINES,
        ));
    }
    if should_merge_html(trimmed_html) {
        merge_adjacent_text_blocks(out)
    } else {
        out
    }
}

/// Keep the parser iterative for deeply nested Foxmail threads, then let one
/// TextView lay out adjacent safe paragraphs together. This preserves paragraph
/// gaps without feeding dozens of nested div/blockquote levels into GPUI's
/// recursive HTML renderer.
fn should_merge_html(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MERGED_HTML_MAX_BYTES
        && value.to_ascii_lowercase().contains("<div")
}

fn merge_adjacent_text_blocks(blocks: Vec<MailBodyBlock>) -> Vec<MailBodyBlock> {
    let mut merged = Vec::with_capacity(blocks.len());
    for block in blocks {
        match (merged.last_mut(), block) {
            (
                Some(MailBodyBlock::Text {
                    html: previous,
                    monospace: false,
                }),
                MailBodyBlock::Text {
                    html,
                    monospace: false,
                },
            ) => previous.push_str(&html),
            (_, block) => merged.push(block),
        }
    }
    merged
}

fn inline_parts(
    spans: Vec<InlineSpan>,
    role: Role,
    bytes: usize,
    lines: usize,
) -> Vec<MailBodyBlock> {
    let mut out = Vec::new();
    let mut text = String::new();
    for span in spans {
        match span {
            InlineSpan::Text(value) => text.push_str(&value),
            InlineSpan::Break => text.push('\n'),
            InlineSpan::Image { cid, alt } => {
                out.extend(text_parts(&text, role, bytes, lines, !out.is_empty()));
                text.clear();
                out.push(MailBodyBlock::Image { cid, alt });
            }
        }
    }
    out.extend(text_parts(&text, role, bytes, lines, !out.is_empty()));
    out
}

// O(n), including giant unbroken Unicode lines. Chunks concatenate to the
// original text, retaining whitespace and cutting only at UTF-8 boundaries.
fn chunks(text: &str, bytes: usize, lines: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut line_count = 0;
    for (index, ch) in text.char_indices() {
        if index > start && (index + ch.len_utf8() - start > bytes || line_count >= lines) {
            out.push(&text[start..index]);
            start = index;
            line_count = 0;
        }
        line_count += usize::from(ch == '\n');
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn text_parts(
    text: &str,
    role: Role,
    bytes: usize,
    lines: usize,
    continuation: bool,
) -> Vec<MailBodyBlock> {
    chunks(text, bytes, lines)
        .into_iter()
        .enumerate()
        .map(|(index, text)| {
            let literal = matches!(role, Role::Literal | Role::Code);
            let value = if literal {
                escape(text)
            } else {
                escape(text).replace('\n', "<br />")
            };
            let html = match role {
                // GPUI 0.3.4 has no white-space: pre-wrap, so <pre> is the only
                // element that retains whitespace. Wrap code with a soft, mono,
                // padded background to make long stack traces readable.
                Role::Literal | Role::Code => format!("<pre{PRE_STYLE}>{value}</pre>"),
                Role::Heading(level) => format!("<h{level}{HEADING_STYLE}>{value}</h{level}>"),
                Role::Quote => {
                    format!("<blockquote{BLOCKQUOTE_STYLE}><p{PARA_STYLE}>{value}</p></blockquote>")
                }
                Role::List(None) if index == 0 && !continuation => {
                    format!("<ul><li{PARA_STYLE}>{value}</li></ul>")
                }
                // TextView 0.6.1 ignores ol[start]. Explicit ordinals retain the
                // sequence across virtual chunks instead of restarting every item.
                Role::List(Some(number)) if index == 0 && !continuation => {
                    format!("<p{PARA_STYLE}>{number}. {value}</p>")
                }
                _ => format!("<p{PARA_STYLE}>{value}</p>"),
            };
            MailBodyBlock::Text {
                html,
                monospace: matches!(role, Role::Code),
            }
        })
        .collect()
}

fn prepare_table(rows: Vec<Vec<TableCell>>, out: &mut Vec<MailBodyBlock>) {
    for (row_index, row) in rows.into_iter().enumerate() {
        for (group, cells) in row.chunks(TABLE_COLUMNS_PER_BLOCK).enumerate() {
            if row.len() > TABLE_COLUMNS_PER_BLOCK {
                let first = group * TABLE_COLUMNS_PER_BLOCK + 1;
                let label = format!(
                    "表格第 {} 行 · 第 {first}–{} 列",
                    row_index + 1,
                    first + cells.len() - 1
                );
                out.extend(text_parts(
                    &label,
                    Role::Paragraph,
                    TEXT_CHUNK_BYTES,
                    TEXT_CHUNK_LINES,
                    false,
                ));
            }
            let count = cells.len().max(1);
            let parts = cells
                .iter()
                .map(|cell| {
                    if cell.occupied {
                        Vec::new()
                    } else {
                        inline_parts(
                            cell.spans.clone(),
                            Role::Paragraph,
                            (TEXT_CHUNK_BYTES / count).max(128),
                            (TEXT_CHUNK_LINES / count).max(2),
                        )
                    }
                })
                .collect::<Vec<_>>();
            let continuations = parts.iter().map(Vec::len).max().unwrap_or(0).max(1);
            for index in 0..continuations {
                out.push(MailBodyBlock::TableRow {
                    cells: cells
                        .iter()
                        .zip(&parts)
                        .map(|(cell, parts)| MailBodyCell {
                            header: cell.header && !cell.occupied,
                            content: parts.get(index).cloned(),
                        })
                        .collect(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn markup(blocks: &[MailBodyBlock]) -> String {
        blocks
            .iter()
            .map(|block| match block {
                MailBodyBlock::Text { html, .. } => html.clone(),
                MailBodyBlock::Image { .. } => String::new(),
                MailBodyBlock::TableRow { cells } => cells
                    .iter()
                    .filter_map(|cell| cell.content.as_ref())
                    .map(|block| markup(std::slice::from_ref(block)))
                    .collect(),
            })
            .collect()
    }
    #[test]
    fn literal_plain_text_cannot_become_markdown_html_or_a_resource() {
        let value = "# literal [link](https://tracker.invalid)\r\n<img src='file:///secret'> & <script>bad</script>";
        let blocks = prepare_blocks("", value);
        let rendered = markup(&blocks);
        assert!(rendered.starts_with("<pre ") && rendered.contains("># literal [link]"));
        assert!(rendered.contains("&lt;img src=&#39;file:///secret&#39;&gt;"));
        assert!(
            !rendered.contains("<img")
                && !rendered.contains("<script")
                && !rendered.contains("<a ")
        );
        assert!(!rendered.contains('\r'));
    }
    #[test]
    fn safe_textview_input_never_contains_mail_resource_tags() {
        let source = "<h2>标题</h2><p><a href='https://tracker.invalid'>链接文本</a><img src='https://tracker.invalid/pixel'><img src='cid:logo' alt='本地图'><img src='data:image/png;base64,AAAA'></p><script>danger()</script>";
        let blocks = prepare_blocks(source, "fallback");
        let text = markup(&blocks);
        assert!(text.contains(">标题</h2>"));
        assert!(text.contains("链接文本"));
        assert!(!text.contains("tracker.invalid") && !text.contains("danger()"));
        assert!(!text.contains("<img") && !text.contains("<a ") && !text.contains("src="));
        assert!(blocks
            .iter()
            .any(|block| matches!(block, MailBodyBlock::Image { cid, .. } if cid == "logo")));
    }

    #[test]
    fn div_heavy_mail_merges_safe_blocks_without_recursing_html() {
        let source = "<div><div>第一段</div><div><span>第二段</span></div></div>";
        let repeated = source.repeat(6);
        let blocks = prepare_blocks(&repeated, "fallback");
        assert_eq!(blocks.len(), 1);
        assert!(matches!(
            &blocks[0],
            MailBodyBlock::Text { html, monospace: false }
                if html.contains(">第一段</p>") && html.contains(">第二段</p>")
        ));
    }

    #[test]
    fn deeply_nested_foxmail_threads_stay_iterative() {
        let mut html = String::new();
        for _ in 0..94 {
            html.push_str("<div><blockquote>");
        }
        html.push_str("正文");
        for _ in 0..94 {
            html.push_str("</blockquote></div>");
        }
        let blocks = prepare_blocks(&html, "fallback");
        assert!(blocks.iter().any(|block| matches!(
            block,
            MailBodyBlock::Text { html, .. } if html.contains("正文")
        )));
    }
    #[test]
    fn unicode_and_unbroken_lines_are_bounded_without_losing_bytes() {
        let text = "字🙂e\u{301}&<> ".repeat(5000) + &"\n空行\n\n".repeat(1000);
        let split = chunks(&text, TEXT_CHUNK_BYTES, TEXT_CHUNK_LINES);
        assert_eq!(split.concat(), text);
        assert!(split.len() > 20);
        assert!(split.iter().all(|part| part.len() <= TEXT_CHUNK_BYTES
            && part.chars().filter(|c| *c == '\n').count() <= TEXT_CHUNK_LINES));
        assert!(prepare_blocks("", &text).iter().all(|block| matches!(block, MailBodyBlock::Text { html, .. } if html.len() <= TEXT_CHUNK_BYTES*6+64)));
    }
    #[test]
    fn headings_quotes_pre_and_ordered_lists_keep_their_meaning() {
        let blocks = prepare_blocks("<h1>题目</h1><blockquote>引用</blockquote><pre>  a &lt; b\n    c</pre><ol><li>第一</li><li>第二</li></ol>", "");
        let text = markup(&blocks);
        assert!(text.contains(">题目</h1>") && text.contains("<blockquote"));
        assert!(text.contains("1. 第一") && text.contains("2. 第二"));
        assert!(blocks.iter().any(|block| matches!(block, MailBodyBlock::Text { monospace: true, html } if html.contains("a &lt; b"))));
    }
    #[test]
    fn paragraph_quote_pre_and_heading_markup_carries_inline_style_hooks() {
        // L1 readability: GPUI 0.3.4 doesn't parse inline style, but the
        // markup still has to carry the hook so a future TextView upgrade
        // picks it up, and so a regression to plain `<p>` is loud.
        let blocks = prepare_blocks(
            "<h2>主题</h2><p>各位：附件第38周更新ECN变更跟进明细；请知悉!</p><blockquote>抄送：陈焱佩</blockquote><pre>  a &lt; b\n    c</pre>",
            "",
        );
        let text = markup(&blocks);
        assert!(
            text.contains("<h2 style="),
            "heading missing inline style: {text}"
        );
        assert!(
            text.contains("<p style="),
            "paragraph missing inline style: {text}"
        );
        assert!(
            text.contains("<blockquote style="),
            "blockquote missing inline style: {text}"
        );
        assert!(
            text.contains("<pre style="),
            "pre missing inline style: {text}"
        );
        assert!(
            text.contains("line-height:1.65"),
            "paragraph line-height hook missing: {text}"
        );
    }
    #[test]
    fn table_empty_span_cells_and_cids_do_not_shift_columns() {
        let blocks = prepare_blocks("<table><tr><th>A</th><th>B</th></tr><tr><td rowspan='2'>one<img src='cid:logo'></td><td>two</td></tr><tr><td>three</td></tr></table>", "");
        let rows = blocks
            .iter()
            .filter_map(|block| {
                if let MailBodyBlock::TableRow { cells } = block {
                    Some(cells)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert!(rows.iter().all(|row| row.len() == 2));
        assert!(rows[0].iter().all(|cell| cell.header));
        assert!(rows.iter().any(
            |row| matches!(&row[0].content, Some(MailBodyBlock::Image { cid, .. }) if cid == "logo")
        ));
        let last = rows.last().unwrap();
        assert!(last[0].content.is_none());
        assert!(
            matches!(&last[1].content, Some(MailBodyBlock::Text { html, .. }) if html.contains("three"))
        );
    }
    #[test]
    fn huge_tables_bound_work_per_row_and_preserve_tail_content() {
        let row = format!(
            "<tr>{}</tr>",
            (0..65)
                .map(|n| format!("<td>column-{n} {}</td>", "超长单元格".repeat(200)))
                .collect::<String>()
        );
        let blocks = prepare_blocks(
            &format!("<table>{row}<tr><td>TAIL-KEEP</td></tr></table>"),
            "",
        );
        assert!(blocks.len() > 65);
        for block in &blocks {
            if let MailBodyBlock::TableRow { cells } = block {
                assert!(cells.len() <= TABLE_COLUMNS_PER_BLOCK);
                let bytes: usize = cells
                    .iter()
                    .filter_map(|cell| cell.content.as_ref())
                    .map(|part| markup(std::slice::from_ref(part)).len())
                    .sum();
                assert!(bytes <= TEXT_CHUNK_BYTES * 6 + 1024);
            }
        }
        assert!(markup(&blocks).contains("TAIL-KEEP"));
    }
    #[test]
    fn empty_and_sanitized_away_html_fall_back_without_panicking() {
        assert!(prepare_blocks("", " \r\n").is_empty());
        assert!(markup(&prepare_blocks("<script>bad()</script>", "后备正文")).contains("后备正文"));
    }
}
