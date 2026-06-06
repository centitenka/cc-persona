use crate::config::Paths;
use std::path::Path;
use tempfile::TempDir;

pub(crate) struct TestEnv {
    _tempdir: TempDir,
    pub(crate) paths: Paths,
}

impl TestEnv {
    pub(crate) fn new() -> Self {
        let tempdir = tempfile::tempdir().expect("create tempdir");
        let home = tempdir.path().join("home");
        let root = home.join(".cc-persona");
        let claude_dir = home.join(".claude");

        std::fs::create_dir_all(&claude_dir).expect("create claude dir");

        let paths = Paths {
            root: root.clone(),
            config: root.join("config.toml"),
            active_persona_state: root.join("active-persona-state.json"),
            personas: root.join("personas"),
            skill_sets: root.join("skill-sets"),
            skill_store: root.join("skill-store"),
            claude_md: root.join("claude-md"),
            backups: root.join("backups"),
            claude_settings: claude_dir.join("settings.json"),
            claude_skills: claude_dir.join("skills"),
            claude_md_file: claude_dir.join("CLAUDE.md"),
            claude_json: home.join(".claude.json"),
        };

        Self {
            _tempdir: tempdir,
            paths,
        }
    }

    pub(crate) fn write_file(&self, path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dir");
        }
        std::fs::write(path, content).expect("write file");
    }

    pub(crate) fn read_file(&self, path: &Path) -> String {
        std::fs::read_to_string(path).expect("read file")
    }

    pub(crate) fn create_skill(&self, skills_dir: &Path, name: &str, content: &str) {
        let skill_dir = skills_dir.join(name);
        std::fs::create_dir_all(&skill_dir).expect("create skill dir");
        self.write_file(&skill_dir.join("SKILL.md"), content);
    }

    /// Create a skill directory inside the shared skill-store with a SKILL.md.
    pub(crate) fn create_store_skill(&self, name: &str, content: &str) {
        self.create_skill(&self.paths.skill_store, name, content);
    }

    /// Link a store skill into ~/.claude/skills as a directory symlink,
    /// mirroring the managed-link model.
    #[cfg(unix)]
    pub(crate) fn link_into_claude_skills(&self, name: &str) {
        let target = self.paths.skill_store.join(name);
        let link = self.paths.claude_skills.join(name);
        self.symlink(&target, &link);
    }

    #[cfg(unix)]
    pub(crate) fn symlink(&self, target: &Path, link: &Path) {
        if let Some(parent) = link.parent() {
            std::fs::create_dir_all(parent).expect("create symlink parent");
        }
        std::os::unix::fs::symlink(target, link).expect("create symlink");
    }
}
