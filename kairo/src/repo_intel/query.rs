//! Deterministic query tokenization and naming-style variants.

const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "to", "of", "in", "on", "for", "and", "or", "is", "are", "be", "by",
    "with", "from", "that", "this", "it", "as", "at", "into", "when", "where", "which",
    "who", "whom", "what", "how", "fix", "bug", "issue", "error", "problem", "failing",
    "broken", "please", "should", "does", "not", "no", "show", "shows",
];

/// Normalized locate query terms + identifier variants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryTokens {
    pub raw: String,
    /// Significant words after stop-word removal (lowercased).
    pub terms: Vec<String>,
    /// All search keys: terms + naming variants + quoted literals.
    pub variants: Vec<String>,
}

pub fn normalize_query(query: &str) -> QueryTokens {
    let raw = query.trim().to_string();
    let mut terms = Vec::new();
    let mut variants = Vec::new();

    // Quoted strings preserved as exact variants.
    let mut rest = raw.as_str();
    let mut unquoted = String::new();
    while let Some((before, quoted, after)) = split_next_quote(rest) {
        unquoted.push_str(before);
        unquoted.push(' ');
        if !quoted.is_empty() {
            push_unique(&mut variants, quoted.to_string());
            push_unique(&mut terms, quoted.to_string());
        }
        rest = after;
    }
    unquoted.push_str(rest);

    for token in unquoted.split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-') {
        if token.is_empty() {
            continue;
        }
        // Code-looking identifiers kept as-is (plus variants).
        if looks_code_ident(token) {
            push_unique(&mut terms, token.to_string());
            for v in naming_variants(token) {
                push_unique(&mut variants, v);
            }
            continue;
        }
        let lower = token.to_ascii_lowercase();
        if STOP_WORDS.contains(&lower.as_str()) || lower.len() < 2 {
            continue;
        }
        push_unique(&mut terms, lower.clone());
        for v in naming_variants(&lower) {
            push_unique(&mut variants, v);
        }
    }

    // Phrase-level camel/snake from consecutive terms (e.g. "due date" → dueDate).
    if terms.len() >= 2 {
        for window in terms.windows(2) {
            let pair = window.to_vec();
            for v in [
                pair.join("_"),
                pair.join("-"),
                to_camel_case(&pair),
                to_pascal_case(&pair),
            ] {
                push_unique(&mut variants, v);
            }
        }
        let joined_snake = terms.join("_");
        let joined_camel = to_camel_case(&terms);
        let joined_pascal = to_pascal_case(&terms);
        let joined_kebab = terms.join("-");
        for v in [joined_snake, joined_camel, joined_pascal, joined_kebab] {
            push_unique(&mut variants, v);
        }
    }

    if variants.is_empty() && !raw.is_empty() {
        push_unique(&mut variants, raw.clone());
    }

    QueryTokens {
        raw,
        terms,
        variants,
    }
}

fn split_next_quote(s: &str) -> Option<(&str, &str, &str)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\'' || bytes[i] == b'"' {
            let q = bytes[i];
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != q {
                j += 1;
            }
            if j < bytes.len() {
                return Some((&s[..i], &s[start..j], &s[j + 1..]));
            }
            return None;
        }
        i += 1;
    }
    None
}

fn looks_code_ident(token: &str) -> bool {
    token.contains('_')
        || token.contains('-')
        || token.chars().any(|c| c.is_ascii_uppercase())
        || token.chars().any(|c| c.is_ascii_digit())
}

pub fn naming_variants(token: &str) -> Vec<String> {
    let mut out = Vec::new();
    push_unique(&mut out, token.to_string());
    push_unique(&mut out, token.to_ascii_lowercase());

    let parts = split_ident_parts(token);
    if parts.len() > 1 {
        push_unique(&mut out, parts.join("_"));
        push_unique(&mut out, parts.join("-"));
        push_unique(&mut out, to_camel_case(&parts));
        push_unique(&mut out, to_pascal_case(&parts));
    } else if !token.is_empty() {
        // single word: still offer PascalCase
        let mut pascal = token.to_ascii_lowercase();
        if let Some(first) = pascal.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        push_unique(&mut out, pascal);
    }
    out
}

fn split_ident_parts(name: &str) -> Vec<String> {
    let mut parts = Vec::new();
    for chunk in name.split(|c: char| c == '_' || c == '-') {
        if chunk.is_empty() {
            continue;
        }
        let mut current = String::new();
        for ch in chunk.chars() {
            if ch.is_uppercase() && !current.is_empty() {
                parts.push(current.to_ascii_lowercase());
                current.clear();
            }
            current.push(ch);
        }
        if !current.is_empty() {
            parts.push(current.to_ascii_lowercase());
        }
    }
    parts
}

fn to_camel_case(parts: &[String]) -> String {
    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i == 0 {
            out.push_str(&p.to_ascii_lowercase());
        } else {
            out.push_str(&to_pascal_word(p));
        }
    }
    out
}

fn to_pascal_case(parts: &[String]) -> String {
    parts.iter().map(|p| to_pascal_word(p)).collect()
}

fn to_pascal_word(p: &str) -> String {
    let lower = p.to_ascii_lowercase();
    let mut chars = lower.chars();
    match chars.next() {
        Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

fn push_unique(out: &mut Vec<String>, s: String) {
    if s.is_empty() {
        return;
    }
    if !out.iter().any(|x| x == &s) {
        out.push(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_date_variants() {
        let q = normalize_query("fix due date mismatch");
        assert!(q.terms.iter().any(|t| t == "due"));
        assert!(q.terms.iter().any(|t| t == "date"));
        assert!(q.variants.iter().any(|v| v == "dueDate"));
        assert!(q.variants.iter().any(|v| v == "due_date"));
        assert!(q.variants.iter().any(|v| v == "DueDate"));
    }

    #[test]
    fn preserves_quoted_and_code_idents() {
        let q = normalize_query("find `moveTask` and \"due_date\"");
        // backticks not quotes — moveTask still via code-looking path if present
        let q2 = normalize_query("find moveTask and \"due_date\"");
        assert!(q2.variants.iter().any(|v| v == "moveTask"));
        assert!(q2.variants.iter().any(|v| v == "due_date"));
        let _ = q;
    }
}
