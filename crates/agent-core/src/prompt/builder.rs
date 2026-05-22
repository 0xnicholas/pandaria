use super::types::{
    FragmentKind, FragmentSource, PromptFragment, RenderedFragment, RenderedPrompt,
};
use tracing::warn;

/// Assembles the final system prompt from ordered fragments.
#[derive(Debug, Clone, Default)]
pub struct PromptBuilder {
    fragments: Vec<PromptFragment>,
}

impl PromptBuilder {
    /// Create a builder with a single BasePersona fragment.
    pub fn from_base(base: impl Into<String>) -> Self {
        let text = base.into();
        Self {
            fragments: vec![PromptFragment {
                id: "base-persona-0".into(),
                kind: FragmentKind::BasePersona,
                source: FragmentSource::SessionParam,
                content: text,
                priority: 0,
            }],
        }
    }

    /// Maximum bytes allowed for a single fragment content.
    /// Fragments exceeding this limit are truncated with a warning.
    pub const MAX_FRAGMENT_BYTES: usize = 256_000;

    /// Insert or replace a fragment by `id`. If an existing fragment has
    /// the same `id`, it is replaced; otherwise the fragment is inserted
    /// and the list is re-sorted by `priority` (stable).
    pub fn upsert_fragment(&mut self, mut fragment: PromptFragment) {
        if fragment.content.len() > Self::MAX_FRAGMENT_BYTES {
            warn!(
                fragment_id = %fragment.id,
                original_bytes = fragment.content.len(),
                max_bytes = Self::MAX_FRAGMENT_BYTES,
                "fragment content exceeds maximum size, truncating"
            );
            fragment.content.truncate(Self::MAX_FRAGMENT_BYTES);
        }
        if let Some(pos) = self.fragments.iter().position(|f| f.id == fragment.id) {
            self.fragments[pos] = fragment;
        } else {
            self.fragments.push(fragment);
        }
        self.sort_by_priority();
    }

    /// Remove all fragments whose `source` matches.
    pub fn remove_by_source(&mut self, source: &FragmentSource) {
        self.fragments.retain(|f| &f.source != source);
    }

    /// Remove all fragments whose `kind` matches.
    pub fn remove_by_kind(&mut self, kind: &FragmentKind) {
        self.fragments.retain(|f| &f.kind != kind);
    }

    /// Remove a single fragment by `id`.
    pub fn remove_by_id(&mut self, id: &str) -> Option<PromptFragment> {
        if let Some(pos) = self.fragments.iter().position(|f| f.id == id) {
            Some(self.fragments.remove(pos))
        } else {
            None
        }
    }

    /// Render to plain string by concatenating fragments in priority order.
    /// Fragments are trimmed of trailing whitespace and joined with a single
    /// newline separator to avoid accidental blank lines.
    pub fn render(&self) -> String {
        if self.fragments.is_empty() {
            return String::new();
        }

        let trimmed: Vec<String> = self
            .fragments
            .iter()
            .map(|f| f.content.trim_end().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        trimmed.join("\n")
    }

    /// Render to `Option<String>`. Returns `None` if the rendered text is
    /// empty, preserving the semantic equivalence of "no system prompt".
    pub fn render_option(&self) -> Option<String> {
        let text = self.render();
        if text.is_empty() { None } else { Some(text) }
    }

    /// Render with per-fragment metadata for observability.
    pub fn render_with_metadata(&self) -> RenderedPrompt {
        let mut text = String::new();
        let mut rendered_fragments = Vec::new();

        for fragment in &self.fragments {
            let trimmed = fragment.content.trim_end();
            if trimmed.is_empty() {
                continue;
            }

            if !text.is_empty() {
                text.push('\n');
            }
            let offset = text.len();
            text.push_str(trimmed);
            let len = text.len() - offset;

            rendered_fragments.push(RenderedFragment {
                id: fragment.id.clone(),
                kind: fragment.kind,
                source: fragment.source.clone(),
                byte_offset: offset,
                byte_len: len,
                estimated_tokens: (trimmed.chars().count() as f64 / 4.0).ceil() as usize,
            });
        }

        RenderedPrompt {
            text,
            fragments: rendered_fragments,
        }
    }

    /// Estimate total token count using a character-based heuristic
    /// (`total_chars / 4.0`), independent of the compaction module to
    /// avoid cross-module coupling.
    pub fn estimate_tokens(&self) -> usize {
        let total_chars: usize = self
            .fragments
            .iter()
            .map(|f| f.content.chars().count())
            .sum();
        (total_chars as f64 / 4.0).ceil() as usize
    }

    fn sort_by_priority(&mut self) {
        self.fragments.sort_by_key(|f| f.priority);
    }
}
