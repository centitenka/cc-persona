use anyhow::Result;
use std::path::Path;

use crate::config::{AppConfig, Paths, ProjectMeta};

/// Remove project bindings (and their state dirs) whose directories no longer
/// exist on disk. Never automatic — a temporarily-unmounted project shouldn't
/// lose its binding, so this is an explicit opt-in cleanup.
pub fn run(paths: &Paths) -> Result<()> {
    let mut config = AppConfig::load(&paths.config)?;
    let mut removed: Vec<String> = Vec::new();

    let keys: Vec<String> = config.projects.keys().cloned().collect();
    for key in keys {
        if !Path::new(&key).exists() {
            config.projects.remove(&key);
            removed.push(key);
        }
    }
    config.save(&paths.config)?;

    // Remove orphan state dirs whose recorded project path is gone.
    let projects_root = paths.projects_root();
    if projects_root.is_dir() {
        for entry in std::fs::read_dir(&projects_root)?.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let meta_path = dir.join("meta.json");
            let gone = std::fs::read_to_string(&meta_path)
                .ok()
                .and_then(|c| serde_json::from_str::<ProjectMeta>(&c).ok())
                .map(|m| !Path::new(&m.project_path).exists())
                .unwrap_or(false);
            if gone {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }

    if removed.is_empty() {
        eprintln!("✓ No stale project bindings.");
    } else {
        eprintln!("✓ Pruned {} stale project binding(s):", removed.len());
        for r in &removed {
            eprintln!("  - {}", r);
        }
    }
    Ok(())
}
