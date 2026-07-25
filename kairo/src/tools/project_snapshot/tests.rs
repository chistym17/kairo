use std::fs;
use std::path::PathBuf;

use super::*;
use crate::tools::project_snapshot::types::ProjectKind;

fn write(path: &std::path::Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, body).unwrap();
}

#[test]
fn nextjs_single_project() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("package.json"),
        r#"{
          "name": "web",
          "scripts": { "dev": "next dev", "lint": "next lint" },
          "dependencies": { "next": "15.0.0", "react": "19.0.0" }
        }"#,
    );
    write(&dir.path().join("src/app/page.tsx"), "export default function Page(){}");
    write(&dir.path().join("src/app/layout.tsx"), "export default function Root({children}){return children}");

    let snap = detect(dir.path());
    assert!(!snap.units.is_empty());
    let u = &snap.units[0];
    assert_eq!(u.kind, Some(ProjectKind::Frontend));
    assert_eq!(u.framework.as_deref(), Some("Next.js"));
    assert!(u.commands.iter().any(|c| c.label == "lint"));
    assert!(u
        .entry_points
        .iter()
        .any(|e| e.path.ends_with("src/app/page.tsx")));
}

#[test]
fn django_with_venv_hint() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("manage.py"), "#!/usr/bin/env python\n");
    write(
        &dir.path().join("requirements.txt"),
        "django==5.0\n",
    );
    write(&dir.path().join("config/settings.py"), "SECRET_KEY='x'\n");
    // fake venv python path (file is enough for detection)
    write(&dir.path().join(".venv/bin/python"), "");

    let snap = detect(dir.path());
    let u = snap
        .units
        .iter()
        .find(|u| u.framework.as_deref() == Some("Django"))
        .expect("django unit");
    assert_eq!(u.kind, Some(ProjectKind::Backend));
    let test_cmd = u.commands.iter().find(|c| c.label == "test").unwrap();
    assert!(test_cmd.program.contains("venv"));
    assert_eq!(test_cmd.args, vec!["manage.py".to_string(), "test".to_string()]);
    assert_eq!(test_cmd.working_directory, PathBuf::from("."));
}

#[test]
fn django_plus_next_monorepo() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("backend/manage.py"), "#!/usr/bin/env python\n");
    write(&dir.path().join("backend/requirements.txt"), "Django>=4\n");
    write(
        &dir.path().join("frontend-v2/package.json"),
        r#"{"dependencies":{"next":"14.0.0","react":"18.0.0"},"scripts":{"lint":"eslint ."}}"#,
    );
    write(&dir.path().join("frontend-v2/src/app/page.tsx"), "export default function P(){return null}");

    let snap = detect(dir.path());
    let paths: Vec<_> = snap.units.iter().map(|u| u.path.to_string_lossy().to_string()).collect();
    assert!(paths.iter().any(|p| p == "backend"), "{paths:?}");
    assert!(paths.iter().any(|p| p == "frontend-v2"), "{paths:?}");

    let be = snap.units.iter().find(|u| u.path.ends_with("backend")).unwrap();
    assert_eq!(be.kind, Some(ProjectKind::Backend));
    assert_eq!(be.commands[0].working_directory, PathBuf::from("backend"));

    let fe = snap.units.iter().find(|u| u.path.ends_with("frontend-v2")).unwrap();
    assert_eq!(fe.kind, Some(ProjectKind::Frontend));
    assert_eq!(fe.framework.as_deref(), Some("Next.js"));
}

#[test]
fn pnpm_workspace_app_and_lib() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("pnpm-workspace.yaml"),
        "packages:\n  - 'apps/*'\n  - 'packages/*'\n",
    );
    write(
        &dir.path().join("apps/web/package.json"),
        r#"{"dependencies":{"next":"14.0.0","react":"18.0.0"},"scripts":{"dev":"next dev"}}"#,
    );
    write(
        &dir.path().join("packages/ui/package.json"),
        r#"{"name":"ui","dependencies":{"react":"18.0.0"}}"#,
    );

    let snap = detect(dir.path());
    let web = snap.units.iter().find(|u| u.path.ends_with("apps/web")).unwrap();
    assert_eq!(web.kind, Some(ProjectKind::Frontend));
    let ui = snap.units.iter().find(|u| u.path.ends_with("packages/ui")).unwrap();
    assert_eq!(ui.kind, Some(ProjectKind::Library));
}

#[test]
fn rust_workspace_bin_and_lib() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("Cargo.toml"),
        r#"[workspace]
members = ["crates/cli", "crates/core"]
"#,
    );
    write(
        &dir.path().join("crates/cli/Cargo.toml"),
        r#"[package]
name = "cli"
version = "0.1.0"
[[bin]]
name = "cli"
path = "src/main.rs"
"#,
    );
    write(&dir.path().join("crates/cli/src/main.rs"), "fn main() {}");
    write(
        &dir.path().join("crates/core/Cargo.toml"),
        r#"[package]
name = "core"
version = "0.1.0"
[lib]
path = "src/lib.rs"
"#,
    );
    write(&dir.path().join("crates/core/src/lib.rs"), "pub fn x() {}");

    let snap = detect(dir.path());
    let cli = snap.units.iter().find(|u| u.path.ends_with("crates/cli")).unwrap();
    assert_eq!(cli.kind, Some(ProjectKind::Cli));
    let core = snap.units.iter().find(|u| u.path.ends_with("crates/core")).unwrap();
    assert_eq!(core.kind, Some(ProjectKind::Library));
}

#[test]
fn unknown_repo_stays_low_confidence() {
    let dir = tempfile::tempdir().unwrap();
    write(&dir.path().join("notes.txt"), "hello");
    let snap = detect(dir.path());
    // Still returns a placeholder unit for empty manifests
    assert!(!snap.units.is_empty());
    let u = &snap.units[0];
    assert!(u.confidence < 0.5);
    assert!(matches!(u.kind, None | Some(ProjectKind::Unknown)));
}

#[test]
fn ignores_node_modules_and_venv_contents() {
    let dir = tempfile::tempdir().unwrap();
    write(
        &dir.path().join("package.json"),
        r#"{"dependencies":{"react":"18.0.0"},"scripts":{"start":"node index.js"}}"#,
    );
    write(
        &dir.path().join("node_modules/foo/package.json"),
        r#"{"name":"foo"}"#,
    );
    write(&dir.path().join("venv/lib/python/site.py"), "");

    let snap = detect(dir.path());
    assert!(
        snap.units.iter().all(|u| !u.path.to_string_lossy().contains("node_modules")),
        "{:?}",
        snap.units
    );
    assert!(
        snap.units.iter().all(|u| !u.path.to_string_lossy().contains("venv")),
        "{:?}",
        snap.units
    );
}

#[test]
fn output_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..20 {
        write(
            &dir.path().join(format!("pkg{i}/package.json")),
            r#"{"name":"x","dependencies":{"express":"4.0.0"},"scripts":{"start":"node index.js","test":"jest","lint":"eslint","build":"tsc"}}"#,
        );
    }
    let snap = detect(dir.path());
    assert!(snap.units.len() <= MAX_UNITS);
    assert!(snap.truncated);
    for u in &snap.units {
        assert!(u.commands.len() <= MAX_COMMANDS);
        assert!(u.entry_points.len() <= MAX_ENTRY_POINTS);
        assert!(u.evidence.len() <= super::detect::MAX_EVIDENCE);
    }
    let rendered = snap.render_compact();
    assert!(rendered.len() < 20_000, "rendered {} chars", rendered.len());
}
