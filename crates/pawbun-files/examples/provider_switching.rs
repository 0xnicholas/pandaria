//! Demonstrates formatting the same content for different LLM providers.

use bytes::Bytes;
use pawbun_files::{
    AnthropicFormat, AzureOpenAiFormat, DefaultFileLoader, File, FileLoader, GeminiFormat,
    OpenAiFormat, ProviderFormat,
};

fn main() {
    // Create a simple text file in memory
    let file = File::from_bytes(Bytes::from_static(b"Q3 revenue up 15%"), "report.txt");

    let loader = DefaultFileLoader::new();
    let loaded = loader.load(&file).expect("load");

    // OpenAI
    let openai = OpenAiFormat;
    let block = openai.format_content(&loaded.content).unwrap();
    println!(
        "OpenAI:\n{}\n",
        serde_json::to_string_pretty(&block).unwrap()
    );

    // Anthropic
    let anthropic = AnthropicFormat;
    let block = anthropic.format_content(&loaded.content).unwrap();
    println!(
        "Anthropic:\n{}\n",
        serde_json::to_string_pretty(&block).unwrap()
    );

    // Gemini
    let gemini = GeminiFormat;
    let block = gemini.format_content(&loaded.content).unwrap();
    println!(
        "Gemini:\n{}\n",
        serde_json::to_string_pretty(&block).unwrap()
    );

    // Azure OpenAI
    let azure = AzureOpenAiFormat;
    let block = azure.format_content(&loaded.content).unwrap();
    println!(
        "Azure OpenAI:\n{}\n",
        serde_json::to_string_pretty(&block).unwrap()
    );
}
