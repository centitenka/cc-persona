<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://readme-typing-svg.demolab.com?font=Bitcount+Grid+Double+Ink&weight=300&size=100&duration=1600&pause=1000&color=E6EDF3&center=true&vCenter=true&repeat=false&width=1100&height=130&lines=CC+Persona">
    <img src="https://readme-typing-svg.demolab.com?font=Bitcount+Grid+Double+Ink&weight=300&size=100&duration=1600&pause=1000&color=1F2328&center=true&vCenter=true&repeat=false&width=1100&height=130&lines=CC+Persona" alt="CC Persona">
  </picture>
  <br/>
  <strong>Instant persona switching for Claude Code.</strong><br/>
  <sub>One CLI. One skill. Every configuration, switched in a blink, with fewer wasted tool calls and tokens.</sub>
</p>

<p align="center">
  <a href="https://github.com/centitenka/cc-persona/actions/workflows/ci.yml"><img src="https://github.com/centitenka/cc-persona/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="License"></a>
  <a href="https://www.rust-lang.org"><img src="https://img.shields.io/badge/rust-stable-orange?style=flat-square&logo=rust" alt="Rust"></a>
</p>

---

## See it in action

```
$ cc-persona snap engineer
  Snapped current config as persona 'engineer'

$ cc-persona create designer --base plain
  Created persona 'designer'

$ cc-persona list
    designer — UI/UX design mode
    engineer * — Software engineering mode
    plain — Clean slate

$ cc-persona use designer
  Backed up current config
  Applied settings.json overrides
  Switched skills -> designer
  Applied MCP server toggles
  Switched CLAUDE.md
  Switched to persona: designer

$ cc-persona use engineer
  Error: Current persona 'designer' has unsaved changes.
  Re-run `cc-persona use engineer --save-current` (recommended)
  or `cc-persona use engineer --discard-current`.

# Or just tell Claude:
> "switch to engineer"
  Claude runs `cc-persona use engineer` via the built-in skill.
```

## What is this

Claude Code scatters configuration across four places: `settings.json`, skills directories, MCP server configs, and `CLAUDE.md`. CC Persona treats these as a single switchable unit called a **persona** — a named preset you define once and activate instantly.

That is not just a convenience win. Matching the agent to the task also cuts waste: the wrong persona can trigger irrelevant tools and skills, which hurts task quality and burns tokens on work that should never have started. CC Persona helps keep the active environment aligned with the job so Claude spends more tokens on the right work and fewer on accidental detours.

## Architecture

### Overlay inheritance

Personas use a **base + override** model. Define a `plain` base, then derive `engineer` and `designer` from it — each only declares what differs. Settings are deep-merged; skills, MCP, and CLAUDE.md override entirely. No duplication.

```
plain (base) ─┬─ engineer (overrides model, skills, MCP)
              └─ designer (overrides skills, CLAUDE.md)
```

### Symlink-based instant switching

Skills and CLAUDE.md switch via symlinks — zero copy, zero latency. `~/.claude/skills` is a symlink that points to the active persona's skill-set directory. Switching personas just re-points the link.

### CLI x Skill

CC Persona ships as two halves that work together:

- **The CLI** is the engine — it reads personas, manages symlinks, patches JSON, and handles backups.
- **The Skill** is the interface — a Claude Code skill installed at `~/.claude/skills/cc-persona/` that lets Claude invoke the CLI through natural language.

Say "switch to engineer" in a session, and Claude calls `cc-persona use engineer` for you. The CLI does the heavy lifting; the skill makes it seamless. This **CLI x Skill architecture** means the tool works both from your terminal and from inside Claude conversations.

If the current persona is dirty, the CLI will block the switch and require an explicit choice. The built-in skill should ask whether to save first, recommend saving, then rerun with `--save-current` or `--discard-current`.

### Safe by default

Every switch creates a timestamped backup. Run `cc-persona off` to restore the exact state before the last switch. No configuration is ever lost.

If the active persona has changed since it was last applied, `cc-persona use` and `cc-persona off` stop and require one of these explicit follow-ups:

- `--save-current` — persist the current persona before continuing
- `--discard-current` — continue without saving current persona changes

This keeps persona files from drifting silently when users install skills, toggle skills, or tweak Claude config during a session.

## How it works

| Config | File | Switch mechanism |
|--------|------|-----------------|
| Settings | `~/.claude/settings.json` | Deep JSON merge overlay |
| Skills | `~/.claude/skills/` | Symlink to persona's skill-set |
| MCP servers | `~/.claude.json` | Toggle `disabled` per server |
| CLAUDE.md | `~/.claude/CLAUDE.md` | Symlink to persona-specific file |

Within a skill-set, individual skills can be toggled on/off via `disable-model-invocation` — giving you per-skill control inside each persona.

## Install

CC Persona currently targets Rust stable with edition 2024 semantics. The minimum supported Rust version is **1.85**.

From a local checkout:

```bash
cargo install --path .
```

For day-to-day development without copying into Cargo's bin dir:

```bash
cargo run -- init
```

## Quick start

```bash
# Install from a local checkout
cargo install --path .

# Initialize — creates ~/.cc-persona/ and installs the Claude Code skill
cc-persona init

# Snapshot your current config as a persona
cc-persona snap engineer

# Switch
cc-persona use engineer
```

> After switching, restart your Claude Code session (`/exit` + relaunch) for all changes to take effect.

## Safety and disk changes

CC Persona intentionally edits files in your home directory so persona switches are instant:

- `~/.claude/settings.json`
- `~/.claude/skills`
- `~/.claude/CLAUDE.md`
- `~/.claude.json`
- `~/.cc-persona/` for personas, per-persona assets, and backups

Before every `cc-persona use`, the tool creates a timestamped backup under `~/.cc-persona/backups/`.

- `cc-persona off` restores the most recent pre-switch `settings.json`, `~/.claude.json`, and `CLAUDE.md`
- If `~/.claude/skills` was already a symlink, `off` restores that symlink target
- If `~/.claude/skills` started as a real directory, the first switch migrates its contents into cc-persona management and future restores keep using the managed skill-set symlink
- If the active persona is dirty, `use` and `off` stop until you rerun with `--save-current` or `--discard-current`

For safety, CC Persona refuses to delete a real directory when a symlink replacement would be destructive. If a path must be moved manually, the CLI stops and tells you what to do.

## CLI reference

```
cc-persona init                    Initialize and install Claude Code skill
cc-persona list                    List all personas (* = active)
cc-persona use [name]              Switch persona (interactive if omitted)
cc-persona use [name] --save-current
                                   Save current persona before switching
cc-persona use [name] --discard-current
                                   Discard current persona changes before switching
cc-persona off                     Restore pre-switch config from backup
cc-persona off --save-current      Save current persona before restoring
cc-persona off --discard-current   Discard current persona changes before restoring
cc-persona create <name>           Create a new persona interactively
cc-persona snap [name]             Snapshot current config into a persona
cc-persona edit <name>             Open persona TOML in $EDITOR
cc-persona show [name]             Print resolved config (with inheritance)
cc-persona diff [name]             Diff current config vs persona
cc-persona which                   Print active persona name

cc-persona skill list              List skills with ON/OFF status
cc-persona skill toggle <name>     Toggle skill activation
cc-persona skill rm <name>         Delete a skill permanently
```

## Persona format

TOML files in `~/.cc-persona/personas/`. Inherit with `base`.

```toml
name = "engineer"
description = "Software engineering mode"
base = "plain"

[settings]
language = "English"
model = "claude-opus-4-6"
outputStyle = "Concise"

[skills]
active = ["issue-creator", "vibe-explainer"]

[mcp]
enable = ["GitHub", "Linear"]
disable = ["Figma", "Playwright"]

[claude_md]
file = "engineer.md"
```

## Directory layout

```
~/.cc-persona/
├── config.toml              # tracks active persona
├── active-persona-state.json# last applied materialized persona state
├── personas/                # persona definitions (TOML)
├── skill-sets/              # per-persona skill directories
│   ├── plain/
│   │   └── cc-persona/      # auto-injected, always present
│   ├── engineer/
│   │   ├── cc-persona/
│   │   ├── issue-creator/
│   │   └── vibe-explainer/
│   └── designer/
├── claude-md/               # per-persona CLAUDE.md files
└── backups/                 # timestamped pre-switch snapshots
```

## Platform support

macOS and Linux. Windows support is planned — contributions welcome ([details](CONTRIBUTING.md)).

## License

[MIT](LICENSE)
