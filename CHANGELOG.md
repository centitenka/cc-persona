# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

## [0.3.0] - 2026-06-08

### Added

- **Project-scoped personas** — `cc-persona use <name> --project` applies to `<cwd>/.claude/` (`settings.local.json` + `skills/`) instead of globally, so multiple Claude Code windows on different projects can hold different personas at once. `--project` is accepted by `use`, `off`, `which`, `show`, `diff`, and `snap`; bindings are recorded per directory in `config.projects`
- `cc-persona prune` — remove stale project bindings whose directories no longer exist
- **Experimental window scope** — `cc-persona shell <name>` launches Claude Code against an isolated, persona-scoped config dir via the undocumented `CLAUDE_CONFIG_DIR` (labeled experimental and version-fragile)
- **Three-source MCP** — a persona's `[mcp].enable`/`disable` patterns now match across top-level `mcpServers`, plugin-provided MCP (`enabledPlugins`), and claude.ai connectors (`projects.<cwd>.disabledMcpServers`), instead of only the often-empty top-level map
- `cc-persona doctor` gained a **Projects** section (enumerates bindings, flags stale/orphan state, suggests `prune`) and **three-source MCP coverage**, including a warning for any `[mcp]` pattern that matches nothing (the previous silent no-op)

### Changed

- All writes to the shared `~/.claude.json` now go through an advisory lock + atomic temp-file rename, so concurrent windows cannot lose each other's updates or corrupt the file
- Per-scope dirty detection and backups: a dirty project no longer blocks a global switch (and vice-versa); `off --project` honors **delete-on-restore**, removing files cc-persona created rather than leaving empty `.claude/` husks
- Project scope auto-adds a `<cwd>/.claude/.gitignore` for `settings.local.json` and `skills/` (machine-local, they embed `$HOME` in symlinks)
- Global scope now honors `$CLAUDE_CONFIG_DIR` when resolving Claude Code's config location (previously hard-coded `~/.claude`), so cc-persona targets wherever Claude Code actually reads config. This is what the experimental window scope is built on; if you export `CLAUDE_CONFIG_DIR`, cc-persona now follows it

### Notes

- **CLAUDE.md stays user-level only.** A persona's `[claude_md]` is applied at global scope and deliberately never touched at project scope (project CLAUDE.md merges with user-level and is often git-tracked)
- **`skill toggle` remains global.** A skill lives once in the shared store, so muting it affects every scope; use a persona's `active` list for per-scope inclusion

## [0.2.1] - 2026-06-06

### Changed

- Post-switch guidance now points to `/reload-skills` (and `/reload-plugins` when plugins changed) instead of a full session restart — updated in the built-in skill reminder, `migrate`, `adopt`, and the README. MCP and `settings.json` changes may still need a session restart.

## [0.2.0] - 2026-06-06

### Added

- Shared **skill store** (`~/.cc-persona/skill-store/`) with per-skill symlinks into `~/.claude/skills/` — one physical copy per skill, shared across personas
- `cc-persona doctor` — diagnose managed / untracked / drift / ghost state for skills, plugins, and MCP servers
- `cc-persona adopt` — bring untracked skills (installed by gstack, `skills.sh`, etc.) under management and into a persona
- `cc-persona migrate` — one-time migration from the v0.1.x whole-directory symlink layout to the skill store
- Reconcile-on-switch: `use` relinks exactly a persona's `active` skills and reports any untracked ones found in `~/.claude/skills/`

### Changed

- `~/.claude/skills/` is now a **real directory** of per-skill symlinks, never a whole-directory symlink to a persona — external installers can no longer silently override a persona's `active` list
- A persona's `active` list is now the exact set of linked skills (a link means enabled); `disable-model-invocation` becomes an optional global mute
- Backups record skill links as a manifest (`skills-links.json`); restore rebuilds managed links and leaves untracked directories untouched
- Dirty detection now reads managed links, so untracked directories no longer trigger false "unsaved changes"

### Migration

- Run `cc-persona migrate` once after upgrading to move existing skills into the store and rebuild links from your active persona

## [0.1.1] - 2026-03-25

### Added

- Dirty-persona guard for `use` and `off` with explicit `--save-current` / `--discard-current` flow
- Active persona state snapshot tracking under `~/.cc-persona/` for materialized dirty detection
- Save-current persistence for live settings, skill activation, MCP state, and CLAUDE.md content

### Changed

- Built-in `cc-persona` skill now explains the save/discard flow and tells Claude to ask before discarding
- Documentation now covers dirty persona handling and the new CLI flags

## [0.1.0] - 2026-03-25

### Added

- Persona management: `create`, `snap`, `edit`, `list`, `show`, `diff`, `which`
- Persona switching: `use` (with interactive select) and `off` (restore from backup)
- Base + override inheritance model for persona definitions
- Settings.json deep merge overlay
- Skills directory symlink switching with per-skill `disable-model-invocation` toggle
- MCP server enable/disable via `~/.claude.json`
- CLAUDE.md symlink switching
- Skill subcommands: `skill list`, `skill toggle`, `skill rm`
- Auto-backup before every switch
- Auto-installed Claude Code skill for natural language triggering
- `init` command for first-time setup
