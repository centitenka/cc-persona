use anyhow::{Result, bail};

use crate::claude::skills as skill_ops;
use crate::claude::{mcp, settings};
use crate::commands::init;
use crate::config::Paths;
use crate::persona::{self, ClaudeMdConfig, McpConfig, Persona, SkillsConfig};

pub fn run(paths: &Paths, name: Option<String>) -> Result<()> {
    let persona_name = name.unwrap_or_else(|| {
        chrono::Local::now()
            .format("snap-%Y%m%d-%H%M%S")
            .to_string()
    });

    let persona_file = persona::persona_path(&paths.personas, &persona_name);
    if persona_file.exists() {
        bail!("Persona '{}' already exists.", persona_name);
    }

    // Snapshot settings.json
    let current_settings = settings::read_settings(&paths.claude_settings)?;

    // Snapshot skills
    let skills_dir = if paths.claude_skills.is_symlink() {
        std::fs::read_link(&paths.claude_skills)?
    } else {
        paths.claude_skills.clone()
    };
    let skill_list = skill_ops::list_skills(&skills_dir)?;
    let active_skills: Vec<String> = skill_list
        .iter()
        .filter(|(_, disabled)| !disabled)
        .map(|(name, _)| name.clone())
        .collect();

    // Snapshot MCP
    let mcp_servers = mcp::list_mcp_servers(&paths.claude_json)?;
    let enabled_mcp: Vec<String> = mcp_servers
        .iter()
        .filter(|(_, disabled)| !disabled)
        .map(|(name, _)| name.clone())
        .collect();
    let disabled_mcp: Vec<String> = mcp_servers
        .iter()
        .filter(|(_, disabled)| *disabled)
        .map(|(name, _)| name.clone())
        .collect();

    // Create persona
    let persona = Persona {
        name: persona_name.clone(),
        description: format!(
            "Snapshot taken at {}",
            chrono::Local::now().format("%Y-%m-%d %H:%M")
        ),
        base: None,
        settings: Some(current_settings),
        skills: Some(SkillsConfig {
            active: active_skills,
        }),
        mcp: Some(McpConfig {
            enable: enabled_mcp,
            disable: disabled_mcp,
        }),
        claude_md: Some(ClaudeMdConfig {
            file: Some(format!("{}.md", persona_name)),
        }),
    };

    paths.ensure_dirs()?;
    persona.save(&persona_file)?;

    // Copy current skills into persona skill-set
    let target_skill_set = paths.skill_sets.join(&persona_name);
    std::fs::create_dir_all(&target_skill_set)?;
    if skills_dir.exists() && skills_dir.is_dir() {
        copy_dir_contents(&skills_dir, &target_skill_set)?;
    }
    // Ensure cc-persona skill is always present
    init::install_skill(&target_skill_set.join("cc-persona"))?;

    // Copy current CLAUDE.md
    let md_target = paths.claude_md.join(format!("{}.md", persona_name));
    if paths.claude_md_file.exists() {
        // Resolve symlink to get actual content
        let content = std::fs::read_to_string(&paths.claude_md_file).unwrap_or_default();
        std::fs::write(&md_target, content)?;
    } else {
        std::fs::write(&md_target, "")?;
    }

    eprintln!("✓ Snapped current config as persona '{}'", persona_name);
    eprintln!("  File: {}", persona_file.display());
    Ok(())
}

fn copy_dir_contents(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            std::fs::create_dir_all(&dst_path)?;
            copy_dir_contents(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TestEnv;
    use serde_json::json;

    #[cfg(unix)]
    #[test]
    fn run_captures_current_configuration_into_persona_assets() {
        let env = TestEnv::new();
        env.paths.ensure_dirs().unwrap();

        env.write_file(
            &env.paths.claude_settings,
            &json!({
                "model": "claude-sonnet",
                "ui": {
                    "theme": "light"
                }
            })
            .to_string(),
        );
        env.write_file(
            &env.paths.claude_json,
            &json!({
                "mcpServers": {
                    "GitHub": {
                        "command": "github"
                    },
                    "Figma": {
                        "command": "figma",
                        "disabled": true
                    }
                }
            })
            .to_string(),
        );
        env.write_file(&env.paths.claude_md_file, "current claude instructions");

        let source_skills = env.paths.root.join("source-skills");
        std::fs::create_dir_all(&source_skills).unwrap();
        env.create_skill(&source_skills, "alpha", "---\nname: alpha\n---\n");
        env.create_skill(
            &source_skills,
            "beta",
            "---\nname: beta\ndisable-model-invocation: true\n---\n",
        );
        env.symlink(&source_skills, &env.paths.claude_skills);

        run(&env.paths, Some("engineer".to_string())).unwrap();

        let persona_file = persona::persona_path(&env.paths.personas, "engineer");
        assert!(persona_file.exists());

        let snapped = Persona::load(&persona_file).unwrap();
        assert_eq!(snapped.name, "engineer");
        assert_eq!(
            snapped.settings,
            Some(json!({
                "model": "claude-sonnet",
                "ui": {
                    "theme": "light"
                }
            }))
        );
        assert_eq!(snapped.skills.unwrap().active, vec!["alpha"]);
        let mcp = snapped.mcp.unwrap();
        assert_eq!(mcp.enable, vec!["GitHub"]);
        assert_eq!(mcp.disable, vec!["Figma"]);
        assert_eq!(
            snapped.claude_md.unwrap().file.as_deref(),
            Some("engineer.md")
        );

        let snapped_skill_set = env.paths.skill_sets.join("engineer");
        assert!(snapped_skill_set.join("alpha").join("SKILL.md").exists());
        assert!(snapped_skill_set.join("beta").join("SKILL.md").exists());
        assert!(
            snapped_skill_set
                .join("cc-persona")
                .join("SKILL.md")
                .exists()
        );
        assert_eq!(
            env.read_file(&env.paths.claude_md.join("engineer.md")),
            "current claude instructions"
        );
    }
}
