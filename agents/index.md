---
name: barrel
description: A CLI for AI-assisted development. Portable agents across LLMs. Reproducible terminal workspaces.
---

# barrel

## Overview

A CLI for AI-assisted development that provides portable agents across LLMs and reproducible terminal workspaces. Write agents once and use them with Claude Code, Codex, or OpenCode. Define workspace layouts in YAML and launch everything with a single command.

## Getting Started

```bash
# Install
curl -sL https://install.barrel.rs | bash
# Or build from source
cargo install --path crates/cli

# Initialize workspace
barrel init

# Import agents
barrel agent import ./agents/

# Launch
barrel          # Full workspace
barrel claude   # Just Claude with agents
```

Prerequisites: tmux, one or more AI assistants (Claude Code, Codex, OpenCode)

## Architecture

Rust workspace with two crates:
- `crates/cli` - CLI entry point, argument parsing (clap)
- `crates/core` - Core logic: config parsing, tmux session management, AI driver abstraction

Key abstractions:
- **Drivers** (`drivers/`) - Adapters for each AI assistant (Claude, Codex, OpenCode)
- **Tmux** (`tmux/`) - Session and pane management
- **Config** (`config.rs`) - barrel.yaml parsing and validation

## Key Files

- `Cargo.toml` - Workspace manifest
- `crates/cli/src/main.rs` - Entry point
- `crates/cli/src/cli.rs` - Clap argument definitions
- `crates/core/src/config.rs` - barrel.yaml schema
- `crates/core/src/drivers/` - AI assistant drivers
- `crates/core/src/tmux/` - Tmux integration

## Related repos

- **barrel-web-ui** (`~/Coding/barrel-web-ui`) - Website and documentation
  - Turborepo pnpm monorepo
  - `apps/www` - Marketing site (Next.js 16, React 19, Tailwind CSS 4)
  - `apps/docs` - Documentation site (Next.js 16, React 19, Tailwind CSS 4)
  - Commands: `pnpm dev:www`, `pnpm dev:docs`, `pnpm build`
