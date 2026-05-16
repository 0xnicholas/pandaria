/// Semantic category of a prompt fragment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FragmentKind {
    /// Core persona / role definition.
    BasePersona,
    /// Tenant-specific metadata (name, plan, region).
    TenantContext,
    /// The `<available_skills>` XML directory block.
    SkillsDirectory,
    /// Full content of an invoked skill (loaded via `/skill:name`).
    SkillBody,
    /// Dynamic injections from steer queue or compaction.
    RuntimeInjection,
    /// Generic contribution from an Extension.
    Extension,
    /// Hard safety constraints (e.g. "never reveal API keys").
    SafetyGuard,
}

/// Origin of a prompt fragment for observability and conflict resolution.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FragmentSource {
    TenantDefault,
    SessionParam,
    SkillsInjector,
    Extension { name: String },
    CompactionSummary,
    System,
}

/// A semantic segment of the system prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptFragment {
    /// Unique identifier within a builder instance.
    pub id: String,
    /// Semantic category of this fragment.
    pub kind: FragmentKind,
    /// Origin of this fragment.
    pub source: FragmentSource,
    /// Raw text content.
    pub content: String,
    /// Sort priority. Lower values appear earlier in rendered output.
    pub priority: i16,
}

/// Metadata for a single rendered fragment.
#[derive(Debug, Clone)]
pub struct RenderedFragment {
    pub id: String,
    pub kind: FragmentKind,
    pub source: FragmentSource,
    pub byte_offset: usize,
    pub byte_len: usize,
    pub estimated_tokens: usize,
}

/// Rendered prompt with per-fragment metadata.
#[derive(Debug, Clone)]
pub struct RenderedPrompt {
    pub text: String,
    pub fragments: Vec<RenderedFragment>,
}
