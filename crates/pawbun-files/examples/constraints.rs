//! Demonstrates file constraints and validation.

use bytes::Bytes;
use pawbun_files::{File, FileConstraints, MediaContent, MediaType, OverflowMode};

fn main() {
    // Create a text file with constraints
    let file = File::from_bytes(Bytes::from_static(b"hello world"), "note.txt").with_constraints(
        FileConstraints {
            max_size_bytes: Some(100),
            allowed_media_types: Some(vec![MediaType::Text]),
            overflow_mode: OverflowMode::Strict,
            ..Default::default()
        },
    );

    // Check constraints against content
    let content = MediaContent::Text(pawbun_files::TextContent {
        text: "hello world".into(),
        encoding: Some("utf-8".into()),
    });

    match file.constraints.check(&content) {
        Ok(()) => println!("Content passes all constraints!"),
        Err(e) => println!("Constraint violation: {}", e),
    }

    // A content that violates size limit
    let big_content = MediaContent::Text(pawbun_files::TextContent {
        text: "x".repeat(200),
        encoding: Some("utf-8".into()),
    });

    match file.constraints.check(&big_content) {
        Ok(()) => println!("Big content passes (unexpected)"),
        Err(e) => println!("Expected violation: {}", e),
    }
}
