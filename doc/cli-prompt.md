# Goal

I want to create a configurable agentic ai cli written in rust.

# General specifications
- entire memory and configuration would be in a local redb database.
- llm agnostic.
- extendable via mcps defined in this database
- hold memory in the database
- allow communicating with other agents with a2a
- allow finding other agents on the local wifi network
- command to the client would be done with /{command}
- only enough characters to uniquely identify the command would be needed.
- as the user types in the command. the possible commands that would apply would show up beneath the user's typing.
- /bye - quit the agent. save the session if named session.
- /quit - quit the agent. do not save the session if named session
- ctrl+c, the session is treated as orphaned/ephemeral and deleted
- /help - show all commands

# LLM
- /model - allows for the selection of models
  - would interactively show a selection of models to chose from including xai, grok, anthropic and ollama models. the models should show the version and whether they are configured or not
  - entering a selection would check if that model is already configured.
  - if already configured that model would become the active model for the current session and all new sessions
  - if the model is not configured the user would be asked to enter the api key and then continue as usual from there
  - del key would remove the api key for the model and require that model to be reconfigured
  - esc to exit interaction

# Named sessions management
- upon start the tool simply starts a new session and deletes the session on exit
- /session -
  - lists active sessions interactively. one entry for all active sessions, one entry to create new session
  - del key would delete a session if cursor is on a named session
  - esc to exit interaction
  - if user selects new session, user would be prompted for the name of the session.
    - new persisted session would be created with that name.

# Agent identity
- /identity - would allow the agent to identify itself via oauth token from a provider (github, google, etc.)
- the identity would be scoped globally
- this should have a device flow opening a browser to handle the direct locally on a loopback port

# A2A
- /marco - toggles allowance for this agent to advertise themselves passively. the command should print the active state of this allowance.
- /polo -
  - would list interactively:
    - 1. other agents on the wifi network that upon selecting would request user to enter a name for that session.
      - Upon selection the user would be required to enter the sessions name
      - Afterwords a new session connected to the other agent would be created whereby all tool calls would be relayed to the other agent. Regular messages are processed locally.
    - 2. sessions connected to other agents with their hostname, ip address, model and another entry to create a new session.
      - selecting existing session would activate that session.
      - del key would delete that session.
    - esc to exit interactive selection

# MCP Transport
- stdio
- /mcp - to interactively list configured mpcs and an entry to create a new entry
  - 1. existing mcp entry
    - selecting an mcp does nothing.
    - del key deletes the mcp
  - 2. new entry
    - user would be prompted to define input flow of:
      - {name} {command} {args?} {env args in the form of [key]=[value]?}

# Builtin MCPs
- `run_shell_command` — allows the LLM to propose shell commands; user is shown the command and reason and must confirm before execution. Commands are run directly (no shell interpolation) for safety.
- prefix a message with `[yolo]` to auto-approve all shell commands for that exchange without confirmation prompts.

# OpenAI-Compat APIs
- support any LLM provider that exposes an OpenAI-compatible chat completions endpoint.
- provider endpoint and api key are configured via /model and stored in the database.

# Shell access
- prefix input with `!` to run a shell command directly (e.g. `!ls -la`). output appears in the chat window.
