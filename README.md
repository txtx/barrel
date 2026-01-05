# barrel

A CLI for AI-assisted development. Portable agents across LLMs. Reproducible terminal workspaces.

## The Problem

You're deep in a debugging session with Claude when you hear Codex just shipped something interesting. But switching means reconfiguring everything—your prompts, your agents, your carefully crafted system instructions scattered across `.claude/`, `.codex/`, and whatever directory the next tool invents.

Meanwhile, your terminal is a mess. Three projects, five tabs, that one background process you forgot about.

**barrel fixes both.**

## Features

### Agent Portability

Write your agents once. barrel symlinks them wherever they need to go.

```
agents/
  code-reviewer.md
  frontend-engineer.md
  security-auditor.md
```

Switch LLMs by changing one line. No more copy-pasting between `.claude/agents/` and `.codex/agents/`.

### Reproducible Workspaces

One command. Your entire workspace materializes.

```yaml
# barrel.yaml
workspace: myproject

shells:
  - type: claude
    agents: ["*"]
  - type: shell
    path: ./backend
  - type: shell
    path: ./frontend

terminal:
  profiles:
    default:
      type: tmux
      claude:
        col: 0
        row: 0
      backend:
        col: 1
        row: 0
      frontend:
        col: 1
        row: 1
```

```bash
barrel
```

Claude on the left, servers on the right. Close everything, come back tomorrow, run `barrel` again—exactly where you left off.

## Installation

```bash
curl -sL https://install.barrel.rs | bash
```

Or build from source:

```bash
cargo barrel-install
```

### Prerequisites

- [tmux](https://github.com/tmux/tmux) for workspace management
- One or more AI coding assistants (Claude Code, Codex, OpenCode)

## Quick Start

```bash
# Initialize a workspace in current directory
barrel init

# Import your existing agents
barrel agent import ./.claude/agents/web-developer.md
barrel agent import ./agents/

# Launch a single AI shell (agents are symlinked automatically)
barrel claude

# Or launch the full workspace
barrel
```

> **Note:** `barrel bootstrap` exists to auto-discover agents across your machine, but it's experimental. We recommend manually importing agents with `barrel agent import <file|dir>` for more control.

## Usage

```bash
# Setup
barrel init                     # Create barrel.yaml in current directory
barrel bootstrap                # [Experimental] Auto-discover agents

# Launching
barrel                          # Launch workspace from ./barrel.yaml
barrel claude                   # Launch just Claude with agents
barrel codex                    # Launch just Codex with agents
barrel <shell>                  # Launch a specific shell from barrel.yaml
barrel -m path/to/barrel.yaml   # Launch from specific manifest
barrel -p <profile>             # Use a specific terminal profile

# Management
barrel -k <workspace>           # Kill workspace and clean up agents
barrel -k <workspace> --keep-agents  # Kill but preserve agent symlinks

# Agents
barrel agent list               # List all agents (local + global)
barrel agent import <path>      # Import agent file or directory
barrel agent new [name]         # Create a new agent
barrel agent fork <name>        # Copy a global agent locally
barrel agent link <name>        # Symlink a global agent locally
barrel agent rm <name>          # Remove an agent
```

## Configuration

### Shells

Define what runs in each pane:

```yaml
shells:
  # AI assistants
  - type: claude
    agents: ["code-reviewer", "frontend-engineer"]
    model: claude-sonnet-4-20250514

  - type: codex
    agents: ["*"]  # All agents

  # Custom commands
  - type: shell
    path: ./backend
    command: npm run dev

  - type: shell
    path: ./frontend
    command: pnpm dev
```

### Profiles

Define terminal layouts:

```yaml
terminal:
  profiles:
    default:
      type: tmux  # tmux, tmux_cc (iTerm2), or shell
      claude:
        col: 0
        row: 0
      backend:
        col: 1
        row: 0
        height: 50
      frontend:
        col: 1
        row: 1
```

### Agents

Centralize your agents:

```yaml
agents:
  - path: ./agents           # Local agents
  - path: ~/.config/barrel/agents  # Global agents
```

## For Developers Who

- Run multiple AI coding assistants and are tired of maintaining separate configs
- Work across several repositories that need to be running simultaneously
- Want reproducible dev environments they can spin up instantly
- Believe their tooling should adapt to them, not the other way around

## License

MIT
