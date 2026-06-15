//! Image loading and provider formatting example.
//!
//! Demonstrates loading a local image and formatting it for OpenAI.

use std::io::Write;
use pawbun_files::{DefaultFileLoader, File, FileLoader, OpenAiFormat, ProviderFormat};

fn main() {
    // Create a temporary PNG file for the demo.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("demo.png");
    {
        let mut f = std::fs::File::create(&path).unwrap();
        // Minimal valid PNG header.
        f.write_all(b"\x89PNG\r\n\x1a\n").unwrap();
    }

    let loader = DefaultFileLoader::with_base_dir(tmp.path());
    let file = File::from_path(&path);
    let loaded = loader.load(&file).expect("load image");

    println!("Loaded: {:?}", loaded.content.media_type());

    let formatter = OpenAiFormat;
    let block = formatter.format_content(&loaded.content).expect("format");
    println!("OpenAI format: {}", serde_json::to_string_pretty(&block).unwrap());
}
