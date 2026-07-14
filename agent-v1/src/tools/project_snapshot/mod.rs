mod detect;
mod types;

#[cfg(test)]
mod tests;

pub use detect::{detect, MAX_COMMANDS, MAX_ENTRY_POINTS, MAX_UNITS};
pub use types::{CommandHint, EntryPoint, ProjectKind, ProjectSnapshot, ProjectUnit};

use std::path::{Path, PathBuf};

use crate::tools::ToolResult;

/// Tool entry: detect project units under the workspace (optional path filters units).
pub fn execute(path: &Path, workspace_root: &Path) -> ToolResult {
    if !workspace_root.exists() {
        return ToolResult::error(format!(
            "workspace_root does not exist: {}",
            workspace_root.display()
        ));
    }

    let rel = if path.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        path.to_path_buf()
    };

    if rel != Path::new(".") {
        let scoped = workspace_root.join(&rel);
        if !scoped.exists() {
            return ToolResult::error(format!(
                "path not found: {} (under workspace {})",
                rel.display(),
                workspace_root.display()
            ));
        }
        if !scoped.is_dir() {
            return ToolResult::error(format!("path is not a directory: {}", rel.display()));
        }
    }

    let mut snapshot = detect(workspace_root);
    if rel != Path::new(".") {
        snapshot.units.retain(|u| {
            u.path == rel || u.path.starts_with(&rel)
        });
    }

    ToolResult {
        success: true,
        output: snapshot.render_compact(),
        diff: None,
        error: None,
        stop_reason: None,
    }
}

impl ProjectSnapshot {
    /// Compact text for tool output / context injection.
    pub fn render_compact(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "Project snapshot ({} unit{}){}\n",
            self.units.len(),
            if self.units.len() == 1 { "" } else { "s" },
            if self.truncated {
                " [truncated]"
            } else {
                ""
            }
        ));

        if self.units.is_empty() {
            out.push_str("(no project units detected)\n");
            return out;
        }

        for (i, u) in self.units.iter().enumerate() {
            out.push_str(&format!("\n{}. {}\n", i + 1, u.path.display()));
            let kind = u
                .kind
                .map(|k| k.as_str())
                .unwrap_or("unknown");
            out.push_str(&format!(
                "   Role: {}{}\n",
                kind,
                u.framework
                    .as_ref()
                    .map(|f| format!(" / {f}"))
                    .unwrap_or_default()
            ));
            out.push_str(&format!("   Confidence: {:.2}\n", u.confidence));
            if !u.evidence.is_empty() {
                out.push_str("   Evidence:\n");
                for e in &u.evidence {
                    out.push_str(&format!("   - {e}\n"));
                }
            }
            if !u.commands.is_empty() {
                out.push_str("   Commands:\n");
                for c in &u.commands {
                    let args = c.args.join(" ");
                    out.push_str(&format!(
                        "   - {}: {} {}  (cwd: {})\n",
                        c.label,
                        c.program,
                        args,
                        c.working_directory.display()
                    ));
                }
            }
            if !u.entry_points.is_empty() {
                out.push_str("   Likely entry points:\n");
                for ep in &u.entry_points {
                    let why = ep.evidence.first().map(|s| s.as_str()).unwrap_or("");
                    if why.is_empty() {
                        out.push_str(&format!(
                            "   - {} (score {})\n",
                            ep.path.display(),
                            ep.score
                        ));
                    } else {
                        out.push_str(&format!(
                            "   - {} (score {}, {})\n",
                            ep.path.display(),
                            ep.score,
                            why
                        ));
                    }
                }
            }
        }

        out
    }
}
