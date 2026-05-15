use crate::ui::theme::Theme;
use pulldown_cmark::{Event, Parser, Tag, TagEnd, Options};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;
use std::sync::LazyLock;

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

pub fn render_markdown(text: &str, theme: &Theme, syntax_theme: &str) -> Vec<Line<'static>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(text, options);
    let mut lines: Vec<Line> = Vec::new();
    let mut current_line: Vec<Span> = Vec::new();
    let mut in_code_block = false;
    let mut code_lang: Option<String> = None;
    let mut code_buffer = String::new();
    let mut current_style = theme.body();
    let mut style_stack: Vec<ratatui::style::Style> = Vec::new();

    for event in parser {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                in_code_block = true;
                code_lang = match kind { pulldown_cmark::CodeBlockKind::Fenced(l) => if l.is_empty() { None } else { Some(l.to_string()) }, _ => None };
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                for span_vec in highlight_code(&code_buffer, code_lang.as_deref(), syntax_theme) {
                    lines.push(Line::from(span_vec));
                }
                code_buffer.clear(); code_lang = None;
            }
            Event::Text(text) | Event::Code(text) => {
                if in_code_block { code_buffer.push_str(&text); }
                else { current_line.push(Span::styled(text.to_string(), current_style)); }
            }
            Event::SoftBreak | Event::HardBreak => {
                lines.push(std::mem::take(&mut current_line).into());
            }
            Event::Start(Tag::Heading { level, .. }) => {
                current_line.push(Span::styled(format!("{} ", "#".repeat(level as usize)), theme.heading(level as u8)));
            }
            Event::End(TagEnd::Heading(_)) => {
                lines.push(std::mem::take(&mut current_line).into());
                lines.push(Line::from(""));
            }
            Event::Start(Tag::BlockQuote(_)) => {
                current_line.push(Span::styled("│ ", theme.muted()));
            }
            Event::Start(Tag::Strong) => {
                style_stack.push(current_style);
                current_style = current_style.add_modifier(ratatui::style::Modifier::BOLD);
            }
            Event::End(TagEnd::Strong) => {
                if let Some(prev) = style_stack.pop() { current_style = prev; }
            }
            Event::Start(Tag::Emphasis) => {
                style_stack.push(current_style);
                current_style = current_style.add_modifier(ratatui::style::Modifier::ITALIC);
            }
            Event::End(TagEnd::Emphasis) => {
                if let Some(prev) = style_stack.pop() { current_style = prev; }
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                let url = if dest_url.len() > 60 { format!("{}…", &dest_url[..57]) } else { dest_url.to_string() };
                current_line.push(Span::styled(format!(" ({})", url), theme.link()));
            }
            _ => {}
        }
    }
    if !current_line.is_empty() { lines.push(Line::from(current_line)); }
    lines
}

fn highlight_code(code: &str, lang: Option<&str>, syntax_theme: &str) -> Vec<Vec<Span<'static>>> {
    let ps = &*SYNTAX_SET;
    let ts = &*THEME_SET;
    let syntax = lang.and_then(|l| ps.find_syntax_by_token(l).or_else(|| ps.find_syntax_by_extension(l))).unwrap_or_else(|| ps.find_syntax_plain_text());
    let highlight_theme = ts.themes.get(syntax_theme).unwrap_or(&ts.themes["base16-ocean.dark"]);
    let mut h = HighlightLines::new(syntax, highlight_theme);
    let mut result = Vec::new();
    for line in LinesWithEndings::from(code) {
        let ranges: Vec<(syntect::highlighting::Style, &str)> = h.highlight_line(line, &ps).unwrap_or_default();
        result.push(ranges.into_iter().map(|(style, text)| Span::styled(text.to_string(), Style::default().fg(ratatui::style::Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b)))).collect());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn test_heading() { assert!(!render_markdown("# Hello", &Theme::default(), "base16-ocean.dark").is_empty()); }
    #[test] fn test_code_block() { assert!(render_markdown("```\nfn main() {}\n```", &Theme::default(), "base16-ocean.dark").iter().any(|l| l.spans.iter().any(|s| s.content.contains("fn main")))); }
    #[test] fn test_paragraph() { let r = render_markdown("hello world", &Theme::default(), "base16-ocean.dark"); assert!(r[0].spans.iter().any(|s| s.content.contains("hello"))); }

    #[test]
    fn test_inline_code() {
        let r = render_markdown("use `Vec::new()` here", &Theme::default(), "base16-ocean.dark");
        assert!(r.iter().any(|l| l.spans.iter().any(|s| s.content.contains("Vec::new()"))));
    }

    #[test]
    fn test_bold_text() {
        let r = render_markdown("**bold**", &Theme::default(), "base16-ocean.dark");
        assert!(r.iter().any(|l| l.spans.iter().any(|s| s.content.contains("bold") && s.style.add_modifier == ratatui::style::Modifier::BOLD)));
    }

    #[test]
    fn test_italic_text() {
        let r = render_markdown("*italic*", &Theme::default(), "base16-ocean.dark");
        assert!(r.iter().any(|l| l.spans.iter().any(|s| s.content.contains("italic") && s.style.add_modifier == ratatui::style::Modifier::ITALIC)));
    }

    #[test]
    fn test_nested_bold_italic() {
        let r = render_markdown("***both***", &Theme::default(), "base16-ocean.dark");
        let has_both = r.iter().any(|l| l.spans.iter().any(|s| s.content.contains("both")));
        assert!(has_both);
    }

    #[test]
    fn test_blockquote() {
        let r = render_markdown("> quote line", &Theme::default(), "base16-ocean.dark");
        assert!(r.iter().any(|l| l.spans.iter().any(|s| s.content.contains("│"))));
    }

    #[test]
    fn test_link_truncation() {
        let long_url = "https://example.com/".repeat(10);
        let md = format!("[text]({})", long_url);
        let r = render_markdown(&md, &Theme::default(), "base16-ocean.dark");
        assert!(r.iter().any(|l| l.spans.iter().any(|s| s.content.contains("…"))));
    }

    #[test]
    fn test_soft_break_creates_new_line() {
        let r = render_markdown("line one  \nline two", &Theme::default(), "base16-ocean.dark");
        assert!(r.len() >= 2);
    }

    #[test]
    fn test_multiple_paragraphs() {
        let r = render_markdown("p1\n\np2", &Theme::default(), "base16-ocean.dark");
        assert!(r.iter().any(|l| l.spans.iter().any(|s| s.content.contains("p1"))));
        assert!(r.iter().any(|l| l.spans.iter().any(|s| s.content.contains("p2"))));
    }
}
