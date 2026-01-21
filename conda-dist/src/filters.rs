use std::fmt::Display;

use askama::{Result, Values};

#[askama::filter_fn]
pub fn rpm_escape<T: Display>(input: T, _: &dyn Values) -> Result<String> {
    Ok(input.to_string().replace('%', "%%"))
}

#[askama::filter_fn]
pub fn join_paragraphs(parts: &[String], _: &dyn Values) -> Result<String> {
    Ok(parts.join("\n\n"))
}

#[askama::filter_fn]
pub fn native_version<T: Display>(input: T, _: &dyn Values) -> Result<String> {
    let raw = input.to_string();
    let with_dev = apply_dev_marker(&raw);
    Ok(apply_prerelease_marker(&with_dev))
}

#[askama::filter_fn]
pub fn deb_description<T: Display>(parts: &[String], _: &dyn Values, summary: T) -> Result<String> {
    let mut lines = Vec::new();
    lines.push(format!("Description: {}", summary));
    if parts.is_empty() {
        lines.push(" .".to_string());
    } else {
        for (idx, part) in parts.iter().enumerate() {
            if idx > 0 {
                lines.push(" .".to_string());
            }
            for line in part.lines() {
                if line.trim().is_empty() {
                    lines.push(" .".to_string());
                } else {
                    lines.push(format!(" {line}"));
                }
            }
        }
    }
    Ok(lines.join("\n"))
}

#[askama::filter_fn]
pub fn rpm_release<T: Display>(build: T, _: &dyn Values) -> Result<String> {
    Ok(normalize_rpm_release(&build.to_string()))
}

#[askama::filter_fn]
pub fn deb_release<T: Display>(build: T, _: &dyn Values) -> Result<String> {
    Ok(normalize_deb_release(&build.to_string()))
}

pub fn normalize_rpm_release(build: &str) -> String {
    normalize_release(build, |ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '.' | '+' | '~' | '_')
    })
}

pub fn normalize_deb_release(build: &str) -> String {
    normalize_release(build, |ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '.' | '+' | '~')
    })
}

fn normalize_release(build: &str, allowed: impl Fn(char) -> bool) -> String {
    let mut out = String::new();
    for ch in build.chars() {
        if allowed(ch) {
            out.push(ch);
        } else {
            out.push('.');
        }
    }
    let trimmed = out.trim_matches('.');
    if trimmed.is_empty() {
        "1".to_string()
    } else {
        trimmed.to_string()
    }
}

fn apply_dev_marker(raw: &str) -> String {
    if let Some((start, end)) = find_marker(raw, "dev") {
        let mut out = String::with_capacity(raw.len() + 4);
        out.push_str(&raw[..start]);
        out.push_str("~~dev");
        out.push_str(&raw[end..]);
        return out;
    }
    raw.to_string()
}

fn apply_prerelease_marker(raw: &str) -> String {
    for marker in ["alpha", "beta", "preview", "pre", "rc", "a", "b"] {
        if let Some((start, end)) = find_marker(raw, marker) {
            let mut out = String::with_capacity(raw.len() + 1);
            let mut prefix = raw[..start].to_string();
            if let Some(ch) = prefix.chars().last()
                && matches!(ch, '.' | '-' | '_')
            {
                prefix.pop();
                prefix.push('~');
            } else {
                prefix.push('~');
            }
            out.push_str(&prefix);
            out.push_str(&raw[start..end]);
            out.push_str(&raw[end..]);
            return out;
        }
    }
    raw.to_string()
}

fn find_marker(raw: &str, marker: &str) -> Option<(usize, usize)> {
    let mut start_index = 0;
    while let Some(offset) = raw[start_index..].find(marker) {
        let start = start_index + offset;
        let end = start + marker.len();
        let prev = raw[..start].chars().last();
        let next = raw[end..].chars().next();
        let prev_ok = prev
            .map(|ch| ch.is_ascii_digit() || matches!(ch, '.' | '-' | '_'))
            .unwrap_or(true);
        let next_ok = next.map(|ch| ch.is_ascii_digit()).unwrap_or(true);
        if prev_ok && next_ok {
            return Some((start, end));
        }
        start_index = end;
    }
    None
}
