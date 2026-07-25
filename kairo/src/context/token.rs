use serde_json::Value;

/// Very rough token estimator.
///
/// This intentionally does **not** depend on any model tokenizer yet.
/// It is used only for visibility (Phase 6.1), not budgeting decisions.
pub fn estimate_tokens_from_str(s: &str) -> usize {
    // Common rule of thumb: ~4 chars per token in English-ish text.
    // Clamp to 0 for empty strings.
    if s.is_empty() {
        return 0;
    }
    (s.len() + 3) / 4
}

pub fn estimate_tokens_from_json(value: &Value) -> usize {
    match serde_json::to_string(value) {
        Ok(s) => estimate_tokens_from_str(&s),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn estimate_tokens_from_str_is_nonzero_for_small_strings() {
        assert_eq!(estimate_tokens_from_str(""), 0);
        assert_eq!(estimate_tokens_from_str("a"), 1);
        assert_eq!(estimate_tokens_from_str("abcd"), 1);
        assert_eq!(estimate_tokens_from_str("abcde"), 2);
    }

    #[test]
    fn estimate_tokens_from_json_is_nonzero() {
        let v = json!({"role":"user","content":"hello"});
        assert!(estimate_tokens_from_json(&v) > 0);
    }
}

