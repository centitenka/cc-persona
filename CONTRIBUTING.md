# Contributing to CC Persona

## Prerequisites

- [Rust toolchain](https://rustup.rs/) (stable)

## Build

```bash
cargo build
```

## Test

```bash
cargo test
```

## Code style

Format and lint before committing:

```bash
cargo fmt
cargo clippy
```

## Pull requests

1. Fork the repo and create a branch from `main`
2. Make your changes
3. Run `cargo fmt` and `cargo clippy` with no warnings
4. Open a PR with a clear description of what changed and how to test it

## Architecture

```
src/
├── main.rs              # Entry point + clap dispatch
├── cli.rs               # CLI subcommand definitions
├── persona.rs           # Persona data model + inheritance resolution
├── config.rs            # cc-persona's own config + path layout (incl. skill-store)
├── backup.rs            # Pre-switch backup and restore (skill-link manifest)
├── symlink.rs           # Symlink utilities (ensure_real_dir, link_skill, is_symlink)
├── diagnostics.rs       # Read-only drift / untracked / ghost inspection
├── claude/
│   ├── settings.rs      # ~/.claude/settings.json read/write
│   ├── mcp.rs           # ~/.claude.json MCP server toggling
│   ├── skills.rs        # Skill-store reconciliation + per-skill links
│   └── claude_md.rs     # CLAUDE.md symlink switching
└── commands/
    ├── init.rs          # init + skill installation
    ├── use_cmd.rs       # persona switching (reconcile skills)
    ├── off.rs           # restore from backup
    ├── doctor.rs        # diagnose managed / untracked / drift state
    ├── adopt.rs         # bring untracked skills under management
    ├── migrate.rs       # v0.1.x layout → skill-store migration
    ├── list.rs, which.rs, show.rs, diff.rs
    ├── create.rs, snap.rs, edit.rs
    └── skill.rs         # skill list/toggle/rm
```

The shared skill store lives at `~/.cc-persona/skill-store/`; `~/.claude/skills/` is a real
directory of per-skill symlinks into it. A symlink means cc-persona manages the skill; a real
subdirectory means it is untracked (installed by something else) and is left untouched.

## Releasing

The version in `Cargo.toml` is the source of truth. When cutting a release, keep these in sync:

1. `Cargo.toml` `version` — then run `cargo build` so `Cargo.lock` updates
2. `CHANGELOG.md` — add a dated section for the new version
3. `README.md` — CLI reference and any changed behavior
4. `src/cli.rs` — if the command set changed
5. Tag: `git tag vX.Y.Z && git push origin vX.Y.Z` (triggers `.github/workflows/release.yml`)
6. Publish: `cargo publish` (after `cargo publish --dry-run` passes)

## Areas where help is wanted

- **Windows support** — Replace `std::os::unix::fs::symlink` with cross-platform logic (symlinks or copy fallback). See the [Platform Support](README.md#platform-support) section.
- **Tests** — More integration coverage for edge cases and the migration path.
- **Shell completions** — Generate completions for bash/zsh/fish via clap.
