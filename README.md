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

### Skill store + per-skill links

`~/.claude/skills/` is a **real directory** that cc-persona keeps in sync — never a symlink to a persona. Every managed skill lives once in a shared **skill store** at `~/.cc-persona/skill-store/<skill>/`, and a persona switch is just a set of per-skill symlinks pointing from `~/.claude/skills/<skill>` back into the store.

```
~/.cc-persona/skill-store/
  ├── cc-persona/        # one physical copy per skill
  ├── issue-creator/
  └── design-review/

~/.claude/skills/        # a real directory
  ├── cc-persona  -> ~/.cc-persona/skill-store/cc-persona     (managed link)
  ├── issue-creator -> ~/.cc-persona/skill-store/issue-creator (managed link)
  └── some-tool/          # a real subdir = untracked (installed by something else)
```

This gives three properties the old whole-directory symlink could not:

- **One copy, shared everywhere.** A skill used by three personas is stored once, not copied three times. Cross-persona classification stops being a copy nightmare.
- **The filesystem is the source of truth for "managed vs wild."** A symlink into the store means cc-persona owns it; a real subdirectory means something else installed it (it is *untracked*, and left untouched).
- **A persona's `active` list is exact.** Switching personas relinks precisely the listed skills (plus the built-in `cc-persona` skill, always linked) and removes links that are no longer wanted — no silent drift when an installer drops a new skill into the directory.

CLAUDE.md still switches via a single symlink (`replace_with_symlink`) — zero copy, zero latency.

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

## Coexisting with the skill ecosystem

Your `~/.claude/skills/` is rarely empty or solely yours. Skill installers — gstack, `skills.sh`, plugin bundles, a `git clone` someone pasted — all drop directories straight into it. Most persona tools fight this; cc-persona is built for it.

Because `~/.claude/skills/` stays a **real directory** and cc-persona only ever owns **symlinks** inside it, the boundary is self-evident and unforgeable:

- A **symlink** into the skill store → cc-persona manages it.
- A **real subdirectory** → something else installed it. cc-persona calls it *untracked*, never touches it, and never lets it silently override your persona's `active` list.

`cc-persona doctor` shows exactly what is managed, untracked, drifted, or missing. When you want to bring a wild skill under management, `cc-persona adopt` moves it into the store and adds it to a persona — one command, no copy-paste:

```
$ cc-persona doctor
  ✗ 3 untracked skills in ~/.claude/skills (installed outside cc-persona)
      benchmark, qa, scrape
      Fix: cc-persona adopt --into build

$ cc-persona adopt benchmark qa --into build
  Adopted 2 skills into persona 'build'. Restart Claude Code to take effect.
```

Upgrading from v0.1.x (whole-directory symlink)? Run `cc-persona migrate` once to move every existing skill into the store and rebuild links from your active persona.

## How it works

| Config | File | Switch mechanism |
|--------|------|-----------------|
| Settings | `~/.claude/settings.json` | Deep JSON merge overlay |
| Skills | `~/.claude/skills/` | Per-skill symlinks into the shared skill store |
| MCP servers | `~/.claude.json` | Toggle `disabled` per server |
| CLAUDE.md | `~/.claude/CLAUDE.md` | Symlink to persona-specific file |

A persona's `active` list is the exact set of skills linked into `~/.claude/skills/` on switch. Anything cc-persona did not link stays a real directory and is left untouched — see [Coexisting with the skill ecosystem](#coexisting-with-the-skill-ecosystem).

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
- For skills, the backup records which managed links pointed where; `off` rebuilds those links and leaves untracked (real) skill directories untouched
- Upgrading from a v0.1.x layout? `cc-persona migrate` moves existing skills into the shared store and rebuilds links from your active persona
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
cc-persona doctor                  Diagnose managed / untracked / drift / ghost state
cc-persona adopt [names] --into <persona>
                                   Bring untracked skills under management
cc-persona migrate                 Migrate a v0.1.x layout into the skill store

cc-persona skill list              List skills with ON/OFF and managed/untracked status
cc-persona skill toggle <name>     Mute/unmute a skill (writes the shared store copy)
cc-persona skill rm <name>         Unlink a skill (optionally delete it from the store)
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
├── skill-store/             # one physical copy of every managed skill
│   ├── cc-persona/          # built-in, always linked
│   ├── issue-creator/
│   └── vibe-explainer/
├── claude-md/               # per-persona CLAUDE.md files
└── backups/                 # timestamped pre-switch snapshots
```

Each persona links the skills in its `active` list from this store into `~/.claude/skills/`; nothing is copied per persona.

## Known limitations

- **Switches need a Claude Code restart.** Skills, MCP, and settings load at session start. After any `cc-persona use` / `off`, run `/exit` and relaunch for changes to take effect.
- **`skill toggle` mutes the shared copy.** Because each skill lives once in the store, toggling `disable-model-invocation` affects every persona that links it, not just the active one. Use a persona's `active` list for per-persona inclusion; use `toggle` only for a global mute.
- **Windows is not yet supported.** Linking uses Unix symlinks; a copy-based fallback is planned. See [Platform support](#platform-support).

## Troubleshooting

- **I switched personas but nothing changed.** Restart Claude Code (`/exit` + relaunch) — config loads at session start.
- **Skills I didn't expect are active, or a newly installed skill isn't picked up.** Something installed skills outside cc-persona. Run `cc-persona doctor`, then `cc-persona adopt` the ones you want.
- **Upgrading from v0.1.x.** Run `cc-persona migrate` once to move existing skills into the store and rebuild links.
- **`use` or `off` refuses with "unsaved changes".** The active persona drifted since it was applied. Rerun with `--save-current` (keep the changes) or `--discard-current` (drop them).

## Platform support

macOS and Linux. Windows support is planned — contributions welcome ([details](CONTRIBUTING.md)).

## License

[MIT](LICENSE)
