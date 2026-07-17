//! Objective verification discovery and outcome recording.
//!
//! The model remains responsible for choosing when and what to run. This module
//! only identifies commands that match checks declared by the repository's
//! project snapshot and turns their tool results into structured outcomes.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::tools::project_snapshot::ProjectSnapshot;
use crate::tools::ToolResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    Test,
    Build,
    Lint,
    Typecheck,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Check {
    pub id: String,
    pub kind: CheckKind,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub confidence: f32,
}

impl Check {
    pub fn command(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationOutcome {
    pub check_id: String,
    pub kind: CheckKind,
    pub passed: bool,
    pub exit_code: Option<i32>,
    pub summary: String,
}

/// Convert repository command hints into stable, typed verification checks.
pub fn discover_checks(snapshot: &ProjectSnapshot) -> Vec<Check> {
    let mut checks = Vec::new();

    for unit in &snapshot.units {
        for hint in &unit.commands {
            let kind = check_kind(&hint.label);
            if kind == CheckKind::Other {
                continue;
            }

            let unit_id = if unit.path == Path::new(".") {
                "root".to_string()
            } else {
                unit.path.to_string_lossy().replace(['/', '\\'], "-")
            };
            let label = hint
                .label
                .to_ascii_lowercase()
                .replace(|c: char| !c.is_ascii_alphanumeric(), "-");

            checks.push(Check {
                id: format!("{unit_id}:{label}"),
                kind,
                program: hint.program.clone(),
                args: hint.args.clone(),
                cwd: normalize_path(&hint.working_directory),
                confidence: hint.confidence,
            });
        }
    }

    checks.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.id.cmp(&b.id))
    });
    checks.dedup_by(|a, b| a.command() == b.command() && a.cwd == b.cwd);
    checks
}

/// Match a command execution to a discovered check.
///
/// Handles direct commands and simple `cd dir && command` shell sequences.
/// Additional arguments are accepted so focused test targets still match the
/// discovered base check (for example `manage.py test tasks`).
pub fn match_executed_check<'a>(
    command: &str,
    action_cwd: &Path,
    checks: &'a [Check],
) -> Option<&'a Check> {
    let mut effective_cwd = normalize_path(action_cwd);

    for raw_segment in command.split([';', '\n']) {
        for segment in raw_segment.split("&&") {
            let segment = normalize_command(segment);
            if segment.is_empty() {
                continue;
            }

            if let Some(dir) = segment.strip_prefix("cd ") {
                effective_cwd = join_normalized(&effective_cwd, Path::new(trim_shell_quotes(dir)));
                continue;
            }

            if let Some(found) = checks.iter().find(|check| {
                check.cwd == effective_cwd && command_matches(&segment, &check.command())
            }) {
                return Some(found);
            }
        }
    }

    None
}

pub fn outcome_for(check: &Check, result: &ToolResult) -> VerificationOutcome {
    VerificationOutcome {
        check_id: check.id.clone(),
        kind: check.kind,
        passed: result.success,
        exit_code: parse_exit_code(&result.output),
        summary: summarize_result(result),
    }
}

fn check_kind(label: &str) -> CheckKind {
    match label.to_ascii_lowercase().as_str() {
        "test" | "tests" | "pytest" => CheckKind::Test,
        "build" => CheckKind::Build,
        "lint" | "clippy" => CheckKind::Lint,
        "typecheck" | "type-check" | "check" => CheckKind::Typecheck,
        _ => CheckKind::Other,
    }
}

fn command_matches(actual: &str, expected: &str) -> bool {
    actual == expected
        || actual
            .strip_prefix(expected)
            .is_some_and(|rest| rest.starts_with(char::is_whitespace))
}

fn normalize_command(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn trim_shell_quotes(value: &str) -> &str {
    value.trim().trim_matches(['\'', '"'])
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

fn join_normalized(base: &Path, child: &Path) -> PathBuf {
    if base == Path::new(".") {
        normalize_path(child)
    } else {
        normalize_path(&base.join(child))
    }
}

fn parse_exit_code(output: &str) -> Option<i32> {
    output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("exit_code:")
            .and_then(|value| value.trim().parse().ok())
    })
}

fn summarize_result(result: &ToolResult) -> String {
    if let Some(error) = &result.error {
        return error.clone();
    }

    result
        .output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| {
            !line.is_empty()
                && !line.starts_with("timeout_secs:")
                && !line.starts_with("duration_ms:")
                && !line.starts_with("cwd:")
                && !line.starts_with("exit_code:")
        })
        .unwrap_or(if result.success {
            "check passed"
        } else {
            "check failed"
        })
        .chars()
        .take(240)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::project_snapshot::{
        CommandHint, ProjectKind, ProjectSnapshot, ProjectUnit,
    };

    fn snapshot() -> ProjectSnapshot {
        ProjectSnapshot {
            root: PathBuf::from("."),
            truncated: false,
            units: vec![
                ProjectUnit {
                    path: PathBuf::from("frontend"),
                    kind: Some(ProjectKind::Frontend),
                    framework: Some("Next.js".into()),
                    manifests: vec![],
                    commands: vec![
                        CommandHint {
                            label: "build".into(),
                            program: "npm".into(),
                            args: vec!["run".into(), "build".into()],
                            working_directory: PathBuf::from("frontend"),
                            confidence: 0.8,
                        },
                        CommandHint {
                            label: "dev".into(),
                            program: "npm".into(),
                            args: vec!["run".into(), "dev".into()],
                            working_directory: PathBuf::from("frontend"),
                            confidence: 0.8,
                        },
                    ],
                    entry_points: vec![],
                    confidence: 0.9,
                    evidence: vec![],
                },
                ProjectUnit {
                    path: PathBuf::from("backend"),
                    kind: Some(ProjectKind::Backend),
                    framework: Some("Django".into()),
                    manifests: vec![],
                    commands: vec![CommandHint {
                        label: "test".into(),
                        program: ".venv/bin/python".into(),
                        args: vec!["manage.py".into(), "test".into()],
                        working_directory: PathBuf::from("backend"),
                        confidence: 0.9,
                    }],
                    entry_points: vec![],
                    confidence: 0.9,
                    evidence: vec![],
                },
            ],
        }
    }

    #[test]
    fn discovers_only_verification_commands() {
        let checks = discover_checks(&snapshot());
        assert_eq!(checks.len(), 2);
        assert!(checks.iter().all(|c| !c.command().contains("dev")));
    }

    #[test]
    fn matches_direct_and_cd_commands() {
        let checks = discover_checks(&snapshot());
        assert_eq!(
            match_executed_check("npm run build", Path::new("frontend"), &checks)
                .map(|c| c.id.as_str()),
            Some("frontend:build")
        );
        assert_eq!(
            match_executed_check(
                "cd backend && .venv/bin/python manage.py test tasks",
                Path::new("."),
                &checks
            )
            .map(|c| c.id.as_str()),
            Some("backend:test")
        );
    }

    #[test]
    fn does_not_treat_arbitrary_commands_as_verification() {
        let checks = discover_checks(&snapshot());
        assert!(match_executed_check("npm run dev", Path::new("frontend"), &checks).is_none());
        assert!(match_executed_check("ls tests", Path::new("backend"), &checks).is_none());
    }

    #[test]
    fn creates_structured_outcome() {
        let check = discover_checks(&snapshot()).remove(0);
        let result = ToolResult {
            success: true,
            output: "exit_code: 0\ncwd: backend\n3 passed".into(),
            diff: None,
            error: None,
            stop_reason: None,
        };
        let outcome = outcome_for(&check, &result);
        assert!(outcome.passed);
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(outcome.summary, "3 passed");
    }
}
