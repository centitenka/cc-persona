use anyhow::{Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};

use crate::persona::McpConfig;

/// Read ~/.claude.json and return the full JSON value.
pub fn read_claude_json(path: &Path) -> Result<Value> {
    if !path.exists() {
        return Ok(Value::Object(serde_json::Map::new()));
    }
    let content = std::fs::read_to_string(path).context("Failed to read ~/.claude.json")?;
    let value: Value = serde_json::from_str(&content).context("Failed to parse ~/.claude.json")?;
    Ok(value)
}

/// Write the full JSON value back to ~/.claude.json, atomically.
///
/// `~/.claude.json` is a single shared file mutated by every scope (and, with
/// project scope, by multiple concurrent windows). An in-place write that is
/// interrupted mid-flush corrupts the user's entire Claude config, so we write to
/// a sibling temp file and `rename` it over the target — rename is atomic on the
/// same filesystem.
pub fn write_claude_json(path: &Path, value: &Value) -> Result<()> {
    let content =
        serde_json::to_string_pretty(value).context("Failed to serialize ~/.claude.json")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = temp_sibling(path);
    std::fs::write(&tmp, content.as_bytes())
        .with_context(|| format!("Failed to write temp file {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("Failed to atomically replace {}", path.display()))?;
    Ok(())
}

/// Read-modify-write `~/.claude.json` under an advisory lock + atomic rename.
///
/// The lock serialises concurrent `cc-persona` invocations (multiple windows) so
/// they do not lose each other's updates; the rename guarantees the file is never
/// left half-written. Best-effort: if the lock cannot be taken it proceeds with a
/// warning rather than deadlocking.
pub fn update_claude_json<F>(path: &Path, lock_path: &Path, f: F) -> Result<()>
where
    F: FnOnce(&mut Value) -> Result<()>,
{
    let _guard = ClaudeJsonLock::acquire(lock_path);
    let mut json = read_claude_json(path)?;
    f(&mut json)?;
    write_claude_json(path, &json)
}

fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".cc-persona.tmp");
    match path.parent() {
        Some(parent) => parent.join(name),
        None => PathBuf::from(name),
    }
}

/// Apply persona top-level MCP server toggles to `mcpServers` in `~/.claude.json`.
///
/// Matches server names by substring against the persona's enable/disable lists
/// and sets/clears the `disabled` field. This is the user-level MCP lever and runs
/// at every scope. (Formerly `apply_mcp_config`.)
pub fn apply_mcp_servers(claude_json: &mut Value, mcp: &McpConfig) -> Result<()> {
    let servers = match claude_json.get_mut("mcpServers") {
        Some(Value::Object(map)) => map,
        _ => return Ok(()), // No mcpServers section, nothing to do
    };

    for (name, server) in servers.iter_mut() {
        let server_obj = match server.as_object_mut() {
            Some(o) => o,
            None => continue,
        };

        if mcp.enable.iter().any(|e| name.contains(e.as_str())) {
            server_obj.remove("disabled");
        } else if mcp.disable.iter().any(|d| name.contains(d.as_str())) {
            server_obj.insert("disabled".to_string(), Value::Bool(true));
        }
    }

    Ok(())
}

/// Apply persona connector toggles to `projects.<key>.disabledMcpServers`.
///
/// claude.ai connectors (e.g. Figma) are managed per-project in Claude Code via a
/// `disabledMcpServers` name list under the project entry. Enabling a connector =
/// removing it from the list; disabling = adding it. Candidate connector names are
/// drawn from `claudeAiMcpEverConnected` plus whatever is already disabled, then
/// substring-matched against the persona's enable/disable patterns. Only ever
/// touches `projects.<key>.disabledMcpServers` — never the rest of the document.
pub fn apply_connectors(claude_json: &mut Value, project_key: &str, mcp: &McpConfig) -> Result<()> {
    if mcp.enable.is_empty() && mcp.disable.is_empty() {
        return Ok(());
    }

    // Reconcile to Claude Code's literal projects.<key> (see [`resolve_project_key`]).
    // ALL connector read/write paths reconcile identically, so backup/restore/dirty
    // never operate on a different key than this write does.
    let project_key = resolve_project_key(claude_json, Path::new(project_key));
    let project_key = project_key.as_str();

    // Candidate connector names: ever-connected ∪ currently-disabled for this key.
    let mut candidates: Vec<String> = connector_universe(claude_json);
    candidates.extend(current_disabled_connectors(claude_json, project_key));
    candidates.sort();
    candidates.dedup();

    // Start from the current disabled set and adjust it.
    let mut disabled: Vec<String> = current_disabled_connectors(claude_json, project_key);

    for name in &candidates {
        if mcp.enable.iter().any(|e| name.contains(e.as_str())) {
            disabled.retain(|d| d != name);
        } else if mcp.disable.iter().any(|d| name.contains(d.as_str()))
            && !disabled.iter().any(|d| d == name)
        {
            disabled.push(name.clone());
        }
    }
    disabled.sort();
    disabled.dedup();

    // Write back, creating the projects.<key> object lazily.
    let projects = claude_json
        .as_object_mut()
        .context("~/.claude.json is not a JSON object")?
        .entry("projects")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let projects = projects
        .as_object_mut()
        .context("`projects` in ~/.claude.json is not an object")?;
    let entry = projects
        .entry(project_key.to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let entry = entry
        .as_object_mut()
        .context("project entry in ~/.claude.json is not an object")?;
    entry.insert(
        "disabledMcpServers".to_string(),
        Value::Array(disabled.into_iter().map(Value::String).collect()),
    );
    Ok(())
}

/// The `projects.<key>` string actually used by Claude Code for `cwd`.
///
/// Claude Code keys projects by the literal path it recorded, which may differ
/// from cc-persona's canonicalized form. Prefer an existing key whose
/// canonicalized form equals `cwd`; otherwise fall back to `cwd`'s string.
pub fn resolve_project_key(claude_json: &Value, cwd: &Path) -> String {
    let want = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    if let Some(Value::Object(projects)) = claude_json.get("projects") {
        for key in projects.keys() {
            let canon = std::fs::canonicalize(key).unwrap_or_else(|_| PathBuf::from(key));
            if canon == want {
                return key.clone();
            }
        }
    }
    cwd.to_string_lossy().into_owned()
}

/// All known MCP names across the three sources, for a given scope. Used to detect
/// persona patterns that match nothing (the v0.2 silent-no-op blind spot).
///
/// Union of: top-level `mcpServers` names, `claudeAiMcpEverConnected` names, and
/// (when `project_key` is set) the project's `disabledMcpServers`.
pub fn known_mcp_names(claude_json: &Value, project_key: Option<&str>) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if let Some(Value::Object(servers)) = claude_json.get("mcpServers") {
        names.extend(servers.keys().cloned());
    }
    names.extend(connector_universe(claude_json));
    if let Some(key) = project_key {
        names.extend(current_disabled_connectors(claude_json, key));
    }
    names.sort();
    names.dedup();
    names
}

/// Every MCP name known across ALL scopes: top-level `mcpServers`, the connector
/// universe (`claudeAiMcpEverConnected`), and every project's `disabledMcpServers`.
///
/// `doctor` runs at global scope and has no single project key, so it uses this to
/// avoid false "pattern matches nothing" warnings for a connector that currently
/// lives only in some project's `disabledMcpServers`.
pub fn all_known_mcp_names(claude_json: &Value) -> Vec<String> {
    let mut names = known_mcp_names(claude_json, None);
    if let Some(Value::Object(projects)) = claude_json.get("projects") {
        for entry in projects.values() {
            if let Some(arr) = entry.get("disabledMcpServers").and_then(|v| v.as_array()) {
                names.extend(arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())));
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Connector names with their disabled state for a project key (read-only, for doctor).
pub fn list_connectors(path: &Path, project_key: &str) -> Result<Vec<(String, bool)>> {
    let json = read_claude_json(path)?;
    let project_key = resolve_project_key(&json, Path::new(project_key));
    let project_key = project_key.as_str();
    let disabled = current_disabled_connectors(&json, project_key);
    let mut universe = connector_universe(&json);
    universe.extend(disabled.iter().cloned());
    universe.sort();
    universe.dedup();
    let mut result: Vec<(String, bool)> = universe
        .into_iter()
        .map(|name| {
            let is_disabled = disabled.iter().any(|d| d == &name);
            (name, is_disabled)
        })
        .collect();
    result.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}

/// The `disabledMcpServers` list under `projects.<key>` (empty if absent).
pub fn list_disabled_connectors(path: &Path, project_key: &str) -> Result<Vec<String>> {
    let json = read_claude_json(path)?;
    let project_key = resolve_project_key(&json, Path::new(project_key));
    let mut names = current_disabled_connectors(&json, &project_key);
    names.sort();
    Ok(names)
}

/// Read `projects.<key>.disabledMcpServers` as an `Option`: `None` when the field
/// is absent (so restore can tell "absent" from "present-but-empty").
pub fn read_project_disabled(claude_json: &Value, project_key: &str) -> Option<Vec<String>> {
    let project_key = resolve_project_key(claude_json, Path::new(project_key));
    claude_json
        .get("projects")
        .and_then(|p| p.get(&project_key))
        .and_then(|e| e.get("disabledMcpServers"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
}

/// Set (or, with `None`, remove) `projects.<key>.disabledMcpServers`. Creates the
/// `projects.<key>` object when setting; leaves the rest of the document alone.
pub fn set_project_disabled(
    claude_json: &mut Value,
    project_key: &str,
    value: Option<&[String]>,
) -> Result<()> {
    let project_key = resolve_project_key(claude_json, Path::new(project_key));
    let project_key = project_key.as_str();
    match value {
        Some(list) => {
            let projects = claude_json
                .as_object_mut()
                .context("~/.claude.json is not a JSON object")?
                .entry("projects")
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            let projects = projects
                .as_object_mut()
                .context("`projects` in ~/.claude.json is not an object")?;
            let entry = projects
                .entry(project_key.to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            let entry = entry
                .as_object_mut()
                .context("project entry in ~/.claude.json is not an object")?;
            entry.insert(
                "disabledMcpServers".to_string(),
                Value::Array(list.iter().cloned().map(Value::String).collect()),
            );
        }
        None => {
            if let Some(entry) = claude_json
                .get_mut("projects")
                .and_then(|p| p.get_mut(project_key))
                .and_then(|e| e.as_object_mut())
            {
                entry.remove("disabledMcpServers");
            }
        }
    }
    Ok(())
}

/// Names recorded in top-level `claudeAiMcpEverConnected` (claude.ai connectors).
fn connector_universe(claude_json: &Value) -> Vec<String> {
    match claude_json.get("claudeAiMcpEverConnected") {
        Some(Value::Object(map)) => map.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

/// The current `projects.<key>.disabledMcpServers` entries.
fn current_disabled_connectors(claude_json: &Value, project_key: &str) -> Vec<String> {
    claude_json
        .get("projects")
        .and_then(|p| p.get(project_key))
        .and_then(|e| e.get("disabledMcpServers"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// List all top-level MCP server names and their disabled status.
pub fn list_mcp_servers(path: &Path) -> Result<Vec<(String, bool)>> {
    let json = read_claude_json(path)?;
    let mut result = Vec::new();
    if let Some(Value::Object(servers)) = json.get("mcpServers") {
        for (name, server) in servers {
            let disabled = server
                .get("disabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            result.push((name.clone(), disabled));
        }
    }
    result.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(result)
}

/// Best-effort advisory lock around `~/.claude.json` read-modify-write, held for
/// the lifetime of the guard and released on drop.
///
/// The lock file is stamped with a per-acquire `nonce` so [`Drop`] removes ONLY a
/// lock we still own. Without that, a false-stale takeover (a holder that legitimately
/// ran past the 30s threshold) would cascade: process B reclaims A's lock, and later
/// A's `Drop` deletes B's lock by path, letting a third writer race in. Matching the
/// nonce before removal breaks that cascade.
struct ClaudeJsonLock {
    path: Option<PathBuf>,
    nonce: String,
}

impl ClaudeJsonLock {
    fn acquire(lock_path: &Path) -> Self {
        use std::io::Write;
        if let Some(parent) = lock_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let nonce = format!("{}-{}", std::process::id(), next_lock_nonce());
        // Spin up to ~10s (slow/NFS disk or a large file can keep a holder busy),
        // re-checking staleness every pass; then proceed unlocked rather than
        // deadlock. This is advisory and best-effort by design.
        for _ in 0..250 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(lock_path)
            {
                Ok(mut file) => {
                    let _ = file.write_all(nonce.as_bytes());
                    return Self {
                        path: Some(lock_path.to_path_buf()),
                        nonce,
                    };
                }
                Err(_) => {
                    if lock_is_stale(lock_path) {
                        // Abandoned by a crashed process — reclaim and retry at once.
                        let _ = std::fs::remove_file(lock_path);
                        continue;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(40));
                }
            }
        }
        eprintln!(
            "  ⚠ Could not lock {}; proceeding without it (concurrent writes possible).",
            lock_path.display()
        );
        Self { path: None, nonce }
    }

    /// Whether the on-disk lock still carries our nonce (i.e. we still own it).
    fn still_ours(&self) -> bool {
        match &self.path {
            Some(path) => std::fs::read_to_string(path)
                .map(|c| c.trim() == self.nonce)
                .unwrap_or(false),
            None => false,
        }
    }
}

impl Drop for ClaudeJsonLock {
    fn drop(&mut self) {
        // Remove the lock only if it is STILL ours: a false-stale takeover may have
        // handed it to another acquirer (whose nonce now differs) — leave that alone.
        let Some(path) = self.path.as_ref() else {
            return;
        };
        if self.still_ours() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Process-unique, monotonic counter feeding lock nonces (avoids time/RNG deps).
fn next_lock_nonce() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// A lock file older than 30s is considered abandoned (left by a crashed process).
fn lock_is_stale(lock_path: &Path) -> bool {
    std::fs::metadata(lock_path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|mtime| mtime.elapsed().ok())
        .map(|age| age.as_secs() > 30)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn apply_mcp_servers_toggles_matching_servers_only() {
        let mut claude_json = json!({
            "mcpServers": {
                "GitHub Prod": {
                    "command": "github",
                    "disabled": true
                },
                "Figma Design": {
                    "command": "figma"
                },
                "Linear": {
                    "command": "linear",
                    "disabled": true
                }
            }
        });
        let config = McpConfig {
            enable: vec!["GitHub".to_string()],
            disable: vec!["Figma".to_string()],
        };

        apply_mcp_servers(&mut claude_json, &config).unwrap();

        let servers = claude_json["mcpServers"].as_object().unwrap();
        assert!(servers["GitHub Prod"].get("disabled").is_none());
        assert_eq!(servers["Figma Design"]["disabled"], Value::Bool(true));
        assert_eq!(servers["Linear"]["disabled"], Value::Bool(true));
    }

    #[test]
    fn apply_mcp_servers_is_noop_without_mcp_servers() {
        let original = json!({
            "version": 1
        });
        let mut claude_json = original.clone();
        let config = McpConfig {
            enable: vec!["GitHub".to_string()],
            disable: vec!["Figma".to_string()],
        };

        apply_mcp_servers(&mut claude_json, &config).unwrap();

        assert_eq!(claude_json, original);
    }

    #[test]
    fn apply_connectors_adds_and_removes_from_disabled_list() {
        let mut claude_json = json!({
            "claudeAiMcpEverConnected": { "Figma": true, "Notion": true },
            "projects": {
                "/repo": { "disabledMcpServers": ["Notion"] }
            }
        });
        let config = McpConfig {
            // Enable Notion (remove from disabled), disable Figma (add).
            enable: vec!["Notion".to_string()],
            disable: vec!["Figma".to_string()],
        };

        apply_connectors(&mut claude_json, "/repo", &config).unwrap();

        let disabled = claude_json["projects"]["/repo"]["disabledMcpServers"]
            .as_array()
            .unwrap();
        let names: Vec<&str> = disabled.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(names, vec!["Figma"]);
    }

    #[test]
    fn apply_connectors_creates_project_entry_when_absent() {
        let mut claude_json = json!({
            "claudeAiMcpEverConnected": { "Figma": true }
        });
        let config = McpConfig {
            enable: vec![],
            disable: vec!["Figma".to_string()],
        };

        apply_connectors(&mut claude_json, "/new", &config).unwrap();

        let names: Vec<&str> = claude_json["projects"]["/new"]["disabledMcpServers"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(names, vec!["Figma"]);
    }

    #[test]
    fn known_mcp_names_unions_all_three_sources() {
        let claude_json = json!({
            "mcpServers": { "GitHub": {} },
            "claudeAiMcpEverConnected": { "Figma": true },
            "projects": {
                "/repo": { "disabledMcpServers": ["Linear"] }
            }
        });

        let names = known_mcp_names(&claude_json, Some("/repo"));
        assert_eq!(names, vec!["Figma", "GitHub", "Linear"]);

        let global = known_mcp_names(&claude_json, None);
        assert_eq!(global, vec!["Figma", "GitHub"]);
    }

    #[cfg(unix)]
    #[test]
    fn connector_ops_reconcile_canonical_key_to_claude_codes_literal_key() {
        // Claude Code keys a project by the literal path the user cd'd into; cc-persona
        // canonicalizes the cwd. When they diverge (a symlinked dir, or macOS
        // /tmp -> /private/tmp), every connector read AND write must reconcile to the
        // same literal key — otherwise backup/restore/dirty operate on a phantom key.
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("proj");
        std::fs::create_dir_all(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let literal_key = link.to_string_lossy().into_owned();

        // Claude Code recorded the project under the SYMLINKED (literal) path.
        let mut projects = serde_json::Map::new();
        projects.insert(literal_key.clone(), json!({ "disabledMcpServers": ["Figma"] }));
        let json = json!({ "projects": Value::Object(projects) });

        // cc-persona's canonical key is the other path; it must map back to the literal.
        let canonical = real.to_string_lossy().into_owned();
        assert_ne!(canonical, literal_key);
        assert_eq!(resolve_project_key(&json, Path::new(&canonical)), literal_key);

        // Reads reconcile internally: the canonical key sees the literal entry's list.
        assert_eq!(
            read_project_disabled(&json, &canonical),
            Some(vec!["Figma".to_string()])
        );

        // Writes target the literal entry — no duplicate canonical-key entry appears.
        let mut json2 = json.clone();
        set_project_disabled(
            &mut json2,
            &canonical,
            Some(&["Figma".to_string(), "Notion".to_string()]),
        )
        .unwrap();
        let projects_obj = json2["projects"].as_object().unwrap();
        assert!(projects_obj.contains_key(&literal_key));
        assert!(!projects_obj.contains_key(&canonical));
        let names: Vec<&str> = projects_obj[&literal_key]["disabledMcpServers"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(names, vec!["Figma", "Notion"]);
    }

    #[test]
    fn all_known_mcp_names_includes_project_disabled_connectors() {
        let claude_json = json!({
            "mcpServers": { "GitHub": {} },
            "claudeAiMcpEverConnected": { "Figma": true },
            "projects": {
                "/repo": { "disabledMcpServers": ["Linear"] }
            }
        });
        // Linear lives ONLY under a project's disabledMcpServers, yet a global-scope
        // caller (doctor) must still see it to avoid a false "no match" warning.
        assert_eq!(
            all_known_mcp_names(&claude_json),
            vec!["Figma", "GitHub", "Linear"]
        );
    }

    #[test]
    fn write_claude_json_round_trips_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".claude.json");
        let value = json!({ "mcpServers": { "GitHub": { "command": "gh" } } });

        write_claude_json(&path, &value).unwrap();
        let read_back = read_claude_json(&path).unwrap();

        assert_eq!(read_back, value);
        // No temp file left behind.
        assert!(!dir.path().join(".claude.json.cc-persona.tmp").exists());
    }

    #[test]
    fn update_claude_json_applies_under_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".claude.json");
        let lock = dir.path().join("claude-json.lock");
        write_claude_json(&path, &json!({ "mcpServers": {} })).unwrap();

        update_claude_json(&path, &lock, |json| {
            json["mcpServers"]["GitHub"] = json!({ "command": "gh" });
            Ok(())
        })
        .unwrap();

        let read_back = read_claude_json(&path).unwrap();
        assert_eq!(read_back["mcpServers"]["GitHub"]["command"], json!("gh"));
        // Lock released.
        assert!(!lock.exists());
    }
}
