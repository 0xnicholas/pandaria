#[cfg(test)]
mod tests {
    use super::super::builder::PromptBuilder;
    use super::super::mutation::PromptMutation;
    use super::super::types::{FragmentKind, FragmentSource, PromptFragment};

    #[test]
    fn test_replace_all_short_circuits() {
        let mut builder = PromptBuilder::from_base("Original");
        builder.upsert_fragment(PromptFragment {
            id: "ext".into(),
            kind: FragmentKind::Extension,
            source: FragmentSource::Extension { name: "x".into() },
            content: "Extra".into(),
            priority: 10,
        });

        let mutation = PromptMutation {
            replace_all: Some("Replaced".into()),
            ..Default::default()
        };
        builder.apply_mutation(mutation);

        assert_eq!(builder.render(), "Replaced");
    }

    #[test]
    fn test_remove_ids() {
        let mut builder = PromptBuilder::from_base("Base");
        builder.upsert_fragment(PromptFragment {
            id: "ext".into(),
            kind: FragmentKind::Extension,
            source: FragmentSource::Extension { name: "x".into() },
            content: "Extra".into(),
            priority: 10,
        });

        let mutation = PromptMutation {
            remove_ids: vec!["ext".into()],
            ..Default::default()
        };
        builder.apply_mutation(mutation);
        assert_eq!(builder.render(), "Base");
    }

    #[test]
    fn test_remove_sources() {
        let mut builder = PromptBuilder::from_base("Base");
        builder.upsert_fragment(PromptFragment {
            id: "ext".into(),
            kind: FragmentKind::Extension,
            source: FragmentSource::Extension { name: "x".into() },
            content: "Extra".into(),
            priority: 10,
        });

        let mutation = PromptMutation {
            remove_sources: vec![FragmentSource::Extension { name: "x".into() }],
            ..Default::default()
        };
        builder.apply_mutation(mutation);
        assert_eq!(builder.render(), "Base");
    }

    #[test]
    fn test_remove_kinds() {
        let mut builder = PromptBuilder::from_base("Base");
        builder.upsert_fragment(PromptFragment {
            id: "skills".into(),
            kind: FragmentKind::SkillsDirectory,
            source: FragmentSource::SkillsInjector,
            content: "<skills/>".into(),
            priority: 10,
        });

        let mutation = PromptMutation {
            remove_kinds: vec![FragmentKind::SkillsDirectory],
            ..Default::default()
        };
        builder.apply_mutation(mutation);
        assert_eq!(builder.render(), "Base");
    }

    #[test]
    fn test_upsert_fragments() {
        let mut builder = PromptBuilder::from_base("Base");

        let mutation = PromptMutation {
            upsert_fragments: vec![PromptFragment {
                id: "safety".into(),
                kind: FragmentKind::SafetyGuard,
                source: FragmentSource::Extension { name: "guard".into() },
                content: "Be safe.".into(),
                priority: -100,
            }],
            ..Default::default()
        };
        builder.apply_mutation(mutation);
        assert_eq!(builder.render(), "Be safe.\nBase");
    }

    #[test]
    fn test_combined_mutation() {
        let mut builder = PromptBuilder::from_base("Base");
        builder.upsert_fragment(PromptFragment {
            id: "old-ext".into(),
            kind: FragmentKind::Extension,
            source: FragmentSource::Extension { name: "old".into() },
            content: "Old".into(),
            priority: 10,
        });

        let mutation = PromptMutation {
            remove_ids: vec!["old-ext".into()],
            upsert_fragments: vec![PromptFragment {
                id: "new-ext".into(),
                kind: FragmentKind::Extension,
                source: FragmentSource::Extension { name: "new".into() },
                content: "New".into(),
                priority: 20,
            }],
            ..Default::default()
        };
        builder.apply_mutation(mutation);
        assert_eq!(builder.render(), "Base\nNew");
    }

    #[test]
    fn test_empty_mutation_is_noop() {
        let mut builder = PromptBuilder::from_base("Base");
        builder.apply_mutation(PromptMutation::default());
        assert_eq!(builder.render(), "Base");
    }
}
