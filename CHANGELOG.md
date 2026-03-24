# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

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
