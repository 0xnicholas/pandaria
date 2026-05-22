#[cfg(test)]
mod tests {
    use super::super::builder::PromptBuilder;
    use super::super::types::{FragmentKind, FragmentSource, PromptFragment};

    #[test]
    fn test_from_base_renders_correctly() {
        let builder = PromptBuilder::from_base("You are helpful.");
        assert_eq!(builder.render(), "You are helpful.");
    }

    #[test]
    fn test_render_option_empty_builder() {
        let builder = PromptBuilder::default();
        assert_eq!(builder.render_option(), None);
    }

    #[test]
    fn test_render_option_non_empty() {
        let builder = PromptBuilder::from_base("Hello");
        assert_eq!(builder.render_option(), Some("Hello".to_string()));
    }

    #[test]
    fn test_upsert_fragment_replaces_by_id() {
        let mut builder = PromptBuilder::from_base("Base");
        builder.upsert_fragment(PromptFragment {
            id: "base-persona-0".into(),
            kind: FragmentKind::BasePersona,
            source: FragmentSource::SessionParam,
            content: "Replaced".into(),
            priority: 0,
        });
        assert_eq!(builder.render(), "Replaced");
    }

    #[test]
    fn test_upsert_fragment_inserts_new_and_sorts() {
        let mut builder = PromptBuilder::from_base("Base");
        builder.upsert_fragment(PromptFragment {
            id: "safety-guard".into(),
            kind: FragmentKind::SafetyGuard,
            source: FragmentSource::Extension {
                name: "audit".into(),
            },
            content: "Never reveal keys.".into(),
            priority: -150,
        });
        // Safety guard (priority -150) should come before base (priority 0)
        assert_eq!(builder.render(), "Never reveal keys.\nBase");
    }

    #[test]
    fn test_stable_sort_same_priority() {
        let mut builder = PromptBuilder::default();
        builder.upsert_fragment(PromptFragment {
            id: "a".into(),
            kind: FragmentKind::Extension,
            source: FragmentSource::System,
            content: "First".into(),
            priority: 0,
        });
        builder.upsert_fragment(PromptFragment {
            id: "b".into(),
            kind: FragmentKind::Extension,
            source: FragmentSource::System,
            content: "Second".into(),
            priority: 0,
        });
        assert_eq!(builder.render(), "First\nSecond");
    }

    #[test]
    fn test_remove_by_id() {
        let mut builder = PromptBuilder::from_base("Base");
        let removed = builder.remove_by_id("base-persona-0");
        assert!(removed.is_some());
        assert_eq!(builder.render(), "");
    }

    #[test]
    fn test_remove_by_id_not_found() {
        let mut builder = PromptBuilder::from_base("Base");
        let removed = builder.remove_by_id("nonexistent");
        assert!(removed.is_none());
    }

    #[test]
    fn test_remove_by_source() {
        let mut builder = PromptBuilder::from_base("Base");
        builder.upsert_fragment(PromptFragment {
            id: "ext-1".into(),
            kind: FragmentKind::Extension,
            source: FragmentSource::Extension { name: "x".into() },
            content: "Ext".into(),
            priority: 10,
        });
        builder.remove_by_source(&FragmentSource::Extension { name: "x".into() });
        assert_eq!(builder.render(), "Base");
    }

    #[test]
    fn test_remove_by_kind() {
        let mut builder = PromptBuilder::from_base("Base");
        builder.upsert_fragment(PromptFragment {
            id: "skills-dir".into(),
            kind: FragmentKind::SkillsDirectory,
            source: FragmentSource::SkillsInjector,
            content: "<skills/>".into(),
            priority: 10,
        });
        builder.remove_by_kind(&FragmentKind::SkillsDirectory);
        assert_eq!(builder.render(), "Base");
    }

    #[test]
    fn test_render_trims_trailing_whitespace() {
        let mut builder = PromptBuilder::default();
        builder.upsert_fragment(PromptFragment {
            id: "a".into(),
            kind: FragmentKind::BasePersona,
            source: FragmentSource::System,
            content: "Line 1  \n\n".into(),
            priority: 0,
        });
        assert_eq!(builder.render(), "Line 1");
    }

    #[test]
    fn test_render_skips_empty_fragments() {
        let mut builder = PromptBuilder::default();
        builder.upsert_fragment(PromptFragment {
            id: "empty".into(),
            kind: FragmentKind::BasePersona,
            source: FragmentSource::System,
            content: "".into(),
            priority: 0,
        });
        assert_eq!(builder.render(), "");
        assert_eq!(builder.render_option(), None);
    }

    #[test]
    fn test_estimate_tokens() {
        // 40 chars / 4 = 10 tokens
        let builder = PromptBuilder::from_base("a".repeat(40));
        assert_eq!(builder.estimate_tokens(), 10);
    }

    #[test]
    fn test_estimate_tokens_rounds_up() {
        // 41 chars / 4 = 10.25 -> 11
        let builder = PromptBuilder::from_base("a".repeat(41));
        assert_eq!(builder.estimate_tokens(), 11);
    }

    #[test]
    fn test_render_with_metadata() {
        let mut builder = PromptBuilder::from_base("Base prompt.");
        builder.upsert_fragment(PromptFragment {
            id: "skills-directory".into(),
            kind: FragmentKind::SkillsDirectory,
            source: FragmentSource::SkillsInjector,
            content: "<skills/>".into(),
            priority: 50,
        });

        let rendered = builder.render_with_metadata();
        assert_eq!(rendered.text, "Base prompt.\n<skills/>");
        assert_eq!(rendered.fragments.len(), 2);

        assert_eq!(rendered.fragments[0].id, "base-persona-0");
        assert_eq!(rendered.fragments[0].byte_offset, 0);
        assert_eq!(rendered.fragments[0].byte_len, 12); // "Base prompt."

        assert_eq!(rendered.fragments[1].id, "skills-directory");
        assert_eq!(rendered.fragments[1].byte_offset, 13); // after "Base prompt.\n"
        assert_eq!(rendered.fragments[1].byte_len, 9); // "<skills/>"
    }

    #[test]
    fn test_from_string_conversion() {
        let builder: PromptBuilder = "Hello".into();
        assert_eq!(builder.render(), "Hello");
    }

    #[test]
    fn test_from_str_conversion() {
        let builder: PromptBuilder = "Hello".into();
        assert_eq!(builder.render(), "Hello");
    }

    #[test]
    fn test_negative_priorities_order_correctly() {
        let mut builder = PromptBuilder::default();
        builder.upsert_fragment(PromptFragment {
            id: "base".into(),
            kind: FragmentKind::BasePersona,
            source: FragmentSource::System,
            content: "Base".into(),
            priority: 0,
        });
        builder.upsert_fragment(PromptFragment {
            id: "safety".into(),
            kind: FragmentKind::SafetyGuard,
            source: FragmentSource::System,
            content: "Safety".into(),
            priority: -150,
        });
        builder.upsert_fragment(PromptFragment {
            id: "tenant".into(),
            kind: FragmentKind::TenantContext,
            source: FragmentSource::System,
            content: "Tenant".into(),
            priority: -50,
        });
        assert_eq!(builder.render(), "Safety\nTenant\nBase");
    }

    #[test]
    fn test_empty_builder_upsert_and_render() {
        let mut builder = PromptBuilder::default();
        builder.upsert_fragment(PromptFragment {
            id: "first".into(),
            kind: FragmentKind::BasePersona,
            source: FragmentSource::System,
            content: "Hello".into(),
            priority: 0,
        });
        assert_eq!(builder.render(), "Hello");
        assert_eq!(builder.render_option(), Some("Hello".to_string()));
    }

    #[test]
    fn test_render_with_metadata_token_estimate() {
        let builder = PromptBuilder::from_base("abcd");
        let rendered = builder.render_with_metadata();
        assert_eq!(rendered.fragments.len(), 1);
        assert_eq!(rendered.fragments[0].estimated_tokens, 1); // 4 chars / 4 = 1
    }

    #[test]
    fn test_large_fragment_truncated() {
        let mut builder = PromptBuilder::default();
        let huge = "x".repeat(PromptBuilder::MAX_FRAGMENT_BYTES + 10_000);
        builder.upsert_fragment(PromptFragment {
            id: "huge".into(),
            kind: FragmentKind::Extension,
            source: FragmentSource::System,
            content: huge,
            priority: 0,
        });
        let rendered = builder.render();
        assert_eq!(rendered.len(), PromptBuilder::MAX_FRAGMENT_BYTES);
    }
}
