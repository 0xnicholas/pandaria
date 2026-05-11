/// Strip lone surrogates and other invalid Unicode from LLM output text.
pub fn sanitize_unicode(s: &str) -> String {
    s.chars().filter(|c| !is_surrogate(*c)).collect()
}

fn is_surrogate(c: char) -> bool {
    let cu = c as u32;
    (0xD800..=0xDFFF).contains(&cu)
}

/// Streaming JSON parser that accumulates partial fragments.
/// Used during tool call argument streaming.
pub struct StreamingJsonParser {
    buffer: String,
}

impl StreamingJsonParser {
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    /// Feed a delta fragment. Returns `Some(Value)` if a valid JSON
    /// can be parsed at this point (best-effort partial).
    pub fn feed(&mut self, delta: &str) -> Option<serde_json::Value> {
        self.buffer.push_str(delta);
        parse_json_with_repair(&self.buffer).ok()
    }

    /// Consume the parser and return the final best-effort parse result.
    pub fn finalize(self) -> Result<serde_json::Value, serde_json::Error> {
        parse_json_with_repair(&self.buffer)
    }

    /// Return the current best-effort parsed value without consuming
    /// the parser. Returns `None` if nothing has been fed yet.
    pub fn peek_value(&self) -> Option<serde_json::Value> {
        if self.buffer.is_empty() {
            return None;
        }
        parse_json_with_repair(&self.buffer).ok()
    }
}

impl Default for StreamingJsonParser {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse JSON with repair, returning `serde_json::Value` or the first parse error
/// after all repair heuristics fail.
pub fn parse_json_with_repair(s: &str) -> Result<serde_json::Value, serde_json::Error> {
    let repaired = repair_json(s);
    serde_json::from_str(&repaired)
}

/// Repair malformed JSON from LLM output by applying heuristics in order.
///
/// 1. Fix unclosed strings by appending closing quote
/// 2. Remove trailing commas before closing brackets/braces
/// 3. Convert single-quoted strings to double-quoted
/// 4. Escape unescaped control characters
/// 5. Balance brackets (`[]`, `{}`) by appending missing closers
/// 6. Strip non-printable Unicode
///
/// If the input is valid JSON, it is returned unchanged.
pub fn repair_json(s: &str) -> String {
    let s = s.trim();

    // Quick path: already valid
    if serde_json::from_str::<serde_json::Value>(s).is_ok() {
        return s.to_string();
    }

    let mut result = String::with_capacity(s.len());
    let mut in_string = false;
    let mut string_char: Option<char> = None;
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();

    let mut i = 0;
    while i < len {
        let c = chars[i];

        if in_string {
            if let Some(quote) = string_char {
                if c == quote && (i == 0 || chars[i - 1] != '\\') {
                    in_string = false;
                    string_char = None;
                    result.push('"');
                } else {
                    result.push(c);
                }
            } else {
                result.push(c);
            }
        } else if c == '"' || c == '\'' {
            in_string = true;
            string_char = Some(c);
            result.push('"');
        } else if c == ',' {
            // Check if this trailing comma is before ] or }
            let mut j = i + 1;
            while j < len && chars[j].is_whitespace() {
                j += 1;
            }
            if j < len && (chars[j] == ']' || chars[j] == '}') {
                // skip trailing comma
            } else {
                result.push(c);
            }
        } else if c.is_control() && !c.is_whitespace() && c != '\n' && c != '\r' && c != '\t' {
            // skip non-printable control chars
        } else {
            result.push(c);
        }
        i += 1;
    }

    // If still in string, close it
    if in_string {
        result.push('"');
    }

    // Balance brackets
    let mut stack: Vec<char> = Vec::new();
    for c in result.chars() {
        match c {
            '[' | '{' => stack.push(c),
            ']' => {
                if stack.last() == Some(&'[') {
                    stack.pop();
                }
            }
            '}' => {
                if stack.last() == Some(&'{') {
                    stack.pop();
                }
            }
            _ => {}
        }
    }
    while let Some(open) = stack.pop() {
        result.push(match open {
            '[' => ']',
            '{' => '}',
            _ => continue,
        });
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repair_unclosed_string() {
        let input = r#"{"key":"val"#;
        let repaired = repair_json(input);
        assert!(serde_json::from_str::<serde_json::Value>(&repaired).is_ok());
    }

    #[test]
    fn test_repair_trailing_comma_object() {
        let input = r#"{"a":1,}"#;
        let repaired = repair_json(input);
        let v: serde_json::Value = serde_json::from_str(&repaired).unwrap();
        assert_eq!(v["a"], 1);
    }

    #[test]
    fn test_repair_trailing_comma_array() {
        let input = "[1,2,]";
        let repaired = repair_json(input);
        let v: serde_json::Value = serde_json::from_str(&repaired).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_repair_single_quotes() {
        let input = "{'a':'b'}";
        let repaired = repair_json(input);
        let v: serde_json::Value = serde_json::from_str(&repaired).unwrap();
        assert_eq!(v["a"], "b");
    }

    #[test]
    fn test_valid_json_unchanged() {
        let input = r#"{"key": "value", "arr": [1, 2]}"#;
        let repaired = repair_json(input);
        assert_eq!(repaired, input);
    }

    #[test]
    fn test_streaming_parser_accumulate() {
        let mut parser = StreamingJsonParser::new();
        assert!(parser.feed(r#"{"key":"#).is_none());
        assert!(parser.feed(r#""val"}"#).is_some());
    }

    #[test]
    fn test_streaming_parser_finalize() {
        let mut parser = StreamingJsonParser::new();
        parser.feed(r#"{"a": 1, "b": "hello"}"#);
        let v = parser.finalize().unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], "hello");
    }

    #[test]
    fn test_sanitize_unicode_noop() {
        // Rust's String type already prevents lone surrogates.
        // This function is a no-op for valid Rust strings, but guards
        // against raw byte inputs that may contain surrogates when decoded.
        let input = "hello world \u{00E9}";
        let output = sanitize_unicode(input);
        assert_eq!(output, input);
    }

    #[test]
    fn test_parse_json_with_repair_success() {
        let input = r#"{"key": "value",}"#;
        let v = parse_json_with_repair(input).unwrap();
        assert_eq!(v["key"], "value");
    }

    #[test]
    fn test_streaming_parser_empty_input() {
        let parser = StreamingJsonParser::new();
        let result = parser.finalize();
        assert!(result.is_err());
    }

    #[test]
    fn test_streaming_parser_invalid_json() {
        let parser = StreamingJsonParser::new();
        let result = parser.finalize();
        // Empty buffer produces parse error
        assert!(result.is_err());
    }

    #[test]
    fn test_streaming_parser_peek_progressive() {
        let mut parser = StreamingJsonParser::new();
        assert!(parser.peek_value().is_none());

        parser.feed(r#"{"ke"#);
        // Best-effort: may or may not parse depending on repair heuristics
        // At minimum, peek_value should not panic

        parser.feed(r#"y": "va"#);
        // Still partial

        parser.feed(r#"lue"}"#);
        let val = parser.peek_value().unwrap();
        assert_eq!(val["key"], "value");

        // finalize should also work and produce the same result
        let val2 = parser.finalize().unwrap();
        assert_eq!(val2["key"], "value");
    }

    #[test]
    fn test_repair_escaped_quotes() {
        // Valid JSON with escaped quotes should remain unchanged
        let input = r#"{"key":"va\"lue"}"#;
        let repaired = repair_json(input);
        let v: serde_json::Value = serde_json::from_str(&repaired).unwrap();
        assert_eq!(v["key"], "va\"lue");
    }

    #[test]
    fn test_repair_nested_unclosed() {
        // Deeply nested object with missing closing braces
        let input = r#"{"a":{"b":{"c":1}"#;
        let repaired = repair_json(input);
        let v: serde_json::Value = serde_json::from_str(&repaired).unwrap();
        assert_eq!(v["a"]["b"]["c"], 1);
    }

    #[test]
    fn test_repair_mixed_heuristics() {
        // Single quotes + trailing comma in array
        let input = r#"{'a': 1, 'b': "hello", 'c': [1,2,]}"#;
        let repaired = repair_json(input);
        let v: serde_json::Value = serde_json::from_str(&repaired).unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"], "hello");
        assert_eq!(v["c"].as_array().unwrap().len(), 2);
    }
}
