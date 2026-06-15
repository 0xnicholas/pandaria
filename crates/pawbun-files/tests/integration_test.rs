//! Integration tests for pawbun-files.

use std::io::Write;

use bytes::Bytes;
use pawbun_files::{
    DefaultFileLoader, File, FileLoader, MediaContent, MediaType, OpenAiFormat, ProviderFormat,
};

#[test]
fn test_end_to_end_text_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("report.txt");
    {
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"Q3 revenue: $1.2M").unwrap();
    }

    let loader = DefaultFileLoader::with_base_dir(dir.path());
    let file = File::from_path(&path).with_key("report");
    let loaded = loader.load(&file).unwrap();

    assert_eq!(loaded.content.as_text(), Some("Q3 revenue: $1.2M"));
    assert_eq!(loaded.content.media_type(), MediaType::Text);
    assert_eq!(loaded.metadata.name, Some("report.txt".into()));

    let fmt = OpenAiFormat;
    let block = fmt.format_content(&loaded.content).unwrap();
    assert_eq!(block["type"], "text");
    assert_eq!(block["text"], "Q3 revenue: $1.2M");
}

#[test]
fn test_end_to_end_image_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chart.png");
    {
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"\x89PNG\r\n\x1a\n").unwrap();
    }

    let loader = DefaultFileLoader::with_base_dir(dir.path());
    let file = File::from_path(&path);
    let loaded = loader.load(&file).unwrap();

    assert!(matches!(loaded.content, MediaContent::Image(_)));

    let fmt = OpenAiFormat;
    let block = fmt.format_content(&loaded.content).unwrap();
    assert_eq!(block["type"], "image_url");
    let url = block["image_url"]["url"].as_str().unwrap();
    assert!(url.starts_with("data:image/png;base64,"));
}

#[test]
fn test_end_to_end_bytes_source() {
    let data = Bytes::from_static(b"inline data");
    let file = File::from_bytes(data, "note.txt").with_key("note");

    let loader = DefaultFileLoader::new();
    let loaded = loader.load(&file).unwrap();

    assert_eq!(loaded.content.as_text(), Some("inline data"));
    assert_eq!(loaded.metadata.size_bytes, Some(11));
}

#[test]
fn test_sandbox_path_traversal() {
    let dir = tempfile::tempdir().unwrap();
    let loader = DefaultFileLoader::with_base_dir(dir.path());

    // Create a file in the parent directory of the sandbox to test traversal.
    let parent_file = dir
        .path()
        .parent()
        .unwrap()
        .join("pawbun_integration_secret.txt");
    std::fs::write(&parent_file, "secret").unwrap();

    let file = File::from_path("../pawbun_integration_secret.txt");
    let result = loader.load(&file);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("path traversal"),
        "expected path traversal error: {}",
        err
    );

    let _ = std::fs::remove_file(&parent_file);
}

#[test]
fn test_format_file_with_url() {
    let file = File::from_url("https://example.com/image.png");
    let loader = DefaultFileLoader::new();
    let fmt = OpenAiFormat;

    // URL should be formatted as reference without loading.
    let block = fmt.format_file(&file, &loader).unwrap();
    assert_eq!(block["type"], "image_url");
    assert_eq!(block["image_url"]["url"], "https://example.com/image.png");
}

#[test]
fn test_batch_load() {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = (0..3)
        .map(|i| {
            let p = dir.path().join(format!("file{i}.txt"));
            std::fs::write(&p, format!("content{i}")).unwrap();
            p
        })
        .collect();

    let loader = DefaultFileLoader::with_base_dir(dir.path());
    let files: Vec<_> = paths.iter().map(File::from_path).collect();
    let results = loader.load_batch(&files);

    assert_eq!(results.len(), 3);
    for (i, (_file, result)) in results.iter().enumerate() {
        let loaded = result.as_ref().unwrap();
        assert_eq!(
            loaded.content.as_text(),
            Some(format!("content{i}").as_str())
        );
    }
}
