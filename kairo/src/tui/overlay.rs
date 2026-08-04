//! Modal overlays: approval gate, diff viewer, help.

use crate::api::ApprovalRequest;

#[derive(Debug, Clone)]
pub enum Overlay {
    Approval(ApprovalView),
    Diff(DiffView),
    Help,
}

#[derive(Debug, Clone)]
pub struct ApprovalView {
    pub request: ApprovalRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Add,
    Delete,
    Meta,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct DiffView {
    pub title: String,
    pub lines: Vec<DiffLine>,
    pub scroll: u16,
    /// When set, Esc returns to this approval instead of closing fully.
    pub return_to_approval: Option<ApprovalRequest>,
}

impl DiffView {
    pub fn from_text(title: impl Into<String>, body: &str) -> Self {
        let lines = parse_diff_lines(body);
        Self {
            title: title.into(),
            lines,
            scroll: 0,
            return_to_approval: None,
        }
    }

    pub fn from_approval_fallback(request: &ApprovalRequest) -> Self {
        let path = request
            .paths
            .first()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(unknown path)".into());
        let mut lines = vec![
            DiffLine {
                kind: DiffLineKind::Meta,
                text: "Proposed change (patch body not available before approval)".into(),
            },
            DiffLine {
                kind: DiffLineKind::Meta,
                text: String::new(),
            },
            DiffLine {
                kind: DiffLineKind::Meta,
                text: format!("Summary: {}", request.summary),
            },
            DiffLine {
                kind: DiffLineKind::Meta,
                text: format!("Reason:  {}", request.reason),
            },
            DiffLine {
                kind: DiffLineKind::Meta,
                text: String::new(),
            },
            DiffLine {
                kind: DiffLineKind::Meta,
                text: "Paths:".into(),
            },
        ];
        if request.paths.is_empty() {
            lines.push(DiffLine {
                kind: DiffLineKind::Context,
                text: format!("  {path}"),
            });
        } else {
            for p in &request.paths {
                lines.push(DiffLine {
                    kind: DiffLineKind::Context,
                    text: format!("  {}", p.display()),
                });
            }
        }
        Self {
            title: format!("Diff: {path}"),
            lines,
            scroll: 0,
            return_to_approval: Some(request.clone()),
        }
    }

    pub fn scroll_by(&mut self, delta: i32, viewport: u16) {
        let max = self
            .lines
            .len()
            .saturating_sub(viewport as usize)
            .max(0) as u16;
        let next = self.scroll as i32 + delta;
        self.scroll = next.clamp(0, max as i32) as u16;
    }
}

pub fn parse_diff_lines(body: &str) -> Vec<DiffLine> {
    if body.trim().is_empty() {
        return vec![DiffLine {
            kind: DiffLineKind::Meta,
            text: "(no diff content)".into(),
        }];
    }
    body.lines()
        .map(|line| {
            let kind = if line.starts_with("+++") || line.starts_with("---") || line.starts_with("diff ")
            {
                DiffLineKind::Meta
            } else if line.starts_with('+') {
                DiffLineKind::Add
            } else if line.starts_with('-') {
                DiffLineKind::Delete
            } else {
                DiffLineKind::Context
            };
            DiffLine {
                kind,
                text: line.to_string(),
            }
        })
        .collect()
}

/// Load `git diff HEAD -- path` from a workspace, if available.
pub fn load_git_diff(workspace: &std::path::Path, rel_path: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["-C", &workspace.to_string_lossy(), "diff", "HEAD", "--", rel_path])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    if text.trim().is_empty() {
        // Untracked or already committed — try staged+unstaged name-only hint.
        return None;
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_add_delete() {
        let lines = parse_diff_lines("-old\n+new\n context\n");
        assert_eq!(lines[0].kind, DiffLineKind::Delete);
        assert_eq!(lines[1].kind, DiffLineKind::Add);
        assert_eq!(lines[2].kind, DiffLineKind::Context);
    }

    #[test]
    fn diff_scroll_clamps() {
        let mut view = DiffView::from_text(
            "f",
            &(0..20).map(|i| format!(" line {i}")).collect::<Vec<_>>().join("\n"),
        );
        view.scroll_by(100, 5);
        assert!(view.scroll <= 15);
        view.scroll_by(-100, 5);
        assert_eq!(view.scroll, 0);
    }
}
