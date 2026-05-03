#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Quit, NewSession { title: Option<String> }, SwitchSession { id: String }, ListSessions,
    SelectModel { id: Option<String> }, Clear, Help, Connect { url: String },
    Auth { token: String }, Tokens,
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
            _ => None,
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
}
