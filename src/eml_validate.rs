use mail_parser::MessageParser;
use serde::Serialize;
use std::error::Error;
use std::fs;
use std::path::Path;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct EmlValidation {
    pub bytes: usize,
    pub subject_present: bool,
    pub from_present: bool,
    pub date_present: bool,
    pub text_parts: usize,
    pub html_parts: usize,
    pub attachments: usize,
}

impl EmlValidation {
    pub fn has_required_headers(&self) -> bool {
        self.subject_present && self.from_present && self.date_present
    }
}

pub fn validate_file(path: &Path) -> Result<EmlValidation, Box<dyn Error>> {
    let raw = fs::read(path)?;
    validate_bytes(&raw)
        .ok_or_else(|| format!("无法解析 RFC822/MIME 邮件：{}", path.display()).into())
}

pub fn validate_bytes(raw: &[u8]) -> Option<EmlValidation> {
    if raw.is_empty() {
        return None;
    }

    let message = MessageParser::default().parse(raw)?;
    Some(EmlValidation {
        bytes: raw.len(),
        subject_present: message
            .subject()
            .is_some_and(|subject| !subject.trim().is_empty()),
        from_present: message.from().is_some(),
        date_present: message.date().is_some(),
        text_parts: message.text_body_count(),
        html_parts: message.html_body_count(),
        attachments: message.attachment_count(),
    })
}

#[cfg(test)]
mod tests {
    use super::validate_bytes;

    #[test]
    fn validates_multipart_message() {
        let raw = b"From: sender@example.com\r\n\
To: receiver@example.com\r\n\
Date: Mon, 17 Aug 2026 09:00:00 +0800\r\n\
Subject: Backup validation\r\n\
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
--foxmail-boundary--\r\n";

        let report = validate_bytes(raw).expect("message should parse");
        assert!(report.has_required_headers());
        assert_eq!(report.bytes, raw.len());
        assert_eq!(report.text_parts, 1);
        assert_eq!(report.attachments, 1);
    }

    #[test]
    fn rejects_empty_input() {
        assert!(validate_bytes(b"").is_none());
    }

    #[test]
    fn decodes_gbk_encoded_subject() {
        let raw = b"From: sender@example.com\r\n\
To: receiver@example.com\r\n\
Date: Mon, 17 Aug 2026 09:00:00 +0800\r\n\
Subject: =?GBK?B?suLK1A==?=\r\n\
MIME-Version: 1.0\r\n\
Content-Type: text/plain; charset=GBK\r\n\
Content-Transfer-Encoding: 8bit\r\n\
\r\n\
\xb2\xe2\xca\xd4\r\n";
        let message = mail_parser::MessageParser::default()
            .parse(raw.as_slice())
            .expect("GBK message should parse");
        assert_eq!(message.subject().unwrap(), "测试");
        assert_eq!(message.body_text(0).unwrap().trim(), "测试");
    }
}
