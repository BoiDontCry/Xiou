use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModifiedUtf7Error(String);

impl fmt::Display for ModifiedUtf7Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ModifiedUtf7Error {}

pub fn decode(input: &str) -> Result<String, ModifiedUtf7Error> {
    if !input.is_ascii() {
        return Err(ModifiedUtf7Error(
            "Modified UTF-7 输入必须是 ASCII".to_owned(),
        ));
    }

    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut position = 0;

    while position < bytes.len() {
        if bytes[position] != b'&' {
            output.push(bytes[position] as char);
            position += 1;
            continue;
        }

        let relative_end = bytes[position + 1..]
            .iter()
            .position(|byte| *byte == b'-')
            .ok_or_else(|| ModifiedUtf7Error("Modified UTF-7 移位段缺少 '-' 终止符".to_owned()))?;
        let end = position + 1 + relative_end;
        let encoded = &input[position + 1..end];

        if encoded.is_empty() {
            output.push('&');
        } else {
            let decoded = decode_modified_base64(encoded)?;
            if decoded.len() % 2 != 0 {
                return Err(ModifiedUtf7Error(
                    "Modified UTF-7 UTF-16BE 字节数不是偶数".to_owned(),
                ));
            }
            let utf16 = decoded
                .chunks_exact(2)
                .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
                .collect::<Vec<_>>();
            let shifted = String::from_utf16(&utf16).map_err(|error| {
                ModifiedUtf7Error(format!("Modified UTF-7 包含无效 UTF-16：{error}"))
            })?;
            output.push_str(&shifted);
        }

        position = end + 1;
    }

    Ok(output)
}

fn decode_modified_base64(input: &str) -> Result<Vec<u8>, ModifiedUtf7Error> {
    if input.len() % 4 == 1 {
        return Err(ModifiedUtf7Error(
            "Modified UTF-7 Base64 长度无效".to_owned(),
        ));
    }

    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut accumulator = 0u32;
    let mut bits = 0u8;

    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b',' => 63,
            _ => {
                return Err(ModifiedUtf7Error(format!(
                    "Modified UTF-7 Base64 包含非法字符：{}",
                    byte as char
                )))
            }
        };

        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        while bits >= 8 {
            bits -= 8;
            output.push((accumulator >> bits) as u8);
            accumulator &= if bits == 0 { 0 } else { (1u32 << bits) - 1 };
        }
    }

    if bits > 0 && accumulator != 0 {
        return Err(ModifiedUtf7Error(
            "Modified UTF-7 Base64 末尾填充位无效".to_owned(),
        ));
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::decode;

    #[test]
    fn decodes_rfc_3501_example() {
        assert_eq!(
            decode("~peter/mail/&U,BTFw-/&ZeVnLIqe-").unwrap(),
            "~peter/mail/台北/日本語"
        );
    }

    #[test]
    fn decodes_ampersand_and_ascii() {
        assert_eq!(decode("Sales &- Support").unwrap(), "Sales & Support");
        assert_eq!(decode("INBOX").unwrap(), "INBOX");
    }

    #[test]
    fn rejects_unterminated_shift_sequence() {
        assert!(decode("broken&U,BTFw").is_err());
    }
}
