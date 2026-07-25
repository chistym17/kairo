//! Text normalization for prompt stability (Phase 13A Day 3).

use serde_json::{Map, Value};

/// Normalize text that may enter the stable prompt prefix.
///
/// - CRLF / CR → LF
/// - Trim trailing whitespace on the whole string
/// - Do **not** rewrite interior spaces (would change meaning)
pub fn normalize_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out.trim_end().to_string()
}

/// Recursively sort object keys so serialization is byte-stable.
pub fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for k in keys {
                out.insert(k.clone(), canonicalize_json(&map[k]));
            }
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(canonicalize_json).collect()),
        other => other.clone(),
    }
}

pub fn canonicalize_json_string(value: &Value) -> String {
    serde_json::to_string(&canonicalize_json(value)).unwrap_or_else(|_| "null".to_string())
}

/// Workspace-relative path string with forward slashes.
pub fn normalize_rel_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalize_text_crlf() {
        assert_eq!(normalize_text("a\r\nb\r\n"), "a\nb");
        assert_eq!(normalize_text("a\rb"), "a\nb");
    }

    #[test]
    fn canonicalize_sorts_object_keys() {
        let a = json!({"b": 1, "a": 2});
        let b = json!({"a": 2, "b": 1});
        assert_eq!(canonicalize_json_string(&a), canonicalize_json_string(&b));
        assert_eq!(canonicalize_json_string(&a), r#"{"a":2,"b":1}"#);
    }
}
