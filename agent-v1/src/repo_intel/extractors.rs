//! Language-limited symbol extractors (v1, hand-rolled — no regex crate).

use std::path::Path;

use super::traits::SymbolExtractor;
use super::types::{Language, SymbolKind, SymbolRecord};

/// Runs the first supporting extractor for the file language.
pub struct CompositeSymbolExtractor {
    extractors: Vec<Box<dyn SymbolExtractor>>,
}

impl Default for CompositeSymbolExtractor {
    fn default() -> Self {
        Self {
            extractors: vec![
                Box::new(RustSymbolExtractor),
                Box::new(PythonSymbolExtractor),
                Box::new(JsTsSymbolExtractor),
            ],
        }
    }
}

impl CompositeSymbolExtractor {
    pub fn extract_for_path(&self, path: &Path, content: &str) -> Vec<SymbolRecord> {
        let lang = Language::from_path(path);
        for ext in &self.extractors {
            if ext.supports(&lang) {
                return ext.extract(path, content);
            }
        }
        Vec::new()
    }
}

pub struct RustSymbolExtractor;
pub struct PythonSymbolExtractor;
pub struct JsTsSymbolExtractor;

impl SymbolExtractor for RustSymbolExtractor {
    fn supports(&self, language: &Language) -> bool {
        *language == Language::Rust
    }

    fn extract(&self, _path: &Path, content: &str) -> Vec<SymbolRecord> {
        scan_lines(content, match_rust_line)
    }
}

impl SymbolExtractor for PythonSymbolExtractor {
    fn supports(&self, language: &Language) -> bool {
        *language == Language::Python
    }

    fn extract(&self, _path: &Path, content: &str) -> Vec<SymbolRecord> {
        scan_lines(content, match_python_line)
    }
}

impl SymbolExtractor for JsTsSymbolExtractor {
    fn supports(&self, language: &Language) -> bool {
        matches!(language, Language::JavaScript | Language::TypeScript)
    }

    fn extract(&self, _path: &Path, content: &str) -> Vec<SymbolRecord> {
        scan_lines(content, match_js_line)
    }
}

fn scan_lines(content: &str, matcher: fn(&str) -> Option<(String, SymbolKind)>) -> Vec<SymbolRecord> {
    let mut out = Vec::new();
    for (line_idx, line) in content.lines().enumerate() {
        if let Some((name, kind)) = matcher(line.trim_start()) {
            out.push(SymbolRecord {
                name,
                kind,
                line: line_idx + 1,
            });
        }
    }
    out
}

fn match_rust_line(trimmed: &str) -> Option<(String, SymbolKind)> {
    if let Some(name) = strip_kw_ident(trimmed, &["pub async fn ", "pub fn ", "async fn ", "fn "]) {
        return Some((name, SymbolKind::Function));
    }
    if let Some(name) = strip_pub_item(trimmed, "struct ") {
        return Some((name, SymbolKind::Struct));
    }
    if let Some(name) = strip_pub_item(trimmed, "enum ") {
        return Some((name, SymbolKind::Enum));
    }
    if let Some(name) = strip_pub_item(trimmed, "trait ") {
        return Some((name, SymbolKind::Trait));
    }
    if let Some(name) = strip_pub_item(trimmed, "type ") {
        return Some((name, SymbolKind::Type));
    }
    if let Some(name) = strip_pub_item(trimmed, "const ") {
        return Some((name, SymbolKind::Const));
    }
    None
}

fn match_python_line(trimmed: &str) -> Option<(String, SymbolKind)> {
    if let Some(name) = strip_kw_ident(trimmed, &["async def ", "def "]) {
        return Some((name, SymbolKind::Function));
    }
    if let Some(name) = strip_kw_ident(trimmed, &["class "]) {
        return Some((name, SymbolKind::Class));
    }
    None
}

fn match_js_line(trimmed: &str) -> Option<(String, SymbolKind)> {
    if let Some(name) = strip_kw_ident(
        trimmed,
        &[
            "export async function ",
            "export function ",
            "async function ",
            "function ",
        ],
    ) {
        return Some((name, SymbolKind::Function));
    }
    if let Some(name) = strip_kw_ident(trimmed, &["export class ", "class "]) {
        return Some((name, SymbolKind::Class));
    }
    if let Some(name) =
        strip_kw_ident(trimmed, &["export interface ", "interface ", "export type ", "type "])
    {
        return Some((name, SymbolKind::Type));
    }
    if let Some(name) = js_const_function(trimmed) {
        return Some((name, SymbolKind::Function));
    }
    None
}

fn strip_pub_item(trimmed: &str, keyword: &str) -> Option<String> {
    let rest = if let Some(r) = trimmed.strip_prefix("pub(") {
        let after = r.find(')')?.checked_add(1)?;
        let r = r.get(after..)?.trim_start();
        r.strip_prefix(keyword)?
    } else if let Some(r) = trimmed.strip_prefix("pub ") {
        r.strip_prefix(keyword)?
    } else {
        trimmed.strip_prefix(keyword)?
    };
    take_ident(rest)
}

fn strip_kw_ident(trimmed: &str, prefixes: &[&str]) -> Option<String> {
    for p in prefixes {
        if let Some(rest) = trimmed.strip_prefix(p) {
            return take_ident(rest);
        }
    }
    None
}

fn js_const_function(trimmed: &str) -> Option<String> {
    let rest = trimmed
        .strip_prefix("export const ")
        .or_else(|| trimmed.strip_prefix("export let "))
        .or_else(|| trimmed.strip_prefix("export var "))
        .or_else(|| trimmed.strip_prefix("const "))
        .or_else(|| trimmed.strip_prefix("let "))
        .or_else(|| trimmed.strip_prefix("var "))?;
    let name = take_ident(rest)?;
    let after = rest.get(name.len()..)?.trim_start().strip_prefix('=')?.trim_start();
    let is_fn = after.starts_with('(')
        || after.starts_with("async")
        || after.contains("=>");
    if is_fn {
        Some(name)
    } else {
        None
    }
}

fn take_ident(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    if !(bytes[0].is_ascii_alphabetic() || bytes[0] == b'_') {
        return None;
    }
    let mut i = 1;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    Some(s[..i].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_rust_and_ts() {
        let rust = CompositeSymbolExtractor::default().extract_for_path(
            Path::new("lib.rs"),
            "pub fn move_task() {}\nstruct Task {}\n",
        );
        assert!(rust.iter().any(|s| s.name == "move_task"));
        assert!(rust.iter().any(|s| s.name == "Task"));

        let ts = CompositeSymbolExtractor::default().extract_for_path(
            Path::new("api.ts"),
            "export async function moveTask() {}\nexport const getTasks = async () => {}\n",
        );
        assert!(ts.iter().any(|s| s.name == "moveTask"));
        assert!(ts.iter().any(|s| s.name == "getTasks"));
    }
}
