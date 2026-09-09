use std::borrow::Cow;

pub fn sanitize_html(html: &str) -> String {
    let mut builder = ammonia::Builder::default();
    builder.add_url_schemes(["cid", "data"]);
    builder.attribute_filter(|element, attribute, value| match (element, attribute) {
        ("img", "src") => allow_local_image(value).then(|| Cow::Borrowed(value)),
        _ => Some(Cow::Borrowed(value)),
    });
    builder.clean(html).to_string()
}

pub fn html_to_plain(html: &str) -> String {
    tags_to_plain(&sanitize_html(html))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InlineSpan {
    Text(String),
    Break,
    Image { cid: String, alt: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BodyBlock {
    Heading {
        level: u8,
        spans: Vec<InlineSpan>,
    },
    Paragraph {
        spans: Vec<InlineSpan>,
    },
    Quote {
        spans: Vec<InlineSpan>,
    },
    Pre {
        text: String,
    },
    ListItem {
        ordered: bool,
        spans: Vec<InlineSpan>,
    },
    Table {
        rows: Vec<Vec<TableCell>>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableCell {
    pub header: bool,
    pub spans: Vec<InlineSpan>,
    pub occupied: bool,
    rowspan: u32,
    colspan: u32,
}

struct TableBuilder {
    rows: Vec<Vec<TableCell>>,
    current_row: Vec<TableCell>,
    current_cell: Vec<InlineSpan>,
    current_header: bool,
    current_rowspan: u32,
    current_colspan: u32,
    in_cell: bool,
}

pub fn layout_html(html: &str) -> Vec<BodyBlock> {
    layout_sanitized(&sanitize_html(html))
}

fn layout_sanitized(value: &str) -> Vec<BodyBlock> {
    let mut blocks = Vec::new();
    let mut spans = Vec::new();
    let mut tables: Vec<TableBuilder> = Vec::new();
    let mut pre = String::new();
    let mut in_pre = false;
    let mut list_ordered = false;
    let mut cursor = 0;

    let flush_paragraph = |blocks: &mut Vec<BodyBlock>, spans: &mut Vec<InlineSpan>| {
        if spans.iter().any(|span| !matches!(span, InlineSpan::Break)) {
            blocks.push(BodyBlock::Paragraph {
                spans: std::mem::take(spans),
            });
        } else {
            spans.clear();
        }
    };

    while cursor < value.len() {
        let Some(relative_start) = value[cursor..].find('<') else {
            push_text(
                active_spans(&mut spans, &mut tables),
                &mut pre,
                in_pre,
                &value[cursor..],
            );
            break;
        };
        let start = cursor + relative_start;
        if start > cursor {
            push_text(
                active_spans(&mut spans, &mut tables),
                &mut pre,
                in_pre,
                &value[cursor..start],
            );
        }
        if value[start..].starts_with("<!--") {
            let Some(relative_end) = value[start + 4..].find("-->") else {
                break;
            };
            cursor = start + 4 + relative_end + 3;
            continue;
        }
        let Some(end) = find_tag_end(value, start + 1) else {
            push_text(
                active_spans(&mut spans, &mut tables),
                &mut pre,
                in_pre,
                &value[start..],
            );
            break;
        };
        let raw_tag = value[start + 1..end].trim();
        let closing = raw_tag.starts_with('/');
        let (tag, attrs) = split_tag(raw_tag);
        match (tag.as_str(), closing) {
            ("br", _) => active_spans(&mut spans, &mut tables).push(InlineSpan::Break),
            ("img", false) => {
                if let Some(cid) = cid_from_src(attr_value(&attrs, "src").as_deref()) {
                    active_spans(&mut spans, &mut tables).push(InlineSpan::Image {
                        cid,
                        alt: attr_value(&attrs, "alt").unwrap_or_default(),
                    });
                }
            }
            ("pre", false) => {
                flush_paragraph(&mut blocks, &mut spans);
                in_pre = true;
                pre.clear();
            }
            ("pre", true) => {
                if !pre.is_empty() {
                    blocks.push(BodyBlock::Pre {
                        text: std::mem::take(&mut pre),
                    });
                }
                in_pre = false;
            }
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => {
                flush_paragraph(&mut blocks, &mut spans);
            }
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) => {
                if !spans.is_empty() {
                    blocks.push(BodyBlock::Heading {
                        level: tag.as_bytes()[1].saturating_sub(b'0'),
                        spans: std::mem::take(&mut spans),
                    });
                }
            }
            ("blockquote", false) => flush_paragraph(&mut blocks, &mut spans),
            ("blockquote", true) => {
                if !spans.is_empty() {
                    blocks.push(BodyBlock::Quote {
                        spans: std::mem::take(&mut spans),
                    });
                }
            }
            ("ul", false) => {
                flush_paragraph(&mut blocks, &mut spans);
                list_ordered = false;
            }
            ("ol", false) => {
                flush_paragraph(&mut blocks, &mut spans);
                list_ordered = true;
            }
            ("li", false) => flush_paragraph(&mut blocks, &mut spans),
            ("li", true) => {
                if !spans.is_empty() {
                    blocks.push(BodyBlock::ListItem {
                        ordered: list_ordered,
                        spans: std::mem::take(&mut spans),
                    });
                }
            }
            ("table", false) => {
                flush_paragraph(&mut blocks, &mut spans);
                tables.push(TableBuilder {
                    rows: Vec::new(),
                    current_row: Vec::new(),
                    current_cell: Vec::new(),
                    current_header: false,
                    current_rowspan: 1,
                    current_colspan: 1,
                    in_cell: false,
                });
            }
            ("table", true) => {
                if let Some(table) = tables.pop() {
                    let rows = finish_table(table);
                    if rows.is_empty() {
                    } else if let Some(parent) = tables.last_mut() {
                        push_nested_table(&mut parent.current_cell, &rows);
                    } else {
                        blocks.push(BodyBlock::Table { rows });
                    }
                }
            }
            ("tr", false) => {
                if let Some(table) = tables.last_mut() {
                    finish_cell(table);
                    finish_row(table);
                } else {
                    flush_paragraph(&mut blocks, &mut spans);
                }
            }
            ("tr", true) => {
                if let Some(table) = tables.last_mut() {
                    finish_cell(table);
                    finish_row(table);
                }
            }
            ("td" | "th", false) => {
                if let Some(table) = tables.last_mut() {
                    finish_cell(table);
                    table.in_cell = true;
                    table.current_header = tag == "th";
                    table.current_rowspan = parse_span(&attrs, "rowspan");
                    table.current_colspan = parse_span(&attrs, "colspan");
                }
            }
            ("td" | "th", true) => {
                if let Some(table) = tables.last_mut() {
                    finish_cell(table);
                }
            }
            ("p" | "div", _) => {
                if tables.is_empty() {
                    flush_paragraph(&mut blocks, &mut spans);
                } else {
                    active_spans(&mut spans, &mut tables).push(InlineSpan::Break);
                }
            }
            _ => {}
        }
        cursor = end + 1;
    }
    if in_pre && !pre.is_empty() {
        blocks.push(BodyBlock::Pre { text: pre });
    } else {
        flush_paragraph(&mut blocks, &mut spans);
    }
    while let Some(table) = tables.pop() {
        let rows = finish_table(table);
        if !rows.is_empty() {
            blocks.push(BodyBlock::Table { rows });
        }
    }
    blocks
}

fn active_spans<'a>(
    spans: &'a mut Vec<InlineSpan>,
    tables: &'a mut [TableBuilder],
) -> &'a mut Vec<InlineSpan> {
    tables
        .last_mut()
        .map(|table| &mut table.current_cell)
        .unwrap_or(spans)
}

fn finish_cell(table: &mut TableBuilder) {
    if !table.in_cell {
        table.current_cell.clear();
        return;
    }
    table.current_row.push(TableCell {
        header: table.current_header,
        spans: std::mem::take(&mut table.current_cell),
        occupied: false,
        rowspan: table.current_rowspan.max(1),
        colspan: table.current_colspan.max(1),
    });
    table.in_cell = false;
    table.current_header = false;
    table.current_rowspan = 1;
    table.current_colspan = 1;
}

fn finish_row(table: &mut TableBuilder) {
    if !table.current_row.is_empty() {
        table.rows.push(std::mem::take(&mut table.current_row));
    }
}

fn finish_table(mut table: TableBuilder) -> Vec<Vec<TableCell>> {
    finish_cell(&mut table);
    finish_row(&mut table);
    expand_table_spans(table.rows)
}

fn parse_span(attrs: &str, name: &str) -> u32 {
    attr_value(attrs, name)
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1)
        .min(32)
}

fn occupied_cell() -> TableCell {
    TableCell {
        header: false,
        spans: Vec::new(),
        occupied: true,
        rowspan: 1,
        colspan: 1,
    }
}

fn expand_table_spans(rows: Vec<Vec<TableCell>>) -> Vec<Vec<TableCell>> {
    if rows.is_empty() {
        return rows;
    }
    let mut skip: Vec<Vec<bool>> = Vec::new();
    let mut grid = Vec::new();
    let mark = |skip: &mut Vec<Vec<bool>>, row: usize, col: usize| {
        while skip.len() <= row {
            skip.push(Vec::new());
        }
        while skip[row].len() <= col {
            skip[row].push(false);
        }
        skip[row][col] = true;
    };
    let taken = |skip: &[Vec<bool>], row: usize, col: usize| {
        skip.get(row).and_then(|line| line.get(col)).copied() == Some(true)
    };
    for (row_index, row) in rows.into_iter().enumerate() {
        let mut col = 0usize;
        let mut out = Vec::new();
        for cell in row {
            while taken(&skip, row_index, col) {
                out.push(occupied_cell());
                col += 1;
            }
            let rowspan = cell.rowspan.max(1) as usize;
            let colspan = cell.colspan.max(1) as usize;
            for row_offset in 0..rowspan {
                for col_offset in 0..colspan {
                    if row_offset == 0 && col_offset == 0 {
                        continue;
                    }
                    mark(&mut skip, row_index + row_offset, col + col_offset);
                }
            }
            out.push(TableCell {
                rowspan: 1,
                colspan: 1,
                ..cell
            });
            for _ in 1..colspan {
                out.push(occupied_cell());
                col += 1;
            }
            col += 1;
        }
        while taken(&skip, row_index, col) {
            out.push(occupied_cell());
            col += 1;
        }
        grid.push(out);
    }
    let width = grid.iter().map(Vec::len).max().unwrap_or(0);
    for row in &mut grid {
        while row.len() < width {
            row.push(occupied_cell());
        }
    }
    grid
}

fn push_nested_table(spans: &mut Vec<InlineSpan>, rows: &[Vec<TableCell>]) {
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            spans.push(InlineSpan::Break);
        }
        let mut line = String::new();
        for (cell_index, cell) in row.iter().enumerate() {
            if cell_index > 0 {
                line.push_str(" | ");
            }
            line.push_str(&spans_to_text(&cell.spans));
        }
        let line = collapse_ws(&line);
        if !line.is_empty() {
            spans.push(InlineSpan::Text(line));
        }
    }
}

fn spans_to_text(spans: &[InlineSpan]) -> String {
    let mut out = String::new();
    for span in spans {
        match span {
            InlineSpan::Text(text) => out.push_str(text),
            InlineSpan::Break => out.push(' '),
            InlineSpan::Image { alt, .. } => {
                if !alt.is_empty() {
                    out.push_str(alt);
                }
            }
        }
    }
    out
}

fn push_text(spans: &mut Vec<InlineSpan>, pre: &mut String, in_pre: bool, raw: &str) {
    let mut text = String::new();
    append_decoded_text(&mut text, raw);
    if in_pre {
        pre.push_str(&text);
        return;
    }
    let collapsed = collapse_ws(&text);
    if collapsed.is_empty() {
        return;
    }
    spans.push(InlineSpan::Text(collapsed));
}

fn collapse_ws(value: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for character in value.chars() {
        if character.is_whitespace() {
            space = true;
            continue;
        }
        if space && !out.is_empty() {
            out.push(' ');
        }
        space = false;
        out.push(character);
    }
    out
}

fn split_tag(raw: &str) -> (String, String) {
    let raw = raw
        .trim_start_matches('/')
        .trim_start_matches('!')
        .trim_start_matches('?')
        .trim_end_matches('/');
    let mut parts = raw.splitn(2, char::is_whitespace);
    let tag = parts
        .next()
        .unwrap_or_default()
        .trim_matches(|character: char| !character.is_ascii_alphanumeric())
        .to_ascii_lowercase();
    let attrs = parts.next().unwrap_or_default().to_owned();
    (tag, attrs)
}

fn attr_value(attrs: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=");
    let lower = attrs.to_ascii_lowercase();
    let index = lower.find(&needle)?;
    let rest = attrs[index + needle.len()..].trim_start();
    if let Some(quote) = rest
        .chars()
        .next()
        .filter(|character| matches!(character, '"' | '\''))
    {
        let inner = rest.get(1..)?;
        let end = inner.find(quote).unwrap_or(inner.len());
        Some(inner[..end].to_owned())
    } else {
        Some(
            rest.split(|character: char| character.is_whitespace() || character == '>')
                .next()
                .unwrap_or_default()
                .to_owned(),
        )
    }
}

pub fn cid_from_src(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    let stripped = value
        .strip_prefix("cid:")
        .or_else(|| value.strip_prefix("CID:"))?;
    Some(stripped.trim_matches(['<', '>', '"', '\'', ' ']).to_owned())
}

fn allow_local_image(value: &str) -> bool {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    lower.starts_with("cid:") || lower.starts_with("data:")
}

fn tags_to_plain(value: &str) -> String {
    const BLOCK_TAGS: &[&str] = &[
        "address",
        "article",
        "aside",
        "blockquote",
        "dd",
        "div",
        "dl",
        "dt",
        "figcaption",
        "figure",
        "footer",
        "form",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "header",
        "hr",
        "li",
        "main",
        "nav",
        "ol",
        "p",
        "pre",
        "section",
        "table",
        "tbody",
        "td",
        "tfoot",
        "th",
        "thead",
        "tr",
        "ul",
    ];
    const VOID_TAGS: &[&str] = &["br", "hr"];
    const BLOCKED_TAGS: &[&str] = &[
        "applet", "embed", "iframe", "object", "script", "style", "svg", "template",
    ];

    let mut output = String::new();
    let mut cursor = 0;
    while cursor < value.len() {
        let Some(relative_start) = value[cursor..].find('<') else {
            append_decoded_text(&mut output, &value[cursor..]);
            break;
        };
        let start = cursor + relative_start;
        append_decoded_text(&mut output, &value[cursor..start]);

        if value[start..].starts_with("<!--") {
            let Some(relative_end) = value[start + 4..].find("-->") else {
                break;
            };
            cursor = start + 4 + relative_end + 3;
            continue;
        }

        let Some(end) = find_tag_end(value, start + 1) else {
            append_decoded_text(&mut output, &value[start..]);
            break;
        };
        let raw_tag = value[start + 1..end].trim();
        let closing = raw_tag.starts_with('/');
        let tag = raw_tag
            .trim_start_matches('/')
            .trim_start_matches('!')
            .trim_start_matches('?')
            .split_ascii_whitespace()
            .next()
            .unwrap_or_default()
            .trim_end_matches('/');
        let tag = tag
            .trim_matches(|character: char| !character.is_ascii_alphanumeric())
            .to_ascii_lowercase();

        if BLOCKED_TAGS.contains(&tag.as_str()) {
            if !closing {
                let closing_marker = format!("</{tag}");
                if let Some(relative_close) =
                    value[end + 1..].to_ascii_lowercase().find(&closing_marker)
                {
                    cursor = end + 1 + relative_close;
                    continue;
                }
                break;
            }
        } else if VOID_TAGS.contains(&tag.as_str()) || BLOCK_TAGS.contains(&tag.as_str()) {
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
        }

        cursor = end + 1;
    }

    output
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_end()
        .to_owned()
}

fn find_tag_end(value: &str, mut cursor: usize) -> Option<usize> {
    let mut quote = None;
    while cursor < value.len() {
        let character = value[cursor..].chars().next()?;
        cursor += character.len_utf8();
        match (quote, character) {
            (Some(expected), current) if expected == current => quote = None,
            (None, '\'') | (None, '"') => quote = Some(character),
            (None, '>') => return Some(cursor - 1),
            _ => {}
        }
    }
    None
}

fn append_decoded_text(output: &mut String, text: &str) {
    let mut cursor = 0;
    while cursor < text.len() {
        let Some(relative_start) = text[cursor..].find('&') else {
            output.push_str(&text[cursor..]);
            break;
        };
        let start = cursor + relative_start;
        output.push_str(&text[cursor..start]);
        let Some(relative_end) = text[start..].find(';') else {
            output.push('&');
            cursor = start + 1;
            continue;
        };
        let end = start + relative_end;
        let entity = &text[start + 1..end];
        if let Some(decoded) = decode_entity(entity) {
            output.push(decoded);
            cursor = end + 1;
        } else {
            output.push('&');
            cursor = start + 1;
        }
    }
}

fn decode_entity(entity: &str) -> Option<char> {
    match entity.to_ascii_lowercase().as_str() {
        "amp" => Some('&'),
        "apos" => Some('\''),
        "gt" => Some('>'),
        "lt" => Some('<'),
        "nbsp" => Some(' '),
        "quot" => Some('"'),
        _ if entity.starts_with("#x") || entity.starts_with("#X") => {
            u32::from_str_radix(&entity[2..], 16)
                .ok()
                .and_then(char::from_u32)
        }
        _ if entity.starts_with('#') => entity[1..].parse::<u32>().ok().and_then(char::from_u32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{html_to_plain, layout_html, sanitize_html, BodyBlock, InlineSpan};

    #[test]
    fn html_to_plain_keeps_structure_and_entities() {
        assert_eq!(
            html_to_plain(
                "<h1>标题</h1><p>第一行<br>第二行 &amp; &#x4E2D;</p><ul><li>项目</li></ul>"
            ),
            "标题\n第一行\n第二行 & 中\n项目"
        );
    }

    #[test]
    fn html_to_plain_drops_scripts_and_remote_resources() {
        let rendered = html_to_plain(
            "<p onclick=alert(1)>正文</p><script>alert(2)</script><style>body{color:red}</style>\
             <iframe src='https://evil.test'>外部内容</iframe><img src='https://evil.test/x' alt='图片'>\
             <a href='javascript:alert(3)'>安全链接文本</a>",
        );
        assert!(rendered.contains("正文"));
        assert!(rendered.contains("安全链接文本"));
        assert!(!rendered.contains("alert"));
        assert!(!rendered.contains("evil.test"));
    }

    #[test]
    fn sanitize_html_keeps_cid_images_and_strips_remote_pixels() {
        let cleaned = sanitize_html(
            "<p>hi</p><img src=\"cid:logo@mail\"><img src=\"https://tracker.test/pixel.gif\">",
        );
        assert!(cleaned.contains("cid:logo@mail"));
        assert!(!cleaned.contains("tracker.test"));
        assert!(!cleaned.contains("https://"));
    }

    #[test]
    fn layout_html_keeps_headings_lists_quotes_and_cid_images() {
        let blocks = layout_html(
            "<h1>标题</h1><p>第一行<br>第二行</p><blockquote>引用</blockquote>\
             <ul><li>一项</li></ul><img src=\"cid:logo@mail\" alt=\"标志\">",
        );
        assert!(
            matches!(&blocks[0], BodyBlock::Heading { level: 1, spans } if matches!(&spans[0], InlineSpan::Text(text) if text == "标题"))
        );
        assert!(matches!(&blocks[1], BodyBlock::Paragraph { .. }));
        assert!(matches!(&blocks[2], BodyBlock::Quote { .. }));
        assert!(matches!(
            &blocks[3],
            BodyBlock::ListItem { ordered: false, .. }
        ));
        assert!(blocks.iter().any(|block| matches!(
            block,
            BodyBlock::Paragraph { spans } if spans.iter().any(|span| matches!(span, InlineSpan::Image { cid, .. } if cid == "logo@mail"))
        )) || matches!(
            blocks.last(),
            Some(BodyBlock::Paragraph { spans }) if spans.iter().any(|span| matches!(span, InlineSpan::Image { cid, .. } if cid == "logo@mail"))
        ));
    }

    #[test]
    fn layout_html_keeps_table_rows_and_header_cells() {
        let blocks = layout_html(
            "<p>说明</p><table><tr><th>项目</th><th>内容</th></tr>\
             <tr><td>结构</td><td>T1 系统</td></tr></table>",
        );
        let Some(BodyBlock::Table { rows }) = blocks
            .iter()
            .find(|block| matches!(block, BodyBlock::Table { .. }))
        else {
            panic!("expected table block, got {blocks:?}");
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].len(), 2);
        assert!(rows[0][0].header);
        assert!(!rows[1][0].header);
        assert!(matches!(&rows[0][0].spans[0], InlineSpan::Text(text) if text == "项目"));
        assert!(matches!(&rows[1][1].spans[0], InlineSpan::Text(text) if text == "T1 系统"));
    }

    #[test]
    fn layout_html_preserves_rowspan_columns() {
        let blocks = layout_html(
            "<table><tr><th>系列</th><th>代码</th><th>名称</th><th>状态</th></tr>\
             <tr><td rowspan=\"2\">S系列</td><td>S1</td><td>认知</td><td>已开展</td></tr>\
             <tr><td>S2</td><td>场景化方案</td><td>后续批次</td></tr></table>",
        );
        let Some(BodyBlock::Table { rows }) = blocks
            .iter()
            .find(|block| matches!(block, BodyBlock::Table { .. }))
        else {
            panic!("expected table block, got {blocks:?}");
        };
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].len(), 4);
        assert_eq!(rows[1].len(), 4);
        assert_eq!(rows[2].len(), 4);
        assert!(!rows[1][0].occupied);
        assert!(matches!(&rows[1][0].spans[0], InlineSpan::Text(text) if text == "S系列"));
        assert!(matches!(&rows[1][1].spans[0], InlineSpan::Text(text) if text == "S1"));
        assert!(rows[2][0].occupied);
        assert!(matches!(&rows[2][1].spans[0], InlineSpan::Text(text) if text == "S2"));
        assert!(matches!(&rows[2][2].spans[0], InlineSpan::Text(text) if text.contains("场景化")));
    }
}
