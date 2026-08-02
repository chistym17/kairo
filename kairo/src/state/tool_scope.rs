//! Runtime-derived tool access scoping.
//!
//! See `annotaa/final-design-plan.md` §5.2. `ToolScope` describes what a
//! single tool may currently operate on. It is always computed fresh from
//! authoritative runtime state (Layer 1) — never stored or mutated in place
//! (the "derive policy from state" invariant, §7.3.2 of the plan).
//!
//! This module introduces the type only. Nothing in the runtime constructs
//! or consults a `ToolScope` yet; that wiring is a later, separate change.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// How freely a tool may currently be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessLevel {
    /// No restriction beyond normal argument validation.
    Unrestricted,
    /// Restricted by `ScopeConstraints` and/or `ScopeBudget`.
    Bounded,
    /// Not usable at all right now.
    Disabled,
}

/// What a `Bounded` tool call may target.
///
/// `allowed_unit_roots` is keyed by a project unit's root path rather than
/// the `ProjectUnit` value itself, since `ProjectUnit` carries an `f32`
/// confidence score and does not implement `Eq`/`Hash`. The root path is the
/// smallest identifier needed to answer "is this operating within the
/// allowed unit?".
///
/// A `None` field means "no constraint of this kind" — but that only has
/// effect for `AccessLevel::Unrestricted`. For a `Bounded` scope, an unset
/// axis denies, it does not fall back to unrestricted (see
/// `rejects_bounded_paths_scope_leaking_into_unit_axis` in the tests below).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScopeConstraints {
    pub allowed_paths: Option<HashSet<PathBuf>>,
    pub allowed_unit_roots: Option<HashSet<PathBuf>>,
}

/// How many bounded calls remain, independent of path/unit constraints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeBudget {
    Unlimited,
    Uses(u32),
}

/// A tool's current permitted access. Always derived fresh from runtime
/// state via a smart constructor below — never built from bare fields, so
/// the "a `Bounded` scope always has a real constraint" invariant can't be
/// bypassed, including in release builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolScope {
    pub access: AccessLevel,
    pub constraints: ScopeConstraints,
    pub budget: ScopeBudget,
}

impl ToolScope {
    /// No restriction beyond normal argument validation.
    pub fn unrestricted() -> Self {
        Self {
            access: AccessLevel::Unrestricted,
            constraints: ScopeConstraints::default(),
            budget: ScopeBudget::Unlimited,
        }
    }

    /// Not usable at all right now.
    pub fn disabled() -> Self {
        Self {
            access: AccessLevel::Disabled,
            constraints: ScopeConstraints::default(),
            budget: ScopeBudget::Unlimited,
        }
    }

    /// Bounded to an explicit, non-empty set of paths (e.g. "already-seen
    /// files", "the failed patch target").
    ///
    /// Panics (in all build profiles — this is a real `assert!`, not
    /// `debug_assert!`, so the invariant holds in release too) if `paths` is
    /// empty. An empty path set is not treated as "budget-only, any path" —
    /// `allows_path` on an empty set is vacuously false for every path, so a
    /// scope like that would be permanently unusable regardless of budget,
    /// which is a worse trap than simply rejecting the construction. A
    /// distinct "budget-only, no path constraint" constructor would be
    /// needed to express that case; none is required yet.
    pub fn bounded_paths(paths: HashSet<PathBuf>, budget: ScopeBudget) -> Self {
        assert!(
            !paths.is_empty(),
            "bounded scope requires a non-empty path set; use disabled() instead"
        );
        Self {
            access: AccessLevel::Bounded,
            constraints: ScopeConstraints {
                allowed_paths: Some(paths),
                allowed_unit_roots: None,
            },
            budget,
        }
    }

    /// Bounded to an explicit, non-empty set of project units, identified by
    /// root path. Same non-empty-set rule as `bounded_paths`, for the same
    /// reason.
    pub fn bounded_unit_roots(unit_roots: HashSet<PathBuf>, budget: ScopeBudget) -> Self {
        assert!(
            !unit_roots.is_empty(),
            "bounded scope requires a non-empty unit-root set; use disabled() instead"
        );
        Self {
            access: AccessLevel::Bounded,
            constraints: ScopeConstraints {
                allowed_paths: None,
                allowed_unit_roots: Some(unit_roots),
            },
            budget,
        }
    }

    /// True if `path` is permitted under this scope's access and path
    /// constraint alone — this does **not** check budget. See `permits_path`
    /// for the combined check callers should normally use.
    pub fn allows_path(&self, path: &Path) -> bool {
        match self.access {
            AccessLevel::Unrestricted => true,
            AccessLevel::Disabled => false,
            AccessLevel::Bounded => self
                .constraints
                .allowed_paths
                .as_ref()
                .is_some_and(|paths| paths.iter().any(|p| paths_equal(p, path))),
        }
    }

    /// True if `unit_root` is permitted under this scope's access and
    /// unit-root constraint alone — this does **not** check budget. See
    /// `permits_unit_root` for the combined check callers should normally
    /// use.
    pub fn allows_unit_root(&self, unit_root: &Path) -> bool {
        match self.access {
            AccessLevel::Unrestricted => true,
            AccessLevel::Disabled => false,
            AccessLevel::Bounded => self
                .constraints
                .allowed_unit_roots
                .as_ref()
                .is_some_and(|roots| roots.iter().any(|r| paths_equal(r, unit_root))),
        }
    }

    /// True if at least one use remains under this scope's budget.
    pub fn has_budget(&self) -> bool {
        match self.budget {
            ScopeBudget::Unlimited => true,
            ScopeBudget::Uses(remaining) => remaining > 0,
        }
    }

    /// Combined check: is `path` allowed **and** is there budget left to act
    /// on it? Callers evaluating an actual tool call should use this (or
    /// `permits_unit_root`), not `allows_path`/`has_budget` separately, so
    /// the combination isn't re-derived ad hoc at every call site.
    pub fn permits_path(&self, path: &Path) -> bool {
        self.allows_path(path) && self.has_budget()
    }

    /// Combined check, mirroring `permits_path` for the unit-root axis.
    pub fn permits_unit_root(&self, unit_root: &Path) -> bool {
        self.allows_unit_root(unit_root) && self.has_budget()
    }
}

/// Path comparison normalized for separator/`./`-prefix differences.
///
/// Deliberately local to this module for now (Part 1 is isolated by design);
/// `state::action_policy` has an equivalent private helper that this may
/// consolidate with once `ToolScope` is actually wired in.
fn paths_equal(a: &Path, b: &Path) -> bool {
    let na = a
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string();
    let nb = b
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string();
    na == nb
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(paths: &[&str]) -> HashSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn unrestricted_allows_any_path_and_unit_and_has_budget() {
        let scope = ToolScope::unrestricted();
        assert_eq!(scope.access, AccessLevel::Unrestricted);
        assert!(scope.allows_path(Path::new("anything.rs")));
        assert!(scope.allows_unit_root(Path::new("anywhere")));
        assert!(scope.has_budget());
        assert!(scope.permits_path(Path::new("anything.rs")));
    }

    #[test]
    fn disabled_allows_nothing_even_though_budget_field_reads_unlimited() {
        let scope = ToolScope::disabled();
        assert_eq!(scope.access, AccessLevel::Disabled);
        assert!(!scope.allows_path(Path::new("a.rs")));
        assert!(!scope.allows_unit_root(Path::new("backend")));
        assert!(!scope.permits_path(Path::new("a.rs")));
    }

    #[test]
    fn bounded_paths_allows_only_listed_paths() {
        let scope = ToolScope::bounded_paths(set(&["Board.tsx", "store.ts"]), ScopeBudget::Unlimited);
        assert!(scope.allows_path(Path::new("Board.tsx")));
        assert!(scope.allows_path(Path::new("store.ts")));
        assert!(!scope.allows_path(Path::new("other.tsx")));
    }

    #[test]
    fn bounded_paths_normalizes_separators_and_dot_prefix() {
        let scope = ToolScope::bounded_paths(set(&["./src/Board.tsx"]), ScopeBudget::Unlimited);
        assert!(scope.allows_path(Path::new("src/Board.tsx")));
    }

    #[test]
    fn bounded_unit_roots_allows_only_listed_roots() {
        let scope = ToolScope::bounded_unit_roots(set(&["backend"]), ScopeBudget::Uses(1));
        assert!(scope.allows_unit_root(Path::new("backend")));
        assert!(!scope.allows_unit_root(Path::new("frontend")));
    }

    #[test]
    fn budget_reflects_remaining_uses() {
        let exhausted = ToolScope::bounded_paths(set(&["a.rs"]), ScopeBudget::Uses(0));
        assert!(!exhausted.has_budget());

        let available = ToolScope::bounded_paths(set(&["a.rs"]), ScopeBudget::Uses(1));
        assert!(available.has_budget());

        let unlimited = ToolScope::bounded_paths(set(&["a.rs"]), ScopeBudget::Unlimited);
        assert!(unlimited.has_budget());
    }

    #[test]
    fn permits_path_requires_both_allowance_and_budget() {
        let exhausted = ToolScope::bounded_paths(set(&["a.rs"]), ScopeBudget::Uses(0));
        // Path is on the allow-list, but the budget is spent.
        assert!(exhausted.allows_path(Path::new("a.rs")));
        assert!(!exhausted.permits_path(Path::new("a.rs")));
    }

    #[test]
    fn rejects_bounded_paths_scope_leaking_into_unit_axis() {
        // A path-only bounded scope must not implicitly allow every unit
        // root just because allowed_unit_roots was never set.
        let scope = ToolScope::bounded_paths(set(&["a.rs"]), ScopeBudget::Unlimited);
        assert!(!scope.allows_unit_root(Path::new("anything")));
    }

    #[test]
    fn rejects_bounded_unit_roots_scope_leaking_into_path_axis() {
        let scope = ToolScope::bounded_unit_roots(set(&["backend"]), ScopeBudget::Unlimited);
        assert!(!scope.allows_path(Path::new("anything.rs")));
    }

    #[test]
    fn constructors_produce_expected_field_values() {
        let scope = ToolScope::bounded_paths(set(&["a.rs"]), ScopeBudget::Uses(2));
        assert_eq!(
            scope,
            ToolScope {
                access: AccessLevel::Bounded,
                constraints: ScopeConstraints {
                    allowed_paths: Some(set(&["a.rs"])),
                    allowed_unit_roots: None,
                },
                budget: ScopeBudget::Uses(2),
            }
        );
    }

    #[test]
    #[should_panic(expected = "non-empty path set")]
    fn bounded_paths_rejects_empty_set_with_unlimited_budget() {
        let _ = ToolScope::bounded_paths(HashSet::new(), ScopeBudget::Unlimited);
    }

    #[test]
    #[should_panic(expected = "non-empty path set")]
    fn bounded_paths_rejects_empty_set_even_with_finite_budget() {
        // Deliberately stricter than "empty set OK if budget is finite":
        // an empty allow-list makes allows_path vacuously false forever,
        // so the budget could never be spent — always reject it outright.
        let _ = ToolScope::bounded_paths(HashSet::new(), ScopeBudget::Uses(3));
    }

    #[test]
    #[should_panic(expected = "non-empty unit-root set")]
    fn bounded_unit_roots_rejects_empty_set() {
        let _ = ToolScope::bounded_unit_roots(HashSet::new(), ScopeBudget::Unlimited);
    }
}
