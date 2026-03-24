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
├── config.rs            # cc-persona's own config (~/.cc-persona/config.toml)
├── backup.rs            # Pre-switch backup and restore
├── symlink.rs           # Symlink utilities
├── claude/
│   ├── settings.rs      # ~/.claude/settings.json read/write
│   ├── mcp.rs           # ~/.claude.json MCP server toggling
│   ├── skills.rs        # Skills symlink switching + per-skill disable
│   └── claude_md.rs     # CLAUDE.md symlink switching
└── commands/
    ├── init.rs          # init + skill installation
    ├── use_cmd.rs       # persona switching
    ├── off.rs           # restore from backup
    ├── list.rs, which.rs, show.rs, diff.rs
    ├── create.rs, snap.rs, edit.rs
    └── skill.rs         # skill list/toggle/rm
```

## Areas where help is wanted

- **Windows support** — Replace `std::os::unix::fs::symlink` with cross-platform logic (symlinks or copy fallback). See the [Platform Support](README.md#platform-support) section.
- **Tests** — Unit and integration test coverage.
- **Shell completions** — Generate completions for bash/zsh/fish via clap.
