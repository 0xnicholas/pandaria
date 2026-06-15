use criterion::{black_box, criterion_group, criterion_main, Criterion};
use pawbun_files::{DefaultFileLoader, File, FileLoader, OpenAiFormat, ProviderFormat};

fn benchmark_load_local(c: &mut Criterion) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test.txt");
    std::fs::write(&path, "hello world").unwrap();

    let loader = DefaultFileLoader::with_base_dir(tmp.path());
    let file = File::from_path(&path);
    c.bench_function("load_local/text", |b| {
        b.iter(|| {
            let _ = loader.load(black_box(&file));
        })
    });
}

fn benchmark_provider_format(c: &mut Criterion) {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("test.txt");
    std::fs::write(&path, "hello world").unwrap();

    let loader = DefaultFileLoader::with_base_dir(tmp.path());
    let file = File::from_path(&path);
    let loaded = loader.load(&file).unwrap();
    let formatter = OpenAiFormat;

    c.bench_function("provider_format/openai/text", |b| {
        b.iter(|| {
            let _ = formatter.format_content(black_box(&loaded.content));
        })
    });
}

criterion_group!(benches, benchmark_load_local, benchmark_provider_format);
criterion_main!(benches);
