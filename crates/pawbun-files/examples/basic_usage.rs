//! Basic usage of pawbun-files: construct, load, and format files.

use bytes::Bytes;
use pawbun_files::{
    DefaultFileLoader, File, FileLoader, MediaContent, OpenAiFormat, ProviderFormat,
};

fn main() {
    // 1. Construct files from different sources
    let local_file = File::from_path("./report.txt").with_key("report");
    let url_file = File::from_url("https://example.com/chart.png");
    let bytes_file = File::from_bytes(Bytes::from_static(b"hello world"), "note.txt");

    println!("Local: {:?}", local_file.media_type);
    println!("URL: {:?}", url_file.media_type);
    println!("Bytes: {:?}", bytes_file.media_type);

    // 2. Load an in-memory file
    let loader = DefaultFileLoader::new();
    let loaded = loader.load(&bytes_file).expect("load bytes file");
    println!("Loaded content: {:?}", loaded.content.media_type());

    if let MediaContent::Text(ref txt) = loaded.content {
        println!("Text content: {}", txt.text);
    }

    // 3. Format for OpenAI
    let formatter = OpenAiFormat;
    let block = formatter.format_content(&loaded.content).expect("format");
    println!(
        "OpenAI block: {}",
        serde_json::to_string_pretty(&block).unwrap()
    );
}
