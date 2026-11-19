//! Strip obvious secrets before they reach MCP results, history, or notes.
//!
//! This is a filter, not a vault. It catches private keys, bearer tokens, and
//! flags/assignments whose names say they are secrets. Ordinary text is left alone.

const SECRET_NAME_PARTS: &[&str] = &[
    "PASSWORD",
    "PASSWD",
    "PASSPHRASE",
    "SECRET",
    "TOKEN",
    "API_KEY",
    "APIKEY",
    "ACCESS_KEY",
    "PRIVATE_KEY",
    "CLIENT_SECRET",
];

const SECRET_FLAGS: &[&str] = &[
    "--password",
    "--passwd",
    "--passphrase",
    "--token",
    "--secret",
    "--api-key",
    "--apikey",
    "--api_key",
    "--access-token",
    "--auth-token",
    "--mcp-token",
    "--bearer",
];

pub fn redact_secrets(input: &str, extra: &[&str]) -> String {
    let mut text = redact_pem(input);
    text = redact_urls(&text);
    text = redact_bearer(&text);
    text = redact_assignments(&text);
    text = redact_flags(&text);
    text = redact_sshpass(&text);
    for secret in extra {
        let secret = secret.trim();
        if secret.len() >= 8 && text.contains(secret) {
            text = text.replace(secret, "[redacted]");
        }
    }
    text
}

pub fn contains_secret(input: &str) -> bool {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.contains("PRIVATE KEY") || trimmed.contains("-----BEGIN") {
        return true;
    }
    redact_secrets(trimmed, &[]) != trimmed
}

fn redact_pem(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("-----BEGIN") {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        if let Some(end_rel) = after.find("-----END") {
            let tail = &after[end_rel..];
            if let Some(line_end) = tail.find('\n') {
                rest = &tail[line_end + 1..];
            } else {
                rest = "";
            }
            out.push_str("[redacted]");
        } else {
            out.push_str("[redacted]");
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

fn redact_urls(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"://") {
            out.push_str("://");
            i += 3;
            let start = i;
            let end = bytes[i..]
                .iter()
                .position(|b| matches!(b, b'/' | b'?' | b'#' | b' ' | b'\t' | b'\n' | b'\r' | b'"' | b'\''))
                .map(|n| i + n)
                .unwrap_or(bytes.len());
            let auth = &input[start..end];
            if let Some(at) = auth.rfind('@') {
                if auth[..at].contains(':') {
                    out.push_str("[redacted]@");
                    out.push_str(&auth[at + 1..]);
                    i = end;
                    continue;
                }
            }
            out.push_str(auth);
            i = end;
            continue;
        }
        let ch = input[i..].chars().next().unwrap_or('\0');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn redact_bearer(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let lower = input.to_ascii_lowercase();
    let mut i = 0;
    let bytes = input.as_bytes();
    while i < bytes.len() {
        let rest_lower = &lower[i..];
        if rest_lower.starts_with("bearer ") {
            out.push_str(&input[i..i + "bearer ".len()]);
            i += "bearer ".len();
            let value_end = end_of_token(input, i);
            if value_end > i {
                out.push_str("[redacted]");
                i = value_end;
                continue;
            }
        }
        let ch = input[i..].chars().next().unwrap_or('\0');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn redact_assignments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '=' && i > 0 {
            let mut name_start = i;
            while name_start > 0 && is_name_char(chars[name_start - 1]) {
                name_start -= 1;
            }
            let name: String = chars[name_start..i].iter().collect();
            if !name.is_empty() && name_is_secret(&name) {
                let value_start = i + 1;
                let value_end = end_of_assignment_value(&chars, value_start);
                if value_end > value_start {
                    out.push('=');
                    out.push_str("[redacted]");
                    i = value_end;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn redact_flags(input: &str) -> String {
    let mut text = input.to_string();
    for flag in SECRET_FLAGS {
        text = replace_flag(&text, flag);
    }
    text
}

fn replace_flag(input: &str, flag: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let flag_lower = flag.to_ascii_lowercase();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if lower[i..].starts_with(&flag_lower) {
            let after = i + flag.len();
            let boundary_ok = i == 0 || !is_name_byte(input.as_bytes()[i - 1]);
            if boundary_ok && after <= input.len() {
                let rest = &input[after..];
                if let Some(stripped) = rest.strip_prefix('=') {
                    let value_end = end_of_token(stripped, 0);
                    if value_end > 0 {
                        out.push_str(&input[i..after]);
                        out.push('=');
                        out.push_str("[redacted]");
                        i = after + 1 + value_end;
                        continue;
                    }
                } else if rest.starts_with(|c: char| c.is_whitespace()) {
                    let trimmed = rest.trim_start();
                    let spaces = rest.len() - trimmed.len();
                    let value_end = end_of_token(trimmed, 0);
                    if value_end > 0 && !trimmed[..value_end].starts_with('-') {
                        out.push_str(&input[i..after]);
                        out.push_str(&rest[..spaces]);
                        out.push_str("[redacted]");
                        i = after + spaces + value_end;
                        continue;
                    }
                }
            }
        }
        let ch = input[i..].chars().next().unwrap_or('\0');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn redact_sshpass(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if lower[i..].starts_with("sshpass") {
            let after = i + "sshpass".len();
            let boundary = i == 0 || !is_name_byte(input.as_bytes()[i - 1]);
            if boundary {
                if let Some(rel) = lower[after..].find(" -p") {
                    let flag_at = after + rel;
                    let value_at = flag_at + 3;
                    let rest = input.get(value_at..).unwrap_or("");
                    let trimmed = rest.trim_start();
                    let spaces = rest.len() - trimmed.len();
                    let value_end = end_of_token(trimmed, 0);
                    if value_end > 0 {
                        out.push_str(&input[i..value_at + spaces]);
                        out.push_str("[redacted]");
                        i = value_at + spaces + value_end;
                        continue;
                    }
                }
            }
        }
        let ch = input[i..].chars().next().unwrap_or('\0');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn name_is_secret(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_NAME_PARTS.iter().any(|part| upper.contains(part))
}

fn is_name_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn end_of_token(text: &str, start: usize) -> usize {
    let rest = text.get(start..).unwrap_or("");
    if rest.is_empty() {
        return start;
    }
    let bytes = rest.as_bytes();
    if bytes[0] == b'\'' || bytes[0] == b'"' {
        let quote = bytes[0];
        let mut i = 1;
        while i < bytes.len() {
            if bytes[i] == quote {
                return start + i + 1;
            }
            i += 1;
        }
        return start + rest.len();
    }
    let len = rest
        .find(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '\n' | '\r'))
        .unwrap_or(rest.len());
    start + len
}

fn end_of_assignment_value(chars: &[char], start: usize) -> usize {
    if start >= chars.len() {
        return start;
    }
    if chars[start] == '\'' || chars[start] == '"' {
        let quote = chars[start];
        let mut i = start + 1;
        while i < chars.len() {
            if chars[i] == quote {
                return i + 1;
            }
            i += 1;
        }
        return chars.len();
    }
    let mut i = start;
    while i < chars.len() && !chars[i].is_whitespace() && !matches!(chars[i], ';' | '&' | '|') {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_ordinary_commands_alone() {
        let cmd = "echo hello && psql -p 5432 -c 'select 1'";
        assert_eq!(redact_secrets(cmd, &[]), cmd);
        assert!(!contains_secret(cmd));
        assert!(!contains_secret("postgres listens on 5432; the token service is local"));
    }

    #[test]
    fn redacts_flags_assignments_bearer_urls_and_keys() {
        assert_eq!(
            redact_secrets("mysql --password=s3cret -e 'show'", &[]),
            "mysql --password=[redacted] -e 'show'"
        );
        assert_eq!(
            redact_secrets("curl --token abc.def https://example.test", &[]),
            "curl --token [redacted] https://example.test"
        );
        assert_eq!(
            redact_secrets("export AWS_SECRET_ACCESS_KEY=supersecretvalue", &[]),
            "export AWS_SECRET_ACCESS_KEY=[redacted]"
        );
        assert_eq!(
            redact_secrets("curl -H 'Authorization: Bearer abc.def.ghi'", &[]),
            "curl -H 'Authorization: Bearer [redacted]"
        );
        assert_eq!(
            redact_secrets("sshpass -p hunter2 ssh host", &[]),
            "sshpass -p [redacted] ssh host"
        );
        assert_eq!(
            redact_secrets("curl https://user:pw@example.test/x", &[]),
            "curl https://[redacted]@example.test/x"
        );
        let pem = "-----BEGIN OPENSSH PRIVATE KEY-----\naabb\n-----END OPENSSH PRIVATE KEY-----\n";
        let redacted = redact_secrets(pem, &[]);
        assert!(!redacted.contains("aabb"));
        assert!(redacted.contains("[redacted]"));
        assert!(contains_secret(pem));
        assert_eq!(redact_secrets("echo hi", &["mcp-token-value"]), "echo hi");
        assert_eq!(
            redact_secrets("see mcp-token-value now", &["mcp-token-value"]),
            "see [redacted] now"
        );
    }
}
