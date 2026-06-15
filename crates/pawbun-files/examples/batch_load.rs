//! Batch file loading with constraints example.
//!
//! Demonstrates loading multiple files with size and type constraints.

use pawbun_files::{DefaultFileLoader, File, FileLoader, FileConstraints, OverflowMode, MediaType};

fn main() {
    let tmp = tempfile::tempdir().unwrap();
    let path1 = tmp.path().join("a.txt");
    let path2 = tmp.path().join("b.txt");
    std::fs::write(&path1, "hello world").unwrap();
    std::fs::write(&path2, "foo bar baz").unwrap();

    let loader = DefaultFileLoader::with_base_dir(tmp.path());
    let files = vec![
        File::from_path(&path1).with_constraints(FileConstraints {
            max_size_bytes: Some(1024),
            allowed_media_types: Some(vec![MediaType::Text]),
            overflow_mode: OverflowMode::Strict,
            ..Default::default()
        }),
        File::from_path(&path2),
    ];

    let results = loader.load_batch(&files);
    for (file, result) in results {
        match result {
            Ok(loaded) => println!("{}: loaded {} bytes", file.metadata.name.as_deref().unwrap_or("?"), loaded.content.size_bytes()),
            Err(e) => println!("{}: error: {}", file.metadata.name.as_deref().unwrap_or("?"), e),
        }
    }
}
