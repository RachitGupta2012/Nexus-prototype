//! Narrow JSON field reader used by the generated MCP test fixture.
//!
//! It understands a JSON object and returns the raw spelling of a top-level
//! value. Strings, nested objects, and nested arrays are skipped structurally,
//! so fields inside strings or nested values cannot be mistaken for top-level
//! fields. It is deliberately not a general JSON parser: it does not validate
//! the complete document, and it rejects surrogate-pair `\u` escapes.

pub(crate) fn object_field<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let bytes = json.as_bytes();
    let mut index = skip_whitespace(bytes, 0);
    if bytes.get(index) != Some(&b'{') {
        return None;
    }
    index += 1;
    loop {
        index = skip_whitespace(bytes, index);
        if bytes.get(index) == Some(&b'}') {
            return None;
        }
        let (name, next) = parse_string(json, index)?;
        index = skip_whitespace(bytes, next);
        if bytes.get(index) != Some(&b':') {
            return None;
        }
        index = skip_whitespace(bytes, index + 1);
        let end = skip_value(json, index)?;
        if name == key {
            return Some(&json[index..end]);
        }
        index = skip_whitespace(bytes, end);
        match bytes.get(index) {
            Some(b',') => index += 1,
            Some(b'}') => return None,
            _ => return None,
        }
    }
}

pub(crate) fn string_value(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let start = skip_whitespace(bytes, 0);
    let (value, end) = parse_string(raw, start)?;
    (skip_whitespace(bytes, end) == bytes.len()).then_some(value)
}

fn skip_whitespace(bytes: &[u8], mut index: usize) -> usize {
    while matches!(
        bytes.get(index).copied(),
        Some(b' ' | b'\n' | b'\r' | b'\t')
    ) {
        index += 1;
    }
    index
}

fn parse_string(json: &str, start: usize) -> Option<(String, usize)> {
    let bytes = json.as_bytes();
    if bytes.get(start) != Some(&b'"') {
        return None;
    }
    let mut value = String::new();
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => return Some((value, index + 1)),
            b'\\' => {
                index += 1;
                match bytes.get(index).copied()? {
                    b'"' => value.push('"'),
                    b'\\' => value.push('\\'),
                    b'/' => value.push('/'),
                    b'b' => value.push('\u{0008}'),
                    b'f' => value.push('\u{000c}'),
                    b'n' => value.push('\n'),
                    b'r' => value.push('\r'),
                    b't' => value.push('\t'),
                    b'u' => {
                        let hex = json.get(index + 1..index + 5)?;
                        let code = u16::from_str_radix(hex, 16).ok()?;
                        value.push(char::from_u32(u32::from(code))?);
                        index += 4;
                    }
                    _ => return None,
                }
            }
            _ => {
                let character = json[index..].chars().next()?;
                value.push(character);
                index += character.len_utf8() - 1;
            }
        }
        index += 1;
    }
    None
}

fn skip_value(json: &str, start: usize) -> Option<usize> {
    let bytes = json.as_bytes();
    let start = skip_whitespace(bytes, start);
    match bytes.get(start).copied()? {
        b'"' => parse_string(json, start).map(|(_, end)| end),
        b'{' => skip_container(bytes, start, b'{', b'}'),
        b'[' => skip_container(bytes, start, b'[', b']'),
        b't' => skip_literal(bytes, start, b"true"),
        b'f' => skip_literal(bytes, start, b"false"),
        b'n' => skip_literal(bytes, start, b"null"),
        b'-' | b'0'..=b'9' => skip_number(bytes, start),
        _ => None,
    }
}

fn skip_container(bytes: &[u8], start: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    let mut index = start;
    let mut in_string = false;
    let mut escaped = false;
    while let Some(byte) = bytes.get(index).copied() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else if byte == b'"' {
            in_string = true;
        } else if byte == open {
            depth += 1;
        } else if byte == close {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index + 1);
            }
        }
        index += 1;
    }
    None
}

fn skip_literal(bytes: &[u8], start: usize, literal: &[u8]) -> Option<usize> {
    let end = start.checked_add(literal.len())?;
    bytes.get(start..end)?.eq(literal).then_some(end)
}

fn skip_number(bytes: &[u8], start: usize) -> Option<usize> {
    let mut index = start;
    while matches!(
        bytes.get(index).copied(),
        Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
    ) {
        index += 1;
    }
    (index > start).then_some(index)
}
