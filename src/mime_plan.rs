use imap_proto::types::{
    BodyContentCommon, BodyContentSinglePart, BodyStructure, ContentDisposition, ContentEncoding,
    SectionPath,
};
use serde::{Deserialize, Serialize};

pub const SKIP_ATTACHMENT_OCTETS: u32 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MimePlan {
    pub texts: Vec<TextPart>,
    pub attachments: Vec<AttachmentPart>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextPart {
    pub section: String,
    pub subtype: String,
    pub charset: String,
    pub encoding: String,
    pub octets: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentPart {
    pub section: String,
    pub mime: String,
    pub filename: String,
    pub encoding: String,
    pub octets: u32,
    pub inline: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentSidecar {
    pub version: u8,
    pub mailbox: String,
    pub uid: u32,
    pub attachments: Vec<SidecarAttachment>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarAttachment {
    pub index: usize,
    pub section: String,
    pub encoding: String,
    pub filename: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeferredAttachment {
    pub mailbox: String,
    pub uid: u32,
    pub section: String,
    pub encoding: String,
}

impl MimePlan {
    pub fn from_bodystructure(body: &BodyStructure<'_>) -> Self {
        let mut plan = Self {
            texts: Vec::new(),
            attachments: Vec::new(),
        };
        walk(body, &[], &mut plan);
        plan
    }

    pub fn omit_large_attachments(&self) -> bool {
        self.attachments
            .iter()
            .any(|part| part.octets >= SKIP_ATTACHMENT_OCTETS)
    }

    pub fn sidecar(&self, mailbox: &str, uid: u32) -> AttachmentSidecar {
        AttachmentSidecar {
            version: 1,
            mailbox: mailbox.to_owned(),
            uid,
            attachments: self
                .attachments
                .iter()
                .enumerate()
                .map(|(index, part)| SidecarAttachment {
                    index,
                    section: part.section.clone(),
                    encoding: part.encoding.clone(),
                    filename: part.filename.clone(),
                })
                .collect(),
        }
    }
}

pub fn section_path(section: &str) -> Option<SectionPath> {
    let parts = section
        .split('.')
        .map(|item| item.parse::<u32>().ok())
        .collect::<Option<Vec<_>>>()?;
    if parts.is_empty() {
        None
    } else {
        Some(SectionPath::Part(parts, None))
    }
}

pub fn rewrite_headers(header: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(header);
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.trim_end_matches('\r');
        if raw.starts_with([' ', '\t']) {
            if let Some(previous) = lines.last_mut() {
                previous.push(' ');
                previous.push_str(raw.trim());
            }
            continue;
        }
        if !raw.is_empty() {
            lines.push(raw.to_owned());
        }
    }
    lines.retain(|line| {
        let name = line.split_once(':').map(|(name, _)| name.trim());
        !matches!(
            name.map(str::to_ascii_lowercase).as_deref(),
            Some("content-type" | "content-transfer-encoding" | "mime-version")
        )
    });
    let mut out = lines.join("\r\n").into_bytes();
    if !out.is_empty() {
        out.extend_from_slice(b"\r\n");
    }
    out
}

pub fn build_partial_eml(header: &[u8], plan: &MimePlan, texts: &[Vec<u8>]) -> Vec<u8> {
    const BOUNDARY: &str = "foxmail-partial";
    let mut out = rewrite_headers(header);
    out.extend_from_slice(
        format!(
            "MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"{BOUNDARY}\"\r\n\r\n"
        )
        .as_bytes(),
    );
    for (part, bytes) in plan.texts.iter().zip(texts.iter()) {
        out.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        out.extend_from_slice(
            format!(
                "Content-Type: text/{}; charset={}\r\nContent-Transfer-Encoding: {}\r\n\r\n",
                part.subtype, part.charset, part.encoding
            )
            .as_bytes(),
        );
        out.extend_from_slice(bytes);
        if !bytes.ends_with(b"\r\n") {
            out.extend_from_slice(b"\r\n");
        }
    }
    for part in &plan.attachments {
        let filename = sanitize_param(&part.filename);
        out.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        out.extend_from_slice(
            format!(
                "Content-Type: {}; name=\"{filename}\"\r\nContent-Disposition: {}; filename=\"{filename}\"\r\nX-Foxmail-Imap-Section: {}\r\nContent-Transfer-Encoding: {}\r\n\r\n",
                part.mime,
                if part.inline { "inline" } else { "attachment" },
                part.section,
                part.encoding
            )
            .as_bytes(),
        );
    }
    out.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    out
}

fn walk(body: &BodyStructure<'_>, path: &[u32], plan: &mut MimePlan) {
    match body {
        BodyStructure::Multipart { bodies, .. } => {
            for (index, child) in bodies.iter().enumerate() {
                let mut next = path.to_vec();
                next.push(u32::try_from(index + 1).unwrap_or(1));
                walk(child, &next, plan);
            }
        }
        BodyStructure::Text { common, other, .. } => {
            let subtype = common.ty.subtype.to_ascii_lowercase();
            if matches!(subtype.as_str(), "plain" | "html" | "calendar") {
                plan.texts.push(TextPart {
                    section: section_id(path),
                    subtype,
                    charset: param(&common.ty.params, "charset")
                        .unwrap_or_else(|| "utf-8".to_owned()),
                    encoding: encoding_label(&other.transfer_encoding),
                    octets: other.octets,
                });
            } else {
                push_attachment(plan, path, common, other);
            }
        }
        BodyStructure::Basic { common, other, .. }
        | BodyStructure::Message { common, other, .. } => {
            push_attachment(plan, path, common, other);
        }
    }
}

fn push_attachment(
    plan: &mut MimePlan,
    path: &[u32],
    common: &BodyContentCommon<'_>,
    other: &BodyContentSinglePart<'_>,
) {
    let inline = is_inline(common.disposition.as_ref(), other.id.as_deref());
    plan.attachments.push(AttachmentPart {
        section: section_id(path),
        mime: format!(
            "{}/{}",
            common.ty.ty.to_ascii_lowercase(),
            common.ty.subtype.to_ascii_lowercase()
        ),
        filename: filename(common, other),
        encoding: encoding_label(&other.transfer_encoding),
        octets: other.octets,
        inline,
    });
}

fn section_id(path: &[u32]) -> String {
    if path.is_empty() {
        "1".to_owned()
    } else {
        path.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(".")
    }
}

fn encoding_label(encoding: &ContentEncoding<'_>) -> String {
    match encoding {
        ContentEncoding::SevenBit => "7bit".to_owned(),
        ContentEncoding::EightBit => "8bit".to_owned(),
        ContentEncoding::Binary => "binary".to_owned(),
        ContentEncoding::Base64 => "base64".to_owned(),
        ContentEncoding::QuotedPrintable => "quoted-printable".to_owned(),
        ContentEncoding::Other(value) => value.to_ascii_lowercase(),
    }
}

fn is_inline(disposition: Option<&ContentDisposition<'_>>, id: Option<&str>) -> bool {
    disposition.is_some_and(|item| item.ty.eq_ignore_ascii_case("inline"))
        || id.is_some_and(|value| !value.trim().is_empty())
}

fn filename(common: &BodyContentCommon<'_>, other: &BodyContentSinglePart<'_>) -> String {
    param(
        common
            .disposition
            .as_ref()
            .map(|item| &item.params)
            .unwrap_or(&None),
        "filename",
    )
    .or_else(|| param(&common.ty.params, "name"))
    .or_else(|| {
        other
            .description
            .as_ref()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    })
    .unwrap_or_else(|| format!("part-{}.bin", other.octets))
}

fn param(params: &imap_proto::types::BodyParams<'_>, name: &str) -> Option<String> {
    params.as_ref().and_then(|items| {
        items
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.to_string())
    })
}

fn sanitize_param(value: &str) -> String {
    value.replace(['"', '\r', '\n', '\\'], "_")
}

#[cfg(test)]
use imap_proto::types::ContentType;
#[cfg(test)]
use std::borrow::Cow;

#[cfg(test)]
fn text_common(subtype: &'static str, charset: &'static str) -> BodyContentCommon<'static> {
    BodyContentCommon {
        ty: ContentType {
            ty: Cow::Borrowed("text"),
            subtype: Cow::Borrowed(subtype),
            params: Some(vec![(Cow::Borrowed("charset"), Cow::Borrowed(charset))]),
        },
        disposition: None,
        language: None,
        location: None,
    }
}

#[cfg(test)]
fn single(octets: u32, encoding: ContentEncoding<'static>) -> BodyContentSinglePart<'static> {
    BodyContentSinglePart {
        id: None,
        md5: None,
        description: None,
        transfer_encoding: encoding,
        octets,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pdf_part(octets: u32) -> BodyStructure<'static> {
        BodyStructure::Basic {
            common: BodyContentCommon {
                ty: ContentType {
                    ty: Cow::Borrowed("application"),
                    subtype: Cow::Borrowed("pdf"),
                    params: Some(vec![(Cow::Borrowed("name"), Cow::Borrowed("report.pdf"))]),
                },
                disposition: Some(ContentDisposition {
                    ty: Cow::Borrowed("attachment"),
                    params: Some(vec![(
                        Cow::Borrowed("filename"),
                        Cow::Borrowed("report.pdf"),
                    )]),
                }),
                language: None,
                location: None,
            },
            other: single(octets, ContentEncoding::Base64),
            extension: None,
        }
    }

    #[test]
    fn mixed_message_plans_text_and_large_pdf() {
        let body = BodyStructure::Multipart {
            common: BodyContentCommon {
                ty: ContentType {
                    ty: Cow::Borrowed("multipart"),
                    subtype: Cow::Borrowed("mixed"),
                    params: None,
                },
                disposition: None,
                language: None,
                location: None,
            },
            bodies: vec![
                BodyStructure::Text {
                    common: text_common("plain", "utf-8"),
                    other: single(12, ContentEncoding::SevenBit),
                    lines: 1,
                    extension: None,
                },
                BodyStructure::Text {
                    common: text_common("html", "utf-8"),
                    other: single(40, ContentEncoding::QuotedPrintable),
                    lines: 4,
                    extension: None,
                },
                pdf_part(2 * 1024 * 1024),
            ],
            extension: None,
        };
        let plan = MimePlan::from_bodystructure(&body);
        assert_eq!(plan.texts[0].section, "1");
        assert_eq!(plan.texts[1].section, "2");
        assert_eq!(plan.attachments[0].section, "3");
        assert_eq!(plan.attachments[0].filename, "report.pdf");
        assert!(plan.omit_large_attachments());
        assert_eq!(section_path("3").unwrap(), SectionPath::Part(vec![3], None));
    }

    #[test]
    fn tiny_attachment_keeps_full_rfc822_download() {
        let body = pdf_part(100);
        let plan = MimePlan::from_bodystructure(&body);
        assert!(!plan.omit_large_attachments());
        assert_eq!(plan.attachments[0].section, "1");
    }

    #[test]
    fn rewrite_headers_drops_content_type_and_unfolds() {
        let rewritten = rewrite_headers(
            b"Subject: Hello\r\nContent-Type: multipart/mixed;\r\n\tboundary=x\r\nFrom: a@b\r\n\r\n",
        );
        let text = String::from_utf8(rewritten).unwrap();
        assert!(text.contains("Subject: Hello"));
        assert!(text.contains("From: a@b"));
        assert!(!text.to_ascii_lowercase().contains("content-type"));
    }
}
