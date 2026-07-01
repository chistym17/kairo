use similar::{ChangeTag, TextDiff};

pub fn generate(old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }

    let diff = TextDiff::from_lines(old, new);
    let mut output = String::new();

    for change in diff.iter_all_changes() {
        let prefix = match change.tag() {
            ChangeTag::Delete => "-",
            ChangeTag::Insert => "+",
            ChangeTag::Equal => " ",
        };
        output.push_str(&format!("{}{}", prefix, change));
    }

    output
}
