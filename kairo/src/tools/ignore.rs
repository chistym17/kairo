/// Directories skipped by exploration tools (list/search/find).
pub const IGNORED_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".turbo",
    ".cache",
    "coverage",
    "__pycache__",
    ".venv",
    "venv",
];

pub fn is_ignored_dir_name(name: &str) -> bool {
    IGNORED_DIRS.contains(&name)
}
