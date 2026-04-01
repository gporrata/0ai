# 0ai

A Rust CLI implementing a configurable, agentic AI for orchestrating bots at home.

## Overview

- All memory and configuration is stored in a local [redb](https://github.com/cberner/redb) database.
- LLM-agnostic — supports any provider with an OpenAI-compatible API.
- Extensible via MCPs configured in the database.
- Supports agent-to-agent communication (A2A) and local network agent discovery.

## Commands

Commands are entered with a `/` prefix. Only enough characters to uniquely identify a command are required. As you type, matching commands appear beneath the input.

| Command | Description |
|---------|-------------|
| `/help` | Show all commands |
| `/bye` | Quit and save the session (if named) |
| `/quit` | Quit without saving the session |
| `Ctrl+C` | Treat the session as ephemeral and delete it |

## LLM Configuration

**`/model`** — Interactively select and configure a model.

- Shows available models from xAI (Grok), Anthropic, and Ollama, including their version and configuration status.
- Selecting an already-configured model activates it for the current and all future sessions.
- Selecting an unconfigured model prompts for an API key, then activates it.
- `Del` removes the API key for a model, requiring reconfiguration.
- `Esc` exits the interaction.

## Session Management

**`/session`** — Manage named sessions interactively.

- Lists all active named sessions, plus an option to create a new one.
- Selecting "new session" prompts for a name, then creates a persisted session.
- `Del` deletes the highlighted session.
- `Esc` exits the interaction.

**`/forget`** — Wipe all message history for the current session (in memory and in the database). The session itself is preserved.

By default, starting the tool creates an ephemeral session that is deleted on exit.

## Agent Identity

**`/identity`** — Authenticate via OAuth using a provider such as GitHub or Google.

- Uses device flow: opens a browser to handle authentication on a local loopback port.
- Identity is scoped globally across sessions.

## Agent-to-Agent (A2A)

**`/marco`** — Toggle passive advertisement of this agent on the local network. Prints the current state.

**`/polo`** — Interactively discover and manage connections to other agents.

- Lists agents discovered on the local Wi-Fi network. Selecting one prompts for a session name, then creates a session that relays tool calls to the remote agent (regular messages are processed locally).
- Lists existing sessions connected to other agents, showing hostname, IP address, and model.
- `Del` deletes the highlighted session.
- `Esc` exits the interaction.

## MCP Configuration

**`/mcp`** — Interactively list and manage configured MCPs.

- `Del` removes the highlighted MCP.
- Selecting "new entry" prompts for: `{name} {command} {args?} {KEY=VALUE env vars?}`

Supported transport: **stdio**.

### Built-in MCPs

- **`run_shell_command`** — Allows the LLM to propose shell commands. The command and its reasoning are shown to the user, who must confirm before execution. Commands run directly (no shell interpolation) for safety.
- Prefix a message with `[yolo]` to auto-approve all shell commands for that exchange without confirmation prompts.

## Shell Access

Prefix input with `!` to run a shell command directly (e.g. `!ls -la`). Output appears in the chat window.

## Prompt Customization

- The prompt character is colored `#03a1fc`. The default is `>` (plain ASCII).
- `/nerd` — Switch to a Nerd Font glyph (default `U+F083F`).
- `/nerd {hexcode}` — Set a specific Nerd Font glyph (e.g. `/nerd f0b0`).
- `/nonerd` — Revert to `>`.

The enabled state and chosen glyph persist across sessions.
