#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Quit, NewSession { title: Option<String> }, SwitchSession { id: String }, ListSessions,
    SelectModel { id: Option<String> }, Clear, Help, Connect { url: String },
    Auth { token: String }, Tokens, Retry, Copy,
    Dump { filename: Option<String> }, Compact, Rename { title: String },
    Tree, Fork { message_id: Option<String> }, Settings,
    Export { filename: Option<String> }, Import { filename: String },
    DeleteSession, SystemPrompt { prompt: String },
    Skill { name: String },
}

impl Command {
    pub fn parse(input: &str) -> Option<Self> {
        let input = input.trim();
        if !input.starts_with('/') { return None; }
        let (cmd, args) = match input[1..].split_once(char::is_whitespace) {
            Some((c, a)) => (c, a.trim()),
            None => (&input[1..], ""),
        };
        match cmd {
            "q" | "quit" => Some(Command::Quit),
            "new" => Some(Command::NewSession { title: if args.is_empty() { None } else { Some(args.to_string()) } }),
            "switch" if !args.is_empty() => Some(Command::SwitchSession { id: args.to_string() }),
            "list" => Some(Command::ListSessions),
            "model" => Some(Command::SelectModel { id: if args.is_empty() { None } else { Some(args.to_string()) } }),
            "clear" => Some(Command::Clear),
            "help" => Some(Command::Help),
            "connect" if !args.is_empty() => Some(Command::Connect { url: args.to_string() }),
            "auth" if !args.is_empty() => Some(Command::Auth { token: args.to_string() }),
            "tokens" => Some(Command::Tokens),
            "retry" => Some(Command::Retry),
            "copy" => Some(Command::Copy),
            "dump" => Some(Command::Dump { filename: if args.is_empty() { None } else { Some(args.to_string()) } }),
            "compact" => Some(Command::Compact),
            "rename" if !args.is_empty() => Some(Command::Rename { title: args.to_string() }),
            "tree" => Some(Command::Tree),
            "fork" => Some(Command::Fork { message_id: if args.is_empty() { None } else { Some(args.to_string()) } }),
            "settings" => Some(Command::Settings),
            "export" => Some(Command::Export { filename: if args.is_empty() { None } else { Some(args.to_string()) } }),
            "import" if !args.is_empty() => Some(Command::Import { filename: args.to_string() }),
            "delete" => Some(Command::DeleteSession),
            "system" if !args.is_empty() => Some(Command::SystemPrompt { prompt: args.to_string() }),
            "skill" if !args.is_empty() => Some(Command::Skill { name: args.to_string() }),
            _ => {
                // Support /skill:name format (no space between skill and name)
                if cmd.starts_with("skill:") {
                    Some(Command::Skill { name: cmd[6..].to_string() })
                } else {
                    None
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn test_quit() { assert_eq!(Command::parse("/quit"), Some(Command::Quit)); assert_eq!(Command::parse("/q"), Some(Command::Quit)); }
    #[test] fn test_new() { assert_eq!(Command::parse("/new test"), Some(Command::NewSession { title: Some("test".into()) })); }
    #[test] fn test_switch() { assert_eq!(Command::parse("/switch abc"), Some(Command::SwitchSession { id: "abc".into() })); }
    #[test] fn test_not_command() { assert_eq!(Command::parse("hello"), None); }
    #[test] fn test_help() { assert_eq!(Command::parse("/help"), Some(Command::Help)); }
    #[test] fn test_unknown() { assert_eq!(Command::parse("/unknown"), None); }
    #[test] fn test_connect() { assert_eq!(Command::parse("/connect http://x"), Some(Command::Connect { url: "http://x".into() })); }
    #[test] fn test_auth() { assert_eq!(Command::parse("/auth sk-t"), Some(Command::Auth { token: "sk-t".into() })); }
    #[test] fn test_retry() { assert_eq!(Command::parse("/retry"), Some(Command::Retry)); }
    #[test] fn test_copy() { assert_eq!(Command::parse("/copy"), Some(Command::Copy)); }
    #[test] fn test_dump() { assert_eq!(Command::parse("/dump out.md"), Some(Command::Dump { filename: Some("out.md".into()) })); }
    #[test] fn test_compact() { assert_eq!(Command::parse("/compact"), Some(Command::Compact)); }
    #[test] fn test_rename() { assert_eq!(Command::parse("/rename my session"), Some(Command::Rename { title: "my session".into() })); }
}
