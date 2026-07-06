use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

/// Replace the task workspace with a fresh copy from its snapshot.
pub fn reset_workspace(snapshot: &Path, workspace: &Path) -> Result<()> {
    anyhow::ensure!(
        snapshot.is_dir(),
        "snapshot does not exist: {}",
        snapshot.display()
    );

    if workspace.exists() {
        fs::remove_dir_all(workspace)
            .with_context(|| format!("failed to clear workspace {}", workspace.display()))?;
    }

    copy_dir(snapshot, workspace)
        .with_context(|| format!("failed to reset workspace {}", workspace.display()))?;

    Ok(())
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();

        if name == "__pycache__" {
            continue;
        }

        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if entry.file_type()?.is_dir() {
            copy_dir(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn reset_copies_snapshot_into_workspace() {
        let root = tempdir().unwrap();
        let snapshot = root.path().join("snap");
        let workspace = root.path().join("work");

        fs::create_dir(&snapshot).unwrap();
        fs::write(snapshot.join("calc.py"), "broken").unwrap();

        reset_workspace(&snapshot, &workspace).unwrap();

        assert_eq!(
            fs::read_to_string(workspace.join("calc.py")).unwrap(),
            "broken"
        );
    }
}
