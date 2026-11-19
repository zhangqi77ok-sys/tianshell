use axum::http::{HeaderMap, HeaderValue, header};

#[derive(Debug)]
pub enum AuthError {
    Forbidden(&'static str),
    Unauthorized(&'static str),
}

impl AuthError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::Forbidden(message) | Self::Unauthorized(message) => message,
        }
    }

    pub fn status_code(&self) -> u16 {
        match self {
            Self::Forbidden(_) => 403,
            Self::Unauthorized(_) => 401,
        }
    }
}

pub fn authorize(headers: &HeaderMap, query: Option<&str>, token: &str) -> Result<(), AuthError> {
    if !host_is_local(headers) {
        return Err(AuthError::Forbidden("仅允许本机访问"));
    }
    if !origin_ok(headers) {
        return Err(AuthError::Forbidden("拒绝跨站来源"));
    }
    if !token_matches(headers, query, token) {
        return Err(AuthError::Unauthorized("未授权的本地访问令牌"));
    }
    Ok(())
}

pub fn token_matches(headers: &HeaderMap, query: Option<&str>, token: &str) -> bool {
    if header_eq(headers, "x-tianshell-token", token) {
        return true;
    }
    if let Some(value) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        if let Some(rest) = value.strip_prefix("Bearer ") {
            if rest.trim() == token {
                return true;
            }
        }
    }
    query_param(query, "token").as_deref() == Some(token)
}

pub fn host_is_local(headers: &HeaderMap) -> bool {
    let Some(value) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    let host = value.split(':').next().unwrap_or(value).trim();
    host.eq_ignore_ascii_case("127.0.0.1") || host.eq_ignore_ascii_case("localhost") || host == "[::1]" || host == "::1"
}

pub fn origin_ok(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    let origin = origin.trim();
    origin.starts_with("http://127.0.0.1")
        || origin.starts_with("http://localhost")
        || origin.starts_with("http://[::1]")
        || origin == "null"
}

fn header_eq(headers: &HeaderMap, name: &str, expected: &str) -> bool {
    headers.get(name).and_then(|value: &HeaderValue| value.to_str().ok()) == Some(expected)
}

pub fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    let query = query?;
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let name = parts.next().unwrap_or("");
        if name == key {
            let raw = parts.next().unwrap_or("");
            return Some(percent_decode(raw));
        }
    }
    None
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(value) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
