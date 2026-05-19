use super::types::Skill;
use crate::prompt::{FragmentKind, FragmentSource, PromptBuilder, PromptFragment};

/// Format a slice of skills as an XML block to be appended to the system
/// prompt.  Compatible with pi.dev's `<available_skills>` format.
pub fn format_skills_for_prompt(skills: &[Skill]) -> String {
    let visible: Vec<_> = skills.iter().filter(|s| !s.disable_model_invocation).collect();
    if visible.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        "\n\nThe following skills provide specialized instructions for specific tasks.".to_string(),
        "Use the read tool to load a skill's file when the task matches its description.".to_string(),
        "When a skill file references a relative path, resolve it against the skill directory.".to_string(),
        String::new(),
        "<available_skills>".to_string(),
    ];

    for skill in visible {
        lines.push("  <skill>".to_string());
        lines.push(format!("    <name>{}</name>", escape_xml(&skill.name)));
        lines.push(format!(
            "    <description>{}</description>",
            escape_xml(&skill.description)
        ));
        lines.push(format!(
            "    <location>{}</location>",
            escape_xml(&skill.file_path)
        ));
        lines.push("  </skill>".to_string());
    }

    lines.push("</available_skills>".to_string());
    lines.join("\n")
}

/// Inject the `<available_skills>` fragment into a [`PromptBuilder`].
///
/// Idempotent — upserts a fragment with id `"skills-directory"`, replacing any
/// existing one. No-op when `skills` is empty.
pub fn inject_skills_into_builder(builder: &mut PromptBuilder, skills: &[Skill]) {
    if skills.is_empty() {
        return;
    }
    let xml = format_skills_for_prompt(skills);
    if xml.is_empty() {
        return;
    }
    builder.upsert_fragment(PromptFragment {
        id: "skills-directory".into(),
        kind: FragmentKind::SkillsDirectory,
        source: FragmentSource::SkillsInjector,
        content: xml,
        priority: 50,
    });
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::types::{Skill, SkillSource};

    fn make_skill(name: &str, desc: &str, path: &str, disabled: bool) -> Skill {
        Skill {
            name: name.to_string(),
            description: desc.to_string(),
            file_path: path.to_string(),
            base_dir: "/tmp".to_string(),
            source: SkillSource::Project,
            disable_model_invocation: disabled,
        }
    }

    #[test]
    fn test_format_skills_basic() {
        let skills = vec![
            make_skill("rust-debug", "Debug Rust async issues.", "/skills/rust-debug/SKILL.md", false),
        ];
        let xml = format_skills_for_prompt(&skills);
        assert!(xml.contains("<available_skills>"));
        assert!(xml.contains("<name>rust-debug</name>"));
        assert!(xml.contains("<description>Debug Rust async issues.</description>"));
        assert!(xml.contains("<location>/skills/rust-debug/SKILL.md</location>"));
        assert!(xml.contains("</available_skills>"));
    }

    #[test]
    fn test_format_skills_empty() {
        assert_eq!(format_skills_for_prompt(&[]), "");
    }

    #[test]
    fn test_format_skills_disabled_filtered() {
        let skills = vec![
            make_skill("visible", "I am visible.", "/a", false),
            make_skill("hidden", "I am hidden.", "/b", true),
        ];
        let xml = format_skills_for_prompt(&skills);
        assert!(xml.contains("visible"));
        assert!(!xml.contains("hidden"));
    }

    #[test]
    fn test_format_skills_all_disabled_returns_empty() {
        let skills = vec![
            make_skill("a", "desc", "/a", true),
        ];
        assert_eq!(format_skills_for_prompt(&skills), "");
    }

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("a & b < c > d \"e\" 'f'"), "a &amp; b &lt; c &gt; d &quot;e&quot; &apos;f&apos;");
    }
}
