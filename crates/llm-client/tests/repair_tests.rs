use llm_client::{
    parse_json_with_repair, repair_json, sanitize_unicode, StreamingJsonParser,
};

#[test]
fn test_repair_unclosed_string() {
    let input = r#"{"key":"val"#;
    let repaired = repair_json(input);
    assert!(serde_json::from_str::<serde_json::Value>(&repaired).is_ok());
    let v: serde_json::Value = serde_json::from_str(&repaired).unwrap();
    assert_eq!(v["key"], "val");
}

#[test]
fn test_repair_trailing_comma_object() {
    let input = r#"{"a":1,}"#;
    let repaired = repair_json(input);
    let v: serde_json::Value = serde_json::from_str(&repaired).unwrap();
    assert_eq!(v["a"], 1);
    assert!(v.as_object().unwrap().get("b").is_none());
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
    let result = parser.feed(r#""val"}"#);
    assert!(result.is_some());
    assert_eq!(result.unwrap()["key"], "val");
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
    let input = "hello world \u{00E9}";
    let output = sanitize_unicode(input);
    assert_eq!(output, input);
}

#[test]
fn test_sanitize_unicode_replacement_chars() {
    // U+FFFD replacement characters (from invalid UTF-8) should be preserved
    // since they are not in the surrogate range
    let input = "hello \u{FFFD}world";
    let output = sanitize_unicode(input);
    assert_eq!(output, input);
}
