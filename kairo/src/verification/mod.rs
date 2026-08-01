//! Objective verification discovery and outcome recording.
//!
//! The model remains responsible for choosing when and what to run. This module
//! identifies commands that match checks declared by the repository's project
//! snapshot and classifies their results — including environment/sandbox blocks
//! that are not evidence the patch itself failed.
//!
//! **Slice 5F:** richer [`VerificationStatus`] / [`VerificationScope`], prefer
//! focused edited-file checks, classify broad failures by reported paths, and
//! derive [`CompletionAssessment`] for telemetry without upgrading StopReason.

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

/// Prefer small, non-browser checks first when suggesting or ranking.
fn kind_preference(kind: CheckKind) -> u8 {
    match kind {
        CheckKind::Typecheck => 0,
        CheckKind::Lint => 1,
        CheckKind::Test => 2,
        CheckKind::Build => 3,
        CheckKind::Other => 4,
    }
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

/// How narrowly a check was scoped relative to the edit (Slice 5F).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VerificationScope {
    /// Single edited file (e.g. `eslint path/to/file.ts`).
    EditedFile,
    /// Project unit that owns the edit (e.g. `frontend-v2` lint/test).
    #[default]
    ProjectUnit,
    /// Whole-repo check.
    Repository,
}

/// Classified verification result (Slice 5F).
///
/// Older persisted sessions may still carry `failed`; that deserializes as
/// [`FailedUnknown`] so we never invent path relevance from legacy data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Passed,
    /// Failure output implicates the edited file / coherent failure group.
    FailedRelevant,
    /// Failure output only implicates paths outside the edited target.
    FailedUnrelated,
    /// Failed, but paths could not be classified conservatively.
    #[serde(alias = "failed")]
    FailedUnknown,
    /// Check could not run meaningfully (browser/sandbox/namespace crash, etc.).
    EnvironmentBlocked,
    /// No verification was run after the last edit.
    NotAttempted,
}

impl VerificationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::FailedRelevant => "failed_relevant",
            Self::FailedUnrelated => "failed_unrelated",
            Self::FailedUnknown => "failed_unknown",
            Self::EnvironmentBlocked => "environment_blocked",
            Self::NotAttempted => "not_attempted",
        }
    }

    /// Any failed check (relevant, unrelated, or unknown) — not env / not-attempted.
    pub fn is_failed(self) -> bool {
        matches!(
            self,
            Self::FailedRelevant | Self::FailedUnrelated | Self::FailedUnknown
        )
    }

    /// Failure that should not be treated as evidence the patch is wrong.
    pub fn is_non_patch_failure(self) -> bool {
        matches!(self, Self::EnvironmentBlocked | Self::FailedUnrelated)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationOutcome {
    pub check_id: String,
    pub kind: CheckKind,
    pub status: VerificationStatus,
    pub exit_code: Option<i32>,
    pub summary: String,
    /// Command that was classified (may be a narrowed focused check).
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub scope: VerificationScope,
    /// Paths extracted from failure output (empty when passed / blocked / unknown).
    #[serde(default)]
    pub affected_paths: Vec<PathBuf>,
}

impl VerificationOutcome {
    pub fn passed(&self) -> bool {
        matches!(self.status, VerificationStatus::Passed)
    }

    pub fn is_environment_blocked(&self) -> bool {
        matches!(self.status, VerificationStatus::EnvironmentBlocked)
    }

    pub fn is_non_patch_failure(&self) -> bool {
        self.status.is_non_patch_failure()
    }
}

/// Derived scoreboard label (Slice 5F Part 4). Coexists with conservative
/// [`crate::state::stop_reason::StopReason`]; does not upgrade to TaskComplete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CompletionAssessment {
    /// Focused or unit check passed.
    Verified,
    /// Broad check failed only on unrelated paths (edit not implicated).
    LocallyVerified,
    /// Runner unusable / environment blocked.
    VerificationBlocked,
    /// Failure implicated the edit (or coherent failure group).
    VerificationFailedRelevant,
    /// Ambiguous, not attempted, or no clearer label applies.
    #[default]
    Unverified,
}

impl CompletionAssessment {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::LocallyVerified => "locally_verified",
            Self::VerificationBlocked => "verification_blocked",
            Self::VerificationFailedRelevant => "verification_failed_relevant",
            Self::Unverified => "unverified",
        }
    }

    pub fn from_outcome(outcome: Option<&VerificationOutcome>) -> Self {
        let Some(outcome) = outcome else {
            return Self::Unverified;
        };
        match outcome.status {
            VerificationStatus::Passed => Self::Verified,
            VerificationStatus::FailedUnrelated => Self::LocallyVerified,
            VerificationStatus::EnvironmentBlocked => Self::VerificationBlocked,
            VerificationStatus::FailedRelevant => Self::VerificationFailedRelevant,
            VerificationStatus::FailedUnknown | VerificationStatus::NotAttempted => Self::Unverified,
        }
    }
}

/// Convert repository command hints into stable, typed verification checks.
///
/// Ordered for preference: typecheck → lint → test → build, demoting
/// browser/e2e-flavored commands, then by snapshot confidence.
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
        kind_preference(a.kind)
            .cmp(&kind_preference(b.kind))
            .then_with(|| browserish_penalty(a).cmp(&browserish_penalty(b)))
            .then_with(|| {
                b.confidence
                    .partial_cmp(&a.confidence)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.id.cmp(&b.id))
    });
    checks.dedup_by(|a, b| a.command() == b.command() && a.cwd == b.cwd);
    checks
}

/// Longest snapshot unit path that prefixes `edited_path` (workspace-relative).
pub fn unit_path_for_edit(snapshot: &ProjectSnapshot, edited_path: &Path) -> Option<PathBuf> {
    let edited = normalize_path(edited_path);
    if edited.as_os_str().is_empty() || edited == Path::new(".") {
        return None;
    }

    let mut best: Option<PathBuf> = None;
    let mut best_len = 0usize;
    for unit in &snapshot.units {
        let unit_path = normalize_path(&unit.path);
        if unit_path == Path::new(".") {
            continue; // root is a weak match; only use when nothing else fits
        }
        if path_is_within(&edited, &unit_path) {
            let len = unit_path.components().count();
            if len > best_len {
                best_len = len;
                best = Some(unit_path);
            }
        }
    }
    best.or_else(|| {
        // Fall back to root unit when the edit is at repo root.
        snapshot
            .units
            .iter()
            .find(|u| normalize_path(&u.path) == Path::new("."))
            .map(|u| normalize_path(&u.path))
    })
}

/// Checks scoped to the unit that owns `edited_path`, falling back to all checks.
pub fn discover_checks_for_edit(snapshot: &ProjectSnapshot, edited_path: &Path) -> Vec<Check> {
    let all = discover_checks(snapshot);
    let Some(unit_path) = unit_path_for_edit(snapshot, edited_path) else {
        return all;
    };
    let scoped: Vec<Check> = all
        .iter()
        .filter(|c| check_belongs_to_unit(c, &unit_path))
        .cloned()
        .collect();
    if scoped.is_empty() {
        all
    } else {
        scoped
    }
}

/// Ranked candidates for post-edit verify: focused first, then unit-wide.
///
/// Each entry carries the intended [`VerificationScope`] for classification /
/// telemetry. Duplicates (same command+cwd) keep the narrowest scope.
pub fn rank_checks_for_edit(
    snapshot: &ProjectSnapshot,
    edited_path: &Path,
) -> Vec<(Check, VerificationScope)> {
    let unit_checks = discover_checks_for_edit(snapshot, edited_path);
    let mut ranked: Vec<(Check, VerificationScope)> = Vec::new();

    for base in &unit_checks {
        if let Some(focused) = narrow_check_to_edited_file(base, edited_path) {
            push_ranked(&mut ranked, focused, VerificationScope::EditedFile);
        }
        if let Some(targeted) = narrow_check_to_module(base, edited_path) {
            push_ranked(&mut ranked, targeted, VerificationScope::EditedFile);
        }
    }
    for base in unit_checks {
        let scope = if normalize_path(&base.cwd) == Path::new(".") {
            VerificationScope::Repository
        } else {
            VerificationScope::ProjectUnit
        };
        push_ranked(&mut ranked, base, scope);
    }
    ranked
}

fn push_ranked(out: &mut Vec<(Check, VerificationScope)>, check: Check, scope: VerificationScope) {
    if let Some((_, existing_scope)) = out
        .iter_mut()
        .find(|(c, _)| c.command() == check.command() && c.cwd == check.cwd)
    {
        // Keep the narrower scope if we see the same command twice.
        if scope_rank(scope) < scope_rank(*existing_scope) {
            *existing_scope = scope;
        }
        return;
    }
    out.push((check, scope));
}

fn scope_rank(scope: VerificationScope) -> u8 {
    match scope {
        VerificationScope::EditedFile => 0,
        VerificationScope::ProjectUnit => 1,
        VerificationScope::Repository => 2,
    }
}

/// Narrow a lint check to the single edited file when the toolchain allows it.
pub fn narrow_check_to_edited_file(check: &Check, edited_path: &Path) -> Option<Check> {
    if check.kind != CheckKind::Lint {
        return None;
    }
    let edited = normalize_path(edited_path);
    let cwd = normalize_path(&check.cwd);
    let rel = strip_prefix_path(&edited, &cwd)?;
    if !is_js_ts_lint_path(&rel) {
        return None;
    }
    let rel_str = rel.to_string_lossy().replace('\\', "/");

    let cmd = check.command().to_ascii_lowercase();
    let looks_eslint = cmd.contains("eslint")
        || check.program == "eslint"
        || (check.program == "npx" && check.args.first().is_some_and(|a| a == "eslint"))
        || (check.program == "npm"
            && check.args.len() >= 2
            && check.args[0] == "run"
            && check.args[1] == "lint");

    if !looks_eslint {
        return None;
    }

    Some(Check {
        id: format!("{}:eslint-file", check.id),
        kind: CheckKind::Lint,
        program: "npx".into(),
        args: vec!["eslint".into(), rel_str],
        cwd: check.cwd.clone(),
        confidence: check.confidence,
    })
}

/// Narrow a Django `manage.py test` suite to the app package owning the edit.
pub fn narrow_check_to_module(check: &Check, edited_path: &Path) -> Option<Check> {
    if check.kind != CheckKind::Test {
        return None;
    }
    let cmd = check.command().to_ascii_lowercase();
    if !cmd.contains("manage.py") || !cmd.contains("test") {
        return None;
    }
    // Already targeted (extra args after `test`).
    if let Some(idx) = check.args.iter().position(|a| a == "test") {
        if check.args.len() > idx + 1 {
            return None;
        }
    }

    let edited = normalize_path(edited_path);
    let cwd = normalize_path(&check.cwd);
    let rel = strip_prefix_path(&edited, &cwd)?;
    let label = django_app_label(&rel)?;

    let mut args = check.args.clone();
    if !args.iter().any(|a| a == "test") {
        return None;
    }
    args.push(label);

    Some(Check {
        id: format!("{}:app", check.id),
        kind: CheckKind::Test,
        program: check.program.clone(),
        args,
        cwd: check.cwd.clone(),
        confidence: check.confidence,
    })
}

fn django_app_label(rel_to_cwd: &Path) -> Option<String> {
    let mut comps = rel_to_cwd.components();
    let Component::Normal(first) = comps.next()? else {
        return None;
    };
    let app = first.to_string_lossy();
    if app.is_empty() || app == "manage.py" || app.starts_with('.') {
        return None;
    }
    // backend/annotations/serializers.py → annotations
    Some(app.to_string())
}

fn is_js_ts_lint_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).unwrap_or(""),
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs"
    )
}

fn strip_prefix_path(path: &Path, prefix: &Path) -> Option<PathBuf> {
    let path = normalize_path(path);
    let prefix = normalize_path(prefix);
    if prefix == Path::new(".") {
        return Some(path);
    }
    path.strip_prefix(&prefix).ok().map(|p| p.to_path_buf())
}

fn check_belongs_to_unit(check: &Check, unit_path: &Path) -> bool {
    let cwd = normalize_path(&check.cwd);
    if unit_path == Path::new(".") {
        return cwd == Path::new(".");
    }
    path_is_within(&cwd, unit_path)
}

fn path_is_within(path: &Path, parent: &Path) -> bool {
    if parent == Path::new(".") {
        return true;
    }
    if path == parent {
        return true;
    }
    path.starts_with(parent)
}

/// First preferred non-browserish check, if any (for summaries / fallbacks).
pub fn preferred_check(checks: &[Check]) -> Option<&Check> {
    checks.iter().find(|c| browserish_penalty(c) == 0)
}

/// Default on. Set `AUTO_POST_EDIT_VERIFY=0` (or false/off/no) to disable.
pub fn auto_post_edit_verify_enabled() -> bool {
    match std::env::var("AUTO_POST_EDIT_VERIFY") {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            !(v.is_empty() || v == "0" || v == "false" || v == "off" || v == "no")
        }
        Err(_) => true,
    }
}

pub const AUTO_VERIFY_TIMEOUT_SECS: u64 = 120;

/// Result of a harness-owned post-edit verification run.
#[derive(Debug)]
pub struct AutoVerifyReport {
    pub check: Check,
    pub result: ToolResult,
    pub outcome: VerificationOutcome,
    pub output_suffix: String,
    /// True when the selected check could not run and no usable fallback passed/failed.
    pub skipped_unusable: bool,
}

/// Run a unit-scoped preferred check after a successful file mutation.
///
/// Prefers focused edited-file / module checks, then the project unit that owns
/// `edited_path`. If the preferred check is environment-blocked / unusable,
/// tries the next non-browserish check. Returns `None` when disabled or no
/// suitable check exists.
pub fn try_auto_post_edit_verify(
    snapshot: &ProjectSnapshot,
    workspace_root: &Path,
    edited_path: &Path,
) -> Option<AutoVerifyReport> {
    if !auto_post_edit_verify_enabled() {
        return None;
    }
    let ranked = rank_checks_for_edit(snapshot, edited_path);
    let candidates: Vec<&(Check, VerificationScope)> = ranked
        .iter()
        .filter(|(c, _)| browserish_penalty(c) == 0)
        .collect();
    if candidates.is_empty() {
        return None;
    }

    let mut last_blocked: Option<AutoVerifyReport> = None;
    for (check, scope) in candidates {
        let result = execute_discovered_check(check, workspace_root, AUTO_VERIFY_TIMEOUT_SECS);
        let mut outcome = outcome_for_edit(check, &result, edited_path, *scope);
        // Unusable runners (missing venv, bare exit 133) are not patch failures.
        if !outcome.is_environment_blocked() && is_unusable_check_result(&result) {
            outcome.status = VerificationStatus::EnvironmentBlocked;
            let detail = summarize_result(&result);
            outcome.summary = format!(
                "EnvironmentBlocked: check could not run meaningfully ({detail}). \
                 This does not mean the patch failed."
            );
            outcome.affected_paths.clear();
        }
        let output_suffix = format_auto_verify_suffix(check, &result, &outcome);
        let report = AutoVerifyReport {
            check: check.clone(),
            result,
            outcome: outcome.clone(),
            output_suffix,
            skipped_unusable: outcome.is_environment_blocked(),
        };
        if outcome.is_environment_blocked() {
            last_blocked = Some(report);
            continue;
        }
        return Some(report);
    }
    last_blocked
}

fn execute_discovered_check(
    check: &Check,
    workspace_root: &Path,
    timeout_secs: u64,
) -> ToolResult {
    let cmd = check.command();
    crate::tools::run_shell::execute(
        crate::tools::run_shell::RunShellOpts {
            command: &cmd,
            cwd: &check.cwd,
            timeout_secs,
        },
        workspace_root,
    )
}

fn format_auto_verify_suffix(
    check: &Check,
    result: &ToolResult,
    outcome: &VerificationOutcome,
) -> String {
    let cwd = check.cwd.display();
    let cmd = check.command();
    let status = outcome.status.as_str();
    let body = if result.output.trim().is_empty() {
        result
            .error
            .as_deref()
            .unwrap_or("(no output)")
            .to_string()
    } else {
        let mut out = result.output.clone();
        if let Some(err) = &result.error {
            if !err.is_empty() {
                out.push_str("\n");
                out.push_str(err);
            }
        }
        out
    };
    // Cap so a huge test log does not blow the next model turn.
    let body = truncate_chars(&body, 6_000);
    let guidance = match outcome.status {
        VerificationStatus::EnvironmentBlocked => {
            "This check could not run in this environment. Do not re-run it identically. \
             Finish unverified, or pick a different unit-local check if one exists."
        }
        VerificationStatus::FailedUnrelated => {
            "Focused/edit target was not implicated — failures are outside the edited file. \
             You may finish; do not chase unrelated baseline errors without need."
        }
        VerificationStatus::FailedRelevant | VerificationStatus::FailedUnknown => {
            "If this check failed, fix the code (or a coherent failure group) before finishing. \
             Do not re-run this same command identically without another edit."
        }
        VerificationStatus::Passed => {
            "Check passed. Call finish with a short summary unless you still need a focused re-edit."
        }
        VerificationStatus::NotAttempted => {
            "Verification was not attempted. Prefer a focused unit check before finishing."
        }
    };
    format!(
        "\n\n[auto-verify — ran `{cmd}` in `{cwd}` after edit — {status}]\n\
         {body}\n\
         {guidance}"
    )
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max).collect();
    format!("{truncated}\n…[auto-verify output truncated]")
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
    outcome_for_edit(check, result, Path::new(""), VerificationScope::ProjectUnit)
}

/// Classify a check result, optionally attributing failures to `edited_path`.
pub fn outcome_for_edit(
    check: &Check,
    result: &ToolResult,
    edited_path: &Path,
    scope: VerificationScope,
) -> VerificationOutcome {
    let exit_code = parse_exit_code(&result.output);
    let env_blocked = is_environment_blocked(result);
    let command = check.command();

    let (status, affected_paths) = if result.success {
        (VerificationStatus::Passed, Vec::new())
    } else if env_blocked {
        (VerificationStatus::EnvironmentBlocked, Vec::new())
    } else {
        let paths = extract_failure_paths(result);
        let status = classify_failure_status(&paths, edited_path, scope);
        (status, paths)
    };

    let summary = match status {
        VerificationStatus::EnvironmentBlocked => {
            let detail = summarize_result(result);
            let fallback = preferred_check_hint(check);
            format!(
                "EnvironmentBlocked: check could not run in this sandbox ({detail}). \
                 This does not mean the patch failed.{fallback}"
            )
        }
        VerificationStatus::FailedUnrelated => {
            let detail = summarize_result(result);
            format!(
                "FailedUnrelated: check failed, but reported paths do not include the edited \
                 file (`{}`). {detail}",
                edited_path.display()
            )
        }
        VerificationStatus::FailedRelevant => {
            let detail = summarize_result(result);
            format!("FailedRelevant: edited target implicated. {detail}")
        }
        _ => summarize_result(result),
    };

    VerificationOutcome {
        check_id: check.id.clone(),
        kind: check.kind,
        status,
        exit_code,
        summary,
        command,
        scope,
        affected_paths,
    }
}

/// Conservative path-based failure classification (Slice 5F Part 3).
///
/// - Any affected path matching the edited file → [`FailedRelevant`]
/// - Paths extracted and none match the edit → [`FailedUnrelated`]
/// - No usable paths → [`FailedUnknown`]
pub fn classify_failure_status(
    affected_paths: &[PathBuf],
    edited_path: &Path,
    _scope: VerificationScope,
) -> VerificationStatus {
    let edited = normalize_path(edited_path);
    if edited.as_os_str().is_empty() || edited == Path::new(".") {
        return VerificationStatus::FailedUnknown;
    }
    if affected_paths.is_empty() {
        return VerificationStatus::FailedUnknown;
    }

    if affected_paths
        .iter()
        .any(|p| paths_refer_to_same_file(p, &edited))
    {
        VerificationStatus::FailedRelevant
    } else {
        VerificationStatus::FailedUnrelated
    }
}

fn paths_refer_to_same_file(reported: &Path, edited: &Path) -> bool {
    let a = normalize_path(reported);
    let b = normalize_path(edited);
    if a == b {
        return true;
    }
    let a_s = a.to_string_lossy().replace('\\', "/");
    let b_s = b.to_string_lossy().replace('\\', "/");
    if a_s.ends_with(&b_s) || b_s.ends_with(&a_s) {
        return true;
    }
    // Absolute vs workspace-relative: compare file_name + a parent component.
    match (a.file_name(), b.file_name(), a.parent(), b.parent()) {
        (Some(af), Some(bf), Some(ap), Some(bp)) if af == bf => ap.file_name() == bp.file_name(),
        _ => false,
    }
}

/// Extract file paths from common lint/test failure formats.
pub fn extract_failure_paths(result: &ToolResult) -> Vec<PathBuf> {
    let mut blob = String::new();
    if let Some(err) = &result.error {
        blob.push_str(err);
        blob.push('\n');
    }
    blob.push_str(&result.output);

    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for raw_line in blob.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        for candidate in path_candidates_in_line(line) {
            let key = candidate.to_string_lossy().replace('\\', "/");
            if seen.insert(key) {
                out.push(candidate);
            }
        }
    }
    out
}

fn path_candidates_in_line(line: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let trimmed = line.trim_start_matches([' ', '\t', '-', '>', '|']);
    let trimmed = trimmed.strip_prefix("-->").unwrap_or(trimmed).trim();

    let token = trimmed
        .split_whitespace()
        .next()
        .unwrap_or(trimmed)
        .trim_matches(['"', '\'', '`', ',', ';', '(', ')']);

    let path_part = strip_line_col_suffix(token);
    if looks_like_source_path(path_part) {
        found.push(PathBuf::from(path_part));
    }

    for piece in line.split(|c: char| c.is_whitespace() || c == '(' || c == ')') {
        let piece = piece.trim_matches(['"', '\'', '`', ',', ';']);
        let piece = strip_line_col_suffix(piece);
        if looks_like_source_path(piece) && !found.iter().any(|p| p.as_os_str() == piece) {
            found.push(PathBuf::from(piece));
        }
    }
    found
}

fn strip_line_col_suffix(s: &str) -> &str {
    let mut cut = s.len();
    loop {
        let head = &s[..cut];
        if let Some(idx) = head.rfind(':') {
            let after = &head[idx + 1..];
            if !after.is_empty() && after.chars().all(|c| c.is_ascii_digit()) {
                cut = idx;
                continue;
            }
        }
        break;
    }
    &s[..cut]
}

fn looks_like_source_path(s: &str) -> bool {
    if s.len() < 3 || s.contains("://") {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    const EXTS: &[&str] = &[
        ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".py", ".rs", ".go", ".java", ".kt", ".vue",
        ".svelte",
    ];
    EXTS.iter().any(|ext| lower.ends_with(ext))
}

fn preferred_check_hint(blocked: &Check) -> String {
    // Hint is generic — caller may not have the full check list here.
    let _ = blocked;
    " Prefer a smaller non-browser check (typecheck, lint, unit test, or build) if available."
        .to_string()
}

/// Detect browser/sandbox/namespace failures that are environmental, not test logic.
pub fn is_environment_blocked(result: &ToolResult) -> bool {
    let mut blob = String::new();
    if let Some(err) = &result.error {
        blob.push_str(err);
        blob.push('\n');
    }
    blob.push_str(&result.output);
    let lower = blob.to_ascii_lowercase();

    const MARKERS: &[&str] = &[
        "zygote_host",
        "zygote",
        "pr_set_no_new_privs",
        "setuid sandbox is not running",
        "failed to move to new namespace",
        "devtoolsactiveport",
        "browsertype.launch",
        "playwright",
        "puppeteer",
        "chromium",
        "chrome_crashpad",
        "running as root without --no-sandbox",
        "snap-confine has elevated permissions",
        "target closed",
        "browser has been closed",
        "executable doesn't exist",
        "no usable sandbox",
    ];

    if MARKERS.iter().any(|m| lower.contains(m)) {
        return true;
    }

    // SIGTRAP (133) / SIGABRT (134) from sandboxed runners is almost always env,
    // even without explicit sandbox wording in the log.
    if matches!(parse_exit_code(&result.output), Some(133) | Some(134)) {
        return true;
    }

    false
}

/// Check runner is missing / broken — not evidence the patch is wrong.
pub fn is_unusable_check_result(result: &ToolResult) -> bool {
    if is_environment_blocked(result) {
        return true;
    }
    let mut blob = String::new();
    if let Some(err) = &result.error {
        blob.push_str(err);
        blob.push('\n');
    }
    blob.push_str(&result.output);
    let lower = blob.to_ascii_lowercase();

    const MARKERS: &[&str] = &[
        "no such file or directory",
        "not found",
        "cannot execute",
        "permission denied",
        "module not found",
        "unable to locate",
        "did you forget to activate",
        "no module named",
        ".venv/bin",
        "virtualenv",
        "command not found",
    ];
    if MARKERS.iter().any(|m| lower.contains(m)) && !result.success {
        // Avoid treating ordinary assertion "not found" test output as unusable
        // unless the exit looks like a runner failure (127) or path errors dominate.
        if matches!(parse_exit_code(&result.output), Some(127) | Some(126) | Some(2)) {
            return true;
        }
        if lower.contains("no such file or directory")
            || lower.contains("command not found")
            || lower.contains("cannot execute")
        {
            return true;
        }
    }
    false
}

fn browserish_penalty(check: &Check) -> u8 {
    let cmd = check.command().to_ascii_lowercase();
    const KEYS: &[&str] = &[
        "playwright",
        "cypress",
        "selenium",
        "puppeteer",
        "e2e",
        "chrome",
        "chromium",
        "webdriver",
    ];
    if KEYS.iter().any(|k| cmd.contains(k)) {
        10
    } else {
        0
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
        // Prefer a more specific line from output when present.
        if let Some(line) = result.output.lines().rev().map(str::trim).find(|line| {
            !line.is_empty()
                && !line.starts_with("timeout_secs:")
                && !line.starts_with("duration_ms:")
                && !line.starts_with("cwd:")
                && !line.starts_with("exit_code:")
                && !line.starts_with("--- ")
        }) {
            if line.len() > 8 {
                return line.chars().take(240).collect();
            }
        }
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
                            label: "typecheck".into(),
                            program: "npm".into(),
                            args: vec!["run".into(), "typecheck".into()],
                            working_directory: PathBuf::from("frontend"),
                            confidence: 0.75,
                        },
                        CommandHint {
                            label: "lint".into(),
                            program: "npm".into(),
                            args: vec!["run".into(), "lint".into()],
                            working_directory: PathBuf::from("frontend"),
                            confidence: 0.7,
                        },
                        CommandHint {
                            label: "dev".into(),
                            program: "npm".into(),
                            args: vec!["run".into(), "dev".into()],
                            working_directory: PathBuf::from("frontend"),
                            confidence: 0.8,
                        },
                        CommandHint {
                            label: "test".into(),
                            program: "npx".into(),
                            args: vec!["playwright".into(), "test".into()],
                            working_directory: PathBuf::from("frontend"),
                            confidence: 0.95,
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
    fn auto_verify_can_be_disabled() {
        std::env::set_var("AUTO_POST_EDIT_VERIFY", "0");
        let none = try_auto_post_edit_verify(&snapshot(), Path::new("."), Path::new("frontend/a.ts"));
        std::env::remove_var("AUTO_POST_EDIT_VERIFY");
        assert!(none.is_none());
    }

    #[test]
    fn auto_verify_suffix_marks_failure() {
        let check = Check {
            id: "root:test".into(),
            kind: CheckKind::Test,
            program: "npm".into(),
            args: vec!["test".into()],
            cwd: PathBuf::from("."),
            confidence: 0.9,
        };
        let result = ToolResult {
            success: false,
            output: "# fail 2\n".into(),
            diff: None,
            error: Some("exited with code 1".into()),
            stop_reason: None,
        };
        let outcome = outcome_for(&check, &result);
        let suffix = format_auto_verify_suffix(&check, &result, &outcome);
        assert!(suffix.contains("[auto-verify"));
        assert!(suffix.contains("failed_unknown") || suffix.contains("failed"));
        assert!(suffix.contains("npm test"));
        assert!(suffix.contains("# fail 2"));
    }

    #[test]
    fn discovers_only_verification_commands() {
        let checks = discover_checks(&snapshot());
        assert!(checks.iter().all(|c| !c.command().contains("dev")));
        assert!(checks.len() >= 3);
    }

    #[test]
    fn ranks_typecheck_before_build_and_demotes_playwright() {
        let checks = discover_checks(&snapshot());
        let ids: Vec<&str> = checks.iter().map(|c| c.id.as_str()).collect();
        let typecheck = ids.iter().position(|id| *id == "frontend:typecheck").unwrap();
        let build = ids.iter().position(|id| *id == "frontend:build").unwrap();
        let pw = ids.iter().position(|id| *id == "frontend:test").unwrap();
        assert!(typecheck < build);
        assert!(pw > build || pw > typecheck);
        assert_eq!(preferred_check(&checks).map(|c| c.id.as_str()), Some("frontend:typecheck"));
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
        let check = discover_checks(&snapshot())
            .into_iter()
            .find(|c| c.id == "backend:test")
            .unwrap();
        let result = ToolResult {
            success: true,
            output: "exit_code: 0\ncwd: backend\n3 passed".into(),
            diff: None,
            error: None,
            stop_reason: None,
        };
        let outcome = outcome_for(&check, &result);
        assert!(outcome.passed());
        assert_eq!(outcome.status, VerificationStatus::Passed);
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(outcome.summary, "3 passed");
    }

    #[test]
    fn chromium_sandbox_crash_is_environment_blocked() {
        let check = discover_checks(&snapshot())
            .into_iter()
            .find(|c| c.id == "backend:test")
            .unwrap();
        let result = ToolResult {
            success: false,
            output: "exit_code: 133\ncwd: backend\ntimeout_secs: 60\n\
                     The setuid sandbox is not running as root.\n\
                     Failed to move to new namespace: PID namespaces supported\n\
                     FATAL:content/browser/zygote_host/zygote_host_impl_linux.cc\n\
                     Trace/breakpoint trap (core dumped)\n"
                .into(),
            diff: None,
            error: Some("exited with code 133".into()),
            stop_reason: None,
        };
        assert!(is_environment_blocked(&result));
        let outcome = outcome_for(&check, &result);
        assert_eq!(outcome.status, VerificationStatus::EnvironmentBlocked);
        assert!(!outcome.passed());
        assert!(outcome.summary.contains("EnvironmentBlocked"));
        assert!(outcome.summary.contains("does not mean the patch failed"));
    }

    #[test]
    fn ordinary_test_failure_is_failed_not_blocked() {
        let check = discover_checks(&snapshot())
            .into_iter()
            .find(|c| c.id == "backend:test")
            .unwrap();
        let result = ToolResult {
            success: false,
            output: "exit_code: 1\ncwd: backend\nFAIL: test_list_requires_date\nAssertionError\n"
                .into(),
            diff: None,
            error: Some("exited with code 1".into()),
            stop_reason: None,
        };
        let outcome = outcome_for(&check, &result);
        assert_eq!(outcome.status, VerificationStatus::FailedUnknown);
        assert!(!outcome.summary.contains("EnvironmentBlocked"));
    }

    #[test]
    fn backend_edit_prefers_backend_checks() {
        let checks = discover_checks_for_edit(&snapshot(), Path::new("backend/app/serializers.py"));
        assert!(checks.iter().all(|c| c.id.starts_with("backend:")));
        assert_eq!(preferred_check(&checks).map(|c| c.id.as_str()), Some("backend:test"));
    }

    #[test]
    fn frontend_edit_prefers_frontend_checks() {
        let checks = discover_checks_for_edit(&snapshot(), Path::new("frontend/src/Board.tsx"));
        assert!(checks.iter().all(|c| c.id.starts_with("frontend:")));
        assert_eq!(
            preferred_check(&checks).map(|c| c.id.as_str()),
            Some("frontend:typecheck")
        );
        assert!(!checks.iter().any(|c| c.id.starts_with("backend:")));
    }

    #[test]
    fn bare_exit_133_is_environment_blocked() {
        let result = ToolResult {
            success: false,
            output: "exit_code: 133\ncwd: backend\n".into(),
            diff: None,
            error: Some("exited with code 133".into()),
            stop_reason: None,
        };
        assert!(is_environment_blocked(&result));
        assert!(is_unusable_check_result(&result));
    }

    #[test]
    fn missing_command_is_unusable() {
        let result = ToolResult {
            success: false,
            output: "exit_code: 127\ncwd: backend\n.venv/bin/python: No such file or directory\n"
                .into(),
            diff: None,
            error: Some("exited with code 127".into()),
            stop_reason: None,
        };
        assert!(is_unusable_check_result(&result));
    }

    #[test]
    fn part5f_narrow_eslint_prefers_edited_file() {
        let ranked = rank_checks_for_edit(
            &snapshot(),
            Path::new("frontend/src/lib/task-status.ts"),
        );
        let focused = ranked
            .iter()
            .find(|(c, s)| {
                *s == VerificationScope::EditedFile && c.command().contains("eslint")
            })
            .expect("focused eslint");
        assert_eq!(focused.0.args.last().map(String::as_str), Some("src/lib/task-status.ts"));
        assert!(ranked
            .iter()
            .any(|(c, s)| *s == VerificationScope::ProjectUnit && c.id.contains("lint")));
    }

    #[test]
    fn part5f_narrow_django_test_to_app_label() {
        let ranked =
            rank_checks_for_edit(&snapshot(), Path::new("backend/annotations/serializers.py"));
        let focused = ranked
            .iter()
            .find(|(c, s)| *s == VerificationScope::EditedFile && c.command().contains("annotations"))
            .expect("targeted django test");
        assert!(focused.0.args.iter().any(|a| a == "annotations"));
    }

    #[test]
    fn part5f_unrelated_lint_paths_are_failed_unrelated() {
        let check = Check {
            id: "frontend:lint".into(),
            kind: CheckKind::Lint,
            program: "npm".into(),
            args: vec!["run".into(), "lint".into()],
            cwd: PathBuf::from("frontend"),
            confidence: 0.7,
        };
        let result = ToolResult {
            success: false,
            output: "exit_code: 1\n\
                     /home/proj/frontend/src/components/SiteNav.tsx\n\
                       36:5  error  setState in effect\n\
                     /home/proj/frontend/src/components/tasks/Board.tsx\n\
                       52:7  error  setState in effect\n"
                .into(),
            diff: None,
            error: Some("exited with code 1".into()),
            stop_reason: None,
        };
        let outcome = outcome_for_edit(
            &check,
            &result,
            Path::new("frontend/src/lib/task-status.ts"),
            VerificationScope::ProjectUnit,
        );
        assert_eq!(outcome.status, VerificationStatus::FailedUnrelated);
        assert_eq!(
            CompletionAssessment::from_outcome(Some(&outcome)),
            CompletionAssessment::LocallyVerified
        );
        assert!(!outcome.affected_paths.is_empty());
    }

    #[test]
    fn part5f_edited_file_in_lint_output_is_failed_relevant() {
        let check = Check {
            id: "frontend:lint".into(),
            kind: CheckKind::Lint,
            program: "npx".into(),
            args: vec!["eslint".into(), ".".into()],
            cwd: PathBuf::from("frontend"),
            confidence: 0.7,
        };
        let result = ToolResult {
            success: false,
            output: "exit_code: 1\nfrontend/src/lib/task-status.ts\n  14:3  error  Unexpected\n"
                .into(),
            diff: None,
            error: Some("exited with code 1".into()),
            stop_reason: None,
        };
        let outcome = outcome_for_edit(
            &check,
            &result,
            Path::new("frontend/src/lib/task-status.ts"),
            VerificationScope::EditedFile,
        );
        assert_eq!(outcome.status, VerificationStatus::FailedRelevant);
        assert_eq!(
            CompletionAssessment::from_outcome(Some(&outcome)),
            CompletionAssessment::VerificationFailedRelevant
        );
    }

    #[test]
    fn part5f_ambiguous_failure_is_failed_unknown() {
        let check = discover_checks(&snapshot())
            .into_iter()
            .find(|c| c.id == "backend:test")
            .unwrap();
        let result = ToolResult {
            success: false,
            output: "exit_code: 1\nFAIL: something broke with no path\n".into(),
            diff: None,
            error: Some("exited with code 1".into()),
            stop_reason: None,
        };
        let outcome = outcome_for_edit(
            &check,
            &result,
            Path::new("backend/annotations/serializers.py"),
            VerificationScope::ProjectUnit,
        );
        assert_eq!(outcome.status, VerificationStatus::FailedUnknown);
        assert_eq!(
            CompletionAssessment::from_outcome(Some(&outcome)),
            CompletionAssessment::Unverified
        );
    }

    #[test]
    fn part5f_env_blocked_maps_to_verification_blocked_assessment() {
        let check = discover_checks(&snapshot())
            .into_iter()
            .find(|c| c.id == "backend:test")
            .unwrap();
        let result = ToolResult {
            success: false,
            output: "exit_code: 133\nzygote_host crash\n".into(),
            diff: None,
            error: Some("exited with code 133".into()),
            stop_reason: None,
        };
        let outcome = outcome_for_edit(
            &check,
            &result,
            Path::new("backend/annotations/serializers.py"),
            VerificationScope::EditedFile,
        );
        assert_eq!(outcome.status, VerificationStatus::EnvironmentBlocked);
        assert_eq!(
            CompletionAssessment::from_outcome(Some(&outcome)),
            CompletionAssessment::VerificationBlocked
        );
    }

    #[test]
    fn part5f_legacy_failed_serde_alias_roundtrips_as_unknown() {
        let json = r#""failed""#;
        let status: VerificationStatus = serde_json::from_str(json).unwrap();
        assert_eq!(status, VerificationStatus::FailedUnknown);
    }
}
