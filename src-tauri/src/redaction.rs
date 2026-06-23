use once_cell::sync::Lazy;
use regex::Regex;

static SECRET_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| vec![
    Regex::new(r#"(?i)(api[_-]?key|token|secret|password|passwd)\s*[:=]\s*['"]?[^\s'"]{8,}"#).unwrap(),
    Regex::new(r"sk-[A-Za-z0-9_\-]{16,}").unwrap(),
    Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}").unwrap(),
]);

pub fn redact(s: &str) -> String {
    let mut out = s.to_string();
    for re in SECRET_PATTERNS.iter() {
        out = re
            .replace_all(&out, |caps: &regex::Captures| {
                if caps.len() > 1 {
                    format!("{}=<REDACTED>", &caps[1])
                } else {
                    "<REDACTED>".to_string()
                }
            })
            .to_string();
    }
    out
}

pub fn excerpt(s: &str, max: usize) -> String {
    let r = redact(s);
    if r.chars().count() <= max {
        r
    } else {
        let mut x = r.chars().take(max.saturating_sub(1)).collect::<String>();
        x.push('…');
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redacts_key_and_email() {
        let r = redact("api_key='sk-abcdef1234567890' me@example.com");
        assert!(!r.contains("abcdef"));
        assert!(!r.contains("example.com"));
    }
}
