use super::builder::PromptBuilder;
use super::types::{FragmentKind, FragmentSource, PromptFragment};

/// Auxiliary descriptor for modifying a PromptBuilder.
#[derive(Debug, Clone, Default)]
pub struct PromptMutation {
    /// Replace the entire prompt with a single string.
    /// When present, all other fields are ignored and the builder is
    /// reset to a single BasePersona fragment.
    pub replace_all: Option<String>,

    /// Remove fragments by id.
    pub remove_ids: Vec<String>,

    /// Remove fragments by source.
    pub remove_sources: Vec<FragmentSource>,

    /// Remove fragments by kind.
    pub remove_kinds: Vec<FragmentKind>,

    /// Fragments to upsert.
    pub upsert_fragments: Vec<PromptFragment>,
}

impl PromptBuilder {
    /// Apply a PromptMutation to this builder.
    /// Order: replace_all (short-circuit) → remove_ids → remove_sources
    /// → remove_kinds → upsert_fragments.
    pub fn apply_mutation(&mut self, mutation: PromptMutation) {
        if let Some(text) = mutation.replace_all {
            *self = PromptBuilder::from_base(text);
            return;
        }

        for id in &mutation.remove_ids {
            self.remove_by_id(id);
        }

        for source in &mutation.remove_sources {
            self.remove_by_source(source);
        }

        for kind in &mutation.remove_kinds {
            self.remove_by_kind(kind);
        }

        for fragment in mutation.upsert_fragments {
            self.upsert_fragment(fragment);
        }
    }
}

impl From<String> for PromptBuilder {
    fn from(s: String) -> Self {
        Self::from_base(s)
    }
}

impl From<&str> for PromptBuilder {
    fn from(s: &str) -> Self {
        Self::from_base(s)
    }
}
