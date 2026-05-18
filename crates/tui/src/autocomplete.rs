use std::path::PathBuf;
use std::process::Command;

pub trait AutocompleteProvider: Send + Sync {
    fn should_trigger(&self, context: &AutocompleteContext) -> bool;
    fn get_suggestions(&self, context: &AutocompleteContext) -> Vec<Suggestion>;
}

pub struct AutocompleteContext {
    pub full_text: String,
    pub cursor_line: usize,
    pub cursor_col: usize,
    pub current_line: String,
    pub text_before_cursor: String,
}

pub struct Suggestion {
    pub label: String,
    pub value: String,
    pub description: Option<String>,
}

pub struct SlashCommand {
    pub name: String,
    pub description: String,
}

pub struct SlashCommandProvider {
    commands: Vec<SlashCommand>,
}

impl SlashCommandProvider {
    pub fn new() -> Self {
        let commands = vec![
            SlashCommand { name: "quit".into(), description: "Exit the application".into() },
            SlashCommand { name: "new".into(), description: "Create a new session".into() },
            SlashCommand { name: "switch".into(), description: "Switch to another session".into() },
            SlashCommand { name: "list".into(), description: "List all sessions".into() },
            SlashCommand { name: "model".into(), description: "Select a model".into() },
            SlashCommand { name: "clear".into(), description: "Clear the current conversation".into() },
            SlashCommand { name: "connect".into(), description: "Connect to a server".into() },
            SlashCommand { name: "auth".into(), description: "Authenticate with the server".into() },
            SlashCommand { name: "tokens".into(), description: "Show token usage for this session".into() },
            SlashCommand { name: "help".into(), description: "Show help".into() },
            SlashCommand { name: "retry".into(), description: "Retry the last user message".into() },
            SlashCommand { name: "copy".into(), description: "Copy last assistant reply to clipboard".into() },
            SlashCommand { name: "dump".into(), description: "Export session to a Markdown file".into() },
            SlashCommand { name: "compact".into(), description: "Trigger context compaction".into() },
            SlashCommand { name: "rename".into(), description: "Rename the current session".into() },
            SlashCommand { name: "delete".into(), description: "Delete the current session".into() },
            SlashCommand { name: "system".into(), description: "Update system prompt for current session".into() },
        ];
        Self { commands }
    }
}

impl AutocompleteProvider for SlashCommandProvider {
    fn should_trigger(&self, context: &AutocompleteContext) -> bool {
        context.current_line.starts_with('/') && context.cursor_col > 0
    }

    fn get_suggestions(&self, context: &AutocompleteContext) -> Vec<Suggestion> {
        let prefix = &context.text_before_cursor[1..]; // strip leading '/'
        self.commands
            .iter()
            .filter(|cmd| cmd.name.starts_with(prefix))
            .map(|cmd| Suggestion {
                label: format!("/{} — {}", cmd.name, cmd.description),
                value: format!("/{}", cmd.name),
                description: Some(cmd.description.clone()),
            })
            .collect()
    }
}

pub struct FilePathProvider {
    base_dir: PathBuf,
}

impl FilePathProvider {
    pub fn new() -> Self {
        let base_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self { base_dir }
    }
}

/// Returns true if `fd` is available on the system PATH.
fn fd_available() -> bool {
    Command::new("fd")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Parse the last word from the current line as a potential path prefix.
/// Returns `(dir, prefix)` where:
/// - `dir` is the directory to list entries from (relative to base_dir)
/// - `prefix` is the filename prefix to filter by (empty if path ends with '/')
fn parse_path_prefix(_current_line: &str, text_before_cursor: &str) -> Option<(PathBuf, String)> {
    // Find the start index of the last word within the current line
    let last_word_start = text_before_cursor
        .rfind(' ')
        .map(|i| i + 1)
        .unwrap_or(0);

    let last_word = &text_before_cursor[last_word_start..];

    if last_word.is_empty() {
        return None;
    }

    // Expand tilde
    let expanded = if last_word.starts_with("~/") {
        if let Some(home) = dirs_fallback() {
            format!("{}/{}", home, &last_word[2..])
        } else {
            last_word.to_string()
        }
    } else {
        last_word.to_string()
    };

    let path = PathBuf::from(&expanded);

    if expanded.ends_with('/') || path.is_dir() {
        Some((path, String::new()))
    } else {
        let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
        let prefix = path.file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("")
            .to_string();
        Some((dir, prefix))
    }
}

fn dirs_fallback() -> Option<String> {
    std::env::var("HOME").ok()
}

impl AutocompleteProvider for FilePathProvider {
    fn should_trigger(&self, context: &AutocompleteContext) -> bool {
        if context.text_before_cursor.ends_with('/') {
            return true;
        }

        // Check if the last word starts with a path-like prefix
        let last_word_start = context.text_before_cursor
            .rfind(' ')
            .map(|i| i + 1)
            .unwrap_or(0);
        let last_word = &context.text_before_cursor[last_word_start..];

        last_word.starts_with("./") || last_word.starts_with("../") || last_word.starts_with('/') || last_word.starts_with("~/")
    }

    fn get_suggestions(&self, context: &AutocompleteContext) -> Vec<Suggestion> {
        let (dir, prefix) = match parse_path_prefix(&context.current_line, &context.text_before_cursor) {
            Some(p) => p,
            None => return Vec::new(),
        };

        let resolved_dir = if dir.is_absolute() {
            dir
        } else {
            self.base_dir.join(&dir)
        };

        if !resolved_dir.is_dir() {
            return Vec::new();
        }

        if fd_available() {
            fd_suggestions(&resolved_dir, &prefix)
        } else {
            fallback_suggestions(&resolved_dir, &prefix)
        }
    }
}

fn fd_suggestions(dir: &PathBuf, prefix: &str) -> Vec<Suggestion> {
    let output = match Command::new("fd")
        .arg("--max-results=20")
        .arg("--type=f")
        .arg("--type=d")
        .arg(".")
        .current_dir(dir)
        .output()
    {
        Ok(o) => o,
        Err(_) => return fallback_suggestions(dir, prefix),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let results: Vec<Suggestion> = stdout
        .lines()
        .filter(|line| {
            line.starts_with(prefix) || prefix.is_empty()
        })
        .take(8)
        .map(|line| {
            let file_path = dir.join(line);
            let is_dir = file_path.is_dir();
            let label = if is_dir {
                format!("{}/", line)
            } else {
                line.to_string()
            };
            Suggestion {
                label: label.clone(),
                value: label,
                description: None,
            }
        })
        .collect();

    // If fd is installed but returned nothing (might not filter by prefix),
    // try fallback
    if results.is_empty() && !prefix.is_empty() {
        return fallback_suggestions(dir, prefix);
    }

    results
}

fn fallback_suggestions(dir: &PathBuf, prefix: &str) -> Vec<Suggestion> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    let prefix_lower = prefix.to_lowercase();

    let mut results: Vec<(String, bool)> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().ok()?.is_dir();
            // Case-insensitive prefix match
            if name.to_lowercase().starts_with(&prefix_lower) {
                Some((name, is_dir))
            } else {
                None
            }
        })
        .collect();

    results.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    results.truncate(8);

    results
        .into_iter()
        .map(|(name, is_dir)| {
            let label = if is_dir {
                format!("{}/", name)
            } else {
                name.clone()
            };
            Suggestion {
                label: label.clone(),
                value: label,
                description: None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_context(current_line: &str, cursor_col: usize) -> AutocompleteContext {
        let text_before_cursor = if cursor_col <= current_line.len() {
            current_line[..cursor_col].to_string()
        } else {
            current_line.to_string()
        };

        AutocompleteContext {
            full_text: current_line.to_string(),
            cursor_line: 0,
            cursor_col,
            current_line: current_line.to_string(),
            text_before_cursor,
        }
    }

    #[test]
    fn test_slash_command_should_trigger() {
        let provider = SlashCommandProvider::new();

        let ctx = make_context("/cl", 3);
        assert!(provider.should_trigger(&ctx));

        let ctx = make_context("hello", 5);
        assert!(!provider.should_trigger(&ctx));

        // cursor at position 0 with '/' should NOT trigger
        let ctx = make_context("/quit", 0);
        assert!(!provider.should_trigger(&ctx));
    }

    #[test]
    fn test_slash_command_get_suggestions() {
        let provider = SlashCommandProvider::new();

        let ctx = make_context("/cl", 3);
        let suggestions = provider.get_suggestions(&ctx);
        assert!(!suggestions.is_empty());
        let labels: Vec<&str> = suggestions.iter().map(|s| s.label.as_str()).collect();
        assert!(labels.iter().any(|l| l.contains("clear")));

        let ctx = make_context("/q", 2);
        let suggestions = provider.get_suggestions(&ctx);
        assert!(suggestions.iter().any(|s| s.value == "/quit"));
    }

    #[test]
    fn test_slash_command_no_match() {
        let provider = SlashCommandProvider::new();

        let ctx = make_context("/xyz", 4);
        let suggestions = provider.get_suggestions(&ctx);
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_file_path_provider_new() {
        let provider = FilePathProvider::new();
        assert!(provider.base_dir.to_string_lossy().len() > 0);
    }

    #[test]
    fn test_autocomplete_context_fields() {
        let ctx = AutocompleteContext {
            full_text: "hello world".into(),
            cursor_line: 2,
            cursor_col: 5,
            current_line: "hello".into(),
            text_before_cursor: "hel".into(),
        };

        assert_eq!(ctx.full_text, "hello world");
        assert_eq!(ctx.cursor_line, 2);
        assert_eq!(ctx.cursor_col, 5);
        assert_eq!(ctx.current_line, "hello");
        assert_eq!(ctx.text_before_cursor, "hel");
    }

    #[test]
    fn test_file_path_should_trigger_with_trailing_slash() {
        let provider = FilePathProvider::new();
        let ctx = make_context("ls ./src/", 8);
        assert!(provider.should_trigger(&ctx));
    }

    #[test]
    fn test_file_path_should_trigger_with_dot_slash() {
        let provider = FilePathProvider::new();
        let ctx = make_context("cat ./Cargo", 9);
        assert!(provider.should_trigger(&ctx));
    }

    #[test]
    fn test_file_path_should_trigger_with_home() {
        let provider = FilePathProvider::new();
        let ctx = make_context("ls ~/Doc", 7);
        assert!(provider.should_trigger(&ctx));
    }

    #[test]
    fn test_file_path_should_not_trigger_plain_text() {
        let provider = FilePathProvider::new();
        let ctx = make_context("hello world", 7);
        assert!(!provider.should_trigger(&ctx));
    }

    #[test]
    fn test_slash_command_all_commands() {
        let provider = SlashCommandProvider::new();
        // All commands should be found with their full name prefix
        let expected = vec![
            "quit", "new", "switch", "list", "model", "clear",
            "connect", "auth", "tokens", "help",
            "retry", "copy", "dump", "compact", "rename",
            "delete", "system",
        ];
        for cmd in &expected {
            let ctx = make_context(&format!("/{}", cmd), cmd.len() + 1);
            let suggestions = provider.get_suggestions(&ctx);
            assert!(
                suggestions.iter().any(|s| s.value == format!("/{}", cmd)),
                "Should find command: /{}",
                cmd
            );
        }
    }
}
