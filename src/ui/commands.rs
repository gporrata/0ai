/// All supported commands with their full names
pub const COMMANDS: &[(&str, &str)] = &[
    ("bye", "Quit (saves named session)"),
    ("quit", "Quit (discards named session)"),
    ("help", "Show all commands"),
    ("model", "Interactive model selection"),
    ("session", "Interactive session management"),
    ("identity", "Set OAuth identity"),
    ("marco", "Toggle mDNS advertising"),
    ("polo", "Interactive agent discovery / sessions"),
    ("mcp", "Manage MCP servers"),
];

/// Given a partial command string (without /), return matching commands.
pub fn autocomplete(partial: &str) -> Vec<(&'static str, &'static str)> {
    if partial.is_empty() {
        return COMMANDS.to_vec();
    }
    COMMANDS
        .iter()
        .filter(|(cmd, _)| cmd.starts_with(partial))
        .copied()
        .collect()
}

/// Find a unique command match. Returns the full command name if there is
/// exactly one prefix match, otherwise None.
pub fn resolve_command(partial: &str) -> Option<&'static str> {
    let matches: Vec<&str> = COMMANDS
        .iter()
        .filter(|(cmd, _)| cmd.starts_with(partial))
        .map(|(cmd, _)| *cmd)
        .collect();
    if matches.len() == 1 {
        Some(matches[0])
    } else {
        None
    }
}

pub enum ParsedInput {
    Command(String, String), // (command, rest_of_line)
    Shell(String),           // !<shell command>
    Message(String),
    Empty,
}

pub fn parse_input(input: &str) -> ParsedInput {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return ParsedInput::Empty;
    }
    if trimmed.starts_with('!') {
        let cmd = trimmed[1..].trim().to_string();
        return if cmd.is_empty() { ParsedInput::Empty } else { ParsedInput::Shell(cmd) };
    }
    if trimmed.starts_with('/') {
        let without_slash = &trimmed[1..];
        let (cmd_part, rest) = match without_slash.find(char::is_whitespace) {
            Some(pos) => (&without_slash[..pos], without_slash[pos..].trim().to_string()),
            None => (without_slash, String::new()),
        };
        ParsedInput::Command(cmd_part.to_lowercase(), rest)
    } else {
        ParsedInput::Message(trimmed.to_string())
    }
}
