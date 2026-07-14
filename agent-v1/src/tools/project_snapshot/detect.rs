use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::tools::ignore::is_ignored_dir_name;

use super::types::{CommandHint, EntryPoint, ProjectKind, ProjectSnapshot, ProjectUnit};

pub const MAX_UNITS: usize = 12;
pub const MAX_COMMANDS: usize = 4;
pub const MAX_ENTRY_POINTS: usize = 5;
pub const MAX_EVIDENCE: usize = 6;

const MANIFEST_NAMES: &[&str] = &[
    "package.json",
    "pnpm-workspace.yaml",
    "yarn.lock",
    "package-lock.json",
    "pyproject.toml",
    "requirements.txt",
    "manage.py",
    "Cargo.toml",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "Makefile",
    "docker-compose.yml",
    "docker-compose.yaml",
];

/// Walk the workspace and build a compact orientation snapshot.
pub fn detect(workspace_root: &Path) -> ProjectSnapshot {
    let mut truncated = false;
    let manifests = collect_manifests(workspace_root, &mut truncated);
    let mut unit_paths = derive_unit_paths(workspace_root, &manifests);

    if unit_paths.is_empty() {
        unit_paths.insert(PathBuf::from("."));
    }

    let mut units: Vec<ProjectUnit> = unit_paths
        .into_iter()
        .map(|rel| inspect_unit(workspace_root, &rel))
        .collect();

    // Prefer more-confident / deeper application units when capping.
    units.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.path.as_os_str().len().cmp(&b.path.as_os_str().len()))
    });

    if units.len() > MAX_UNITS {
        units.truncate(MAX_UNITS);
        truncated = true;
    }

    // Stable display order: path.
    units.sort_by(|a, b| a.path.cmp(&b.path));

    ProjectSnapshot {
        root: PathBuf::from("."),
        units,
        truncated,
    }
}

fn collect_manifests(workspace_root: &Path, truncated: &mut bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let _ = walk_manifests(workspace_root, workspace_root, &mut out, truncated);
    out.sort();
    out
}

fn walk_manifests(
    workspace_root: &Path,
    cur: &Path,
    out: &mut Vec<PathBuf>,
    truncated: &mut bool,
) -> std::io::Result<()> {
    if out.len() >= 200 {
        *truncated = true;
        return Ok(());
    }
    let entries = match fs::read_dir(cur) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if is_ignored_dir_name(&name) {
                continue;
            }
            walk_manifests(workspace_root, &path, out, truncated)?;
        } else if MANIFEST_NAMES.contains(&name.as_ref()) {
            if let Ok(rel) = path.strip_prefix(workspace_root) {
                out.push(rel.to_path_buf());
            }
        }
    }
    Ok(())
}

fn derive_unit_paths(workspace_root: &Path, manifests: &[PathBuf]) -> BTreeSet<PathBuf> {
    let mut units = BTreeSet::new();

    for m in manifests {
        let name = m.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let parent = m
            .parent()
            .map(|p| {
                if p.as_os_str().is_empty() {
                    PathBuf::from(".")
                } else {
                    p.to_path_buf()
                }
            })
            .unwrap_or_else(|| PathBuf::from("."));

        match name {
            "package.json" | "manage.py" | "pyproject.toml" | "go.mod" | "pom.xml"
            | "build.gradle" => {
                units.insert(normalize_unit(&parent));
            }
            "Cargo.toml" => {
                let members = cargo_workspace_members(workspace_root, m);
                if members.is_empty() {
                    units.insert(normalize_unit(&parent));
                } else {
                    for mem in members {
                        units.insert(normalize_unit(&parent.join(mem)));
                    }
                }
            }
            "pnpm-workspace.yaml" => {
                for pkg in pnpm_workspace_packages(workspace_root, m) {
                    units.insert(normalize_unit(&pkg));
                }
            }
            "requirements.txt" => {
                // Only a unit if nothing stronger exists in the same dir.
                let has_stronger = ["manage.py", "pyproject.toml", "package.json", "Cargo.toml"]
                    .iter()
                    .any(|n| workspace_root.join(&parent).join(n).exists());
                if !has_stronger {
                    units.insert(normalize_unit(&parent));
                }
            }
            _ => {}
        }
    }

    units
}

fn normalize_unit(path: &Path) -> PathBuf {
    if path.as_os_str().is_empty() || path == Path::new("") {
        PathBuf::from(".")
    } else {
        path.to_path_buf()
    }
}

fn cargo_workspace_members(workspace_root: &Path, cargo_toml: &Path) -> Vec<PathBuf> {
    let text = fs::read_to_string(workspace_root.join(cargo_toml)).unwrap_or_default();
    if !text.contains("[workspace]") {
        return Vec::new();
    }
    // crude: members = ["a", "b"] or multi-line
    let mut members = Vec::new();
    let mut in_members = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("members") && t.contains('[') {
            in_members = true;
        }
        if in_members {
            for part in t.split([',', '[', ']', '"', '\'']) {
                let p = part.trim();
                if !p.is_empty()
                    && p != "members"
                    && !p.starts_with('#')
                    && !p.contains('=')
                {
                    members.push(PathBuf::from(p));
                }
            }
            if t.contains(']') {
                break;
            }
        }
    }
    members
}

fn pnpm_workspace_packages(workspace_root: &Path, yaml: &Path) -> Vec<PathBuf> {
    let text = fs::read_to_string(workspace_root.join(yaml)).unwrap_or_default();
    let mut patterns = Vec::new();
    let mut in_packages = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("packages:") {
            in_packages = true;
            continue;
        }
        if in_packages {
            if t.starts_with('-') {
                let pat = t
                    .trim_start_matches('-')
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'');
                if !pat.is_empty() {
                    patterns.push(pat.to_string());
                }
            } else if !t.is_empty() && !t.starts_with('#') {
                break;
            }
        }
    }

    let mut out = Vec::new();
    for pat in patterns {
        if let Some(star) = pat.strip_suffix("/*") {
            let base = workspace_root.join(star);
            if let Ok(rd) = fs::read_dir(&base) {
                for e in rd.flatten() {
                    if e.path().is_dir() && e.path().join("package.json").exists() {
                        if let Ok(rel) = e.path().strip_prefix(workspace_root) {
                            out.push(rel.to_path_buf());
                        }
                    }
                }
            }
        } else {
            let p = workspace_root.join(&pat);
            if p.join("package.json").exists() {
                out.push(PathBuf::from(pat));
            }
        }
    }
    out
}

fn inspect_unit(workspace_root: &Path, rel: &Path) -> ProjectUnit {
    let abs = if rel == Path::new(".") {
        workspace_root.to_path_buf()
    } else {
        workspace_root.join(rel)
    };

    let mut manifests = Vec::new();
    for name in [
        "package.json",
        "pyproject.toml",
        "requirements.txt",
        "manage.py",
        "Cargo.toml",
        "go.mod",
        "pom.xml",
        "build.gradle",
        "Makefile",
        "next.config.js",
        "next.config.ts",
        "next.config.mjs",
        "vite.config.ts",
        "vite.config.js",
    ] {
        if abs.join(name).exists() {
            manifests.push(rel_join(rel, name));
        }
    }

    let mut evidence = Vec::new();
    let mut kind = None;
    let mut framework = None;
    let mut confidence: f32 = 0.35;
    let mut commands = Vec::new();
    let mut entry_points = Vec::new();

    if abs.join("package.json").exists() {
        analyze_package_json(
            workspace_root,
            rel,
            &abs,
            &mut kind,
            &mut framework,
            &mut confidence,
            &mut evidence,
            &mut commands,
            &mut entry_points,
        );
    }

    if abs.join("manage.py").exists() {
        kind = Some(ProjectKind::Backend);
        framework = Some("Django".into());
        confidence = confidence.max(0.9);
        push_ev(&mut evidence, "manage.py exists");
        add_django_commands(&abs, rel, &mut commands);
        add_django_entries(rel, &abs, &mut entry_points);
    } else if abs.join("pyproject.toml").exists() || abs.join("requirements.txt").exists() {
        analyze_python(
            &abs,
            rel,
            &mut kind,
            &mut framework,
            &mut confidence,
            &mut evidence,
            &mut commands,
            &mut entry_points,
        );
    }

    if abs.join("Cargo.toml").exists() {
        analyze_cargo(
            &abs,
            rel,
            &mut kind,
            &mut framework,
            &mut confidence,
            &mut evidence,
            &mut commands,
            &mut entry_points,
        );
    }

    if abs.join("go.mod").exists() {
        kind = kind.or(Some(ProjectKind::Backend));
        framework = framework.or(Some("Go".into()));
        confidence = confidence.max(0.7);
        push_ev(&mut evidence, "go.mod exists");
        commands.push(CommandHint {
            label: "test".into(),
            program: "go".into(),
            args: vec!["test".into(), "./...".into()],
            working_directory: rel.to_path_buf(),
            confidence: 0.75,
        });
    }

    truncate_unit_fields(
        &mut evidence,
        &mut commands,
        &mut entry_points,
    );

    ProjectUnit {
        path: normalize_unit(rel),
        kind,
        framework,
        manifests,
        commands,
        entry_points,
        confidence,
        evidence,
    }
}

fn rel_join(unit: &Path, name: &str) -> PathBuf {
    if unit == Path::new(".") {
        PathBuf::from(name)
    } else {
        unit.join(name)
    }
}

fn push_ev(ev: &mut Vec<String>, s: impl Into<String>) {
    if ev.len() < MAX_EVIDENCE {
        ev.push(s.into());
    }
}

fn analyze_package_json(
    _workspace_root: &Path,
    rel: &Path,
    abs: &Path,
    kind: &mut Option<ProjectKind>,
    framework: &mut Option<String>,
    confidence: &mut f32,
    evidence: &mut Vec<String>,
    commands: &mut Vec<CommandHint>,
    entry_points: &mut Vec<EntryPoint>,
) {
    let raw = fs::read_to_string(abs.join("package.json")).unwrap_or_default();
    let json: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    let deps = merge_deps(&json);

    let pm = detect_package_manager(abs);

    if deps.iter().any(|d| d == "next") {
        *kind = Some(ProjectKind::Frontend);
        *framework = Some("Next.js".into());
        *confidence = (*confidence).max(0.92);
        push_ev(evidence, "dependency \"next\"");
        add_next_entries(rel, abs, entry_points);
    } else if deps.iter().any(|d| d == "react")
        && (deps.iter().any(|d| d == "vite")
            || abs.join("vite.config.ts").exists()
            || abs.join("vite.config.js").exists())
    {
        *kind = Some(ProjectKind::Frontend);
        *framework = Some("Vite React".into());
        *confidence = (*confidence).max(0.85);
        push_ev(evidence, "react + vite");
        add_vite_entries(rel, abs, entry_points);
    } else if deps.iter().any(|d| d == "react") {
        // Could be app or library
        let scripts = json.get("scripts").cloned().unwrap_or(Value::Null);
        let has_dev = scripts.get("dev").is_some() || scripts.get("start").is_some();
        if has_dev {
            *kind = Some(ProjectKind::Frontend);
            *framework = Some("React".into());
            *confidence = (*confidence).max(0.75);
            push_ev(evidence, "react + dev/start script");
        } else {
            *kind = Some(ProjectKind::Library);
            *framework = Some("React".into());
            *confidence = (*confidence).max(0.65);
            push_ev(evidence, "react package without app start script");
        }
    }

    if deps.iter().any(|d| d == "express") {
        *kind = kind.or(Some(ProjectKind::Backend));
        if framework.is_none() {
            *framework = Some("Express".into());
        }
        *confidence = (*confidence).max(0.8);
        push_ev(evidence, "dependency \"express\"");
        add_node_server_entries(rel, abs, entry_points);
    }
    if deps.iter().any(|d| d == "fastify") {
        *kind = kind.or(Some(ProjectKind::Backend));
        *framework = Some("Fastify".into());
        *confidence = (*confidence).max(0.8);
        push_ev(evidence, "dependency \"fastify\"");
        add_node_server_entries(rel, abs, entry_points);
    }

    if kind.is_none() {
        *kind = Some(ProjectKind::Unknown);
        *confidence = (*confidence).max(0.4);
        push_ev(evidence, "package.json present");
    }

    // Scripts → commands + script-referenced entry points
    if let Some(scripts) = json.get("scripts").and_then(|s| s.as_object()) {
        for (name, val) in scripts {
            if !["test", "lint", "build", "dev", "start", "typecheck", "check"].contains(&name.as_str())
            {
                continue;
            }
            if let Some(cmd) = val.as_str() {
                commands.push(CommandHint {
                    label: name.clone(),
                    program: pm.clone(),
                    args: vec!["run".into(), name.clone()],
                    working_directory: normalize_unit(rel),
                    confidence: 0.8,
                });
                // Extract path-like tokens from script
                for token in cmd.split_whitespace() {
                    let t = token.trim_matches(|c| c == '"' || c == '\'');
                    if (t.ends_with(".ts")
                        || t.ends_with(".js")
                        || t.ends_with(".tsx")
                        || t.ends_with(".mjs"))
                        && !t.starts_with('-')
                    {
                        let candidate = abs.join(t);
                        if candidate.exists() {
                            entry_points.push(EntryPoint {
                                path: rel_join(rel, t),
                                score: 12,
                                evidence: vec![format!("script \"{name}\" references {t}")],
                            });
                        }
                    }
                }
            }
            if commands.len() >= MAX_COMMANDS {
                break;
            }
        }
    }
}

fn merge_deps(json: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(obj) = json.get(key).and_then(|v| v.as_object()) {
            out.extend(obj.keys().cloned());
        }
    }
    out
}

fn detect_package_manager(abs: &Path) -> String {
    if abs.join("pnpm-lock.yaml").exists() || abs.parent().map(|p| p.join("pnpm-lock.yaml").exists()).unwrap_or(false)
    {
        "pnpm".into()
    } else if abs.join("yarn.lock").exists() {
        "yarn".into()
    } else {
        "npm".into()
    }
}

fn analyze_python(
    abs: &Path,
    rel: &Path,
    kind: &mut Option<ProjectKind>,
    framework: &mut Option<String>,
    confidence: &mut f32,
    evidence: &mut Vec<String>,
    commands: &mut Vec<CommandHint>,
    entry_points: &mut Vec<EntryPoint>,
) {
    let blob = [
        fs::read_to_string(abs.join("pyproject.toml")).unwrap_or_default(),
        fs::read_to_string(abs.join("requirements.txt")).unwrap_or_default(),
    ]
    .join("\n")
    .to_lowercase();

    if blob.contains("fastapi") {
        *kind = Some(ProjectKind::Backend);
        *framework = Some("FastAPI".into());
        *confidence = (*confidence).max(0.85);
        push_ev(evidence, "fastapi dependency");
        for cand in ["main.py", "app/main.py", "src/main.py"] {
            if abs.join(cand).exists() {
                entry_points.push(EntryPoint {
                    path: rel_join(rel, cand),
                    score: 10,
                    evidence: vec!["FastAPI convention".into()],
                });
            }
        }
        add_python_test_commands(abs, rel, commands);
    } else if blob.contains("django") {
        *kind = Some(ProjectKind::Backend);
        *framework = Some("Django".into());
        *confidence = (*confidence).max(0.8);
        push_ev(evidence, "django dependency");
        add_django_commands(abs, rel, commands);
    } else {
        *kind = kind.or(Some(ProjectKind::Unknown));
        *confidence = (*confidence).max(0.45);
        push_ev(evidence, "Python manifest present");
        add_python_test_commands(abs, rel, commands);
    }
}

fn analyze_cargo(
    abs: &Path,
    rel: &Path,
    kind: &mut Option<ProjectKind>,
    framework: &mut Option<String>,
    confidence: &mut f32,
    evidence: &mut Vec<String>,
    commands: &mut Vec<CommandHint>,
    entry_points: &mut Vec<EntryPoint>,
) {
    let text = fs::read_to_string(abs.join("Cargo.toml")).unwrap_or_default();
    *framework = Some("Rust".into());
    if text.contains("[[bin]]") || abs.join("src/main.rs").exists() {
        *kind = Some(ProjectKind::Cli);
        *confidence = (*confidence).max(0.8);
        push_ev(evidence, "Rust binary target");
        if abs.join("src/main.rs").exists() {
            entry_points.push(EntryPoint {
                path: rel_join(rel, "src/main.rs"),
                score: 10,
                evidence: vec!["src/main.rs".into()],
            });
        }
    } else if text.contains("[lib]") || abs.join("src/lib.rs").exists() {
        *kind = Some(ProjectKind::Library);
        *confidence = (*confidence).max(0.75);
        push_ev(evidence, "Rust library target");
        if abs.join("src/lib.rs").exists() {
            entry_points.push(EntryPoint {
                path: rel_join(rel, "src/lib.rs"),
                score: 9,
                evidence: vec!["src/lib.rs".into()],
            });
        }
    } else {
        *kind = Some(ProjectKind::Unknown);
        *confidence = (*confidence).max(0.5);
        push_ev(evidence, "Cargo.toml present");
    }
    commands.push(CommandHint {
        label: "test".into(),
        program: "cargo".into(),
        args: vec!["test".into()],
        working_directory: normalize_unit(rel),
        confidence: 0.85,
    });
}

fn find_python_bin(unit_abs: &Path) -> Option<PathBuf> {
    for candidate in [".venv/bin/python", "venv/bin/python"] {
        let p = unit_abs.join(candidate);
        if p.exists() {
            return Some(PathBuf::from(candidate));
        }
    }
    None
}

fn add_django_commands(abs: &Path, rel: &Path, commands: &mut Vec<CommandHint>) {
    let program = find_python_bin(abs)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "python".into());
    let conf = if program.contains("venv") { 0.9 } else { 0.55 };
    commands.push(CommandHint {
        label: "test".into(),
        program,
        args: vec!["manage.py".into(), "test".into()],
        working_directory: normalize_unit(rel),
        confidence: conf,
    });
}

fn add_python_test_commands(abs: &Path, rel: &Path, commands: &mut Vec<CommandHint>) {
    let program = find_python_bin(abs)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "python".into());
    let conf = if program.contains("venv") { 0.85 } else { 0.5 };
    commands.push(CommandHint {
        label: "pytest".into(),
        program: program.clone(),
        args: vec!["-m".into(), "pytest".into()],
        working_directory: normalize_unit(rel),
        confidence: conf,
    });
}

fn add_django_entries(rel: &Path, abs: &Path, entry_points: &mut Vec<EntryPoint>) {
    entry_points.push(EntryPoint {
        path: rel_join(rel, "manage.py"),
        score: 11,
        evidence: vec!["Django manage.py".into()],
    });
    // settings.py / urls.py under first-level package dirs
    if let Ok(rd) = fs::read_dir(abs) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let name = e.file_name();
            let name = name.to_string_lossy();
            if is_ignored_dir_name(&name) || name.starts_with('.') {
                continue;
            }
            for leaf in ["settings.py", "urls.py"] {
                if p.join(leaf).exists() {
                    let rel_path = rel_join(rel, &format!("{name}/{leaf}"));
                    entry_points.push(EntryPoint {
                        path: rel_path,
                        score: 8,
                        evidence: vec![format!("Django {leaf}")],
                    });
                }
            }
        }
    }
}

fn add_next_entries(rel: &Path, abs: &Path, entry_points: &mut Vec<EntryPoint>) {
    for cand in [
        "src/app/layout.tsx",
        "src/app/page.tsx",
        "app/layout.tsx",
        "app/page.tsx",
        "src/pages/_app.tsx",
        "pages/_app.tsx",
        "next.config.ts",
        "next.config.js",
        "next.config.mjs",
        "middleware.ts",
        "src/middleware.ts",
    ] {
        if abs.join(cand).exists() {
            entry_points.push(EntryPoint {
                path: rel_join(rel, cand),
                score: 9,
                evidence: vec!["Next.js convention".into()],
            });
        }
    }
}

fn add_vite_entries(rel: &Path, abs: &Path, entry_points: &mut Vec<EntryPoint>) {
    for cand in ["src/main.tsx", "src/main.ts", "src/App.tsx", "vite.config.ts", "vite.config.js"] {
        if abs.join(cand).exists() {
            entry_points.push(EntryPoint {
                path: rel_join(rel, cand),
                score: 9,
                evidence: vec!["Vite/React convention".into()],
            });
        }
    }
}

fn add_node_server_entries(rel: &Path, abs: &Path, entry_points: &mut Vec<EntryPoint>) {
    for cand in [
        "src/index.ts",
        "src/server.ts",
        "src/app.ts",
        "index.ts",
        "server.ts",
        "app.ts",
        "src/index.js",
        "src/server.js",
    ] {
        if abs.join(cand).exists() {
            entry_points.push(EntryPoint {
                path: rel_join(rel, cand),
                score: 8,
                evidence: vec!["Node service convention".into()],
            });
        }
    }
}

fn truncate_unit_fields(
    evidence: &mut Vec<String>,
    commands: &mut Vec<CommandHint>,
    entry_points: &mut Vec<EntryPoint>,
) {
    evidence.truncate(MAX_EVIDENCE);
    commands.truncate(MAX_COMMANDS);
    // Dedup entry points by path, keep highest score
    entry_points.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
    let mut seen = BTreeSet::new();
    entry_points.retain(|e| seen.insert(e.path.clone()));
    entry_points.truncate(MAX_ENTRY_POINTS);
    for ep in entry_points.iter_mut() {
        ep.evidence.truncate(3);
    }
}
