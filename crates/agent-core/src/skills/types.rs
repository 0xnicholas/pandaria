use serde::Deserialize;

/// Parsed YAML frontmatter from a SKILL.md file.
#[derive(Debug, Clone, Deserialize)]
pub struct SkillFrontmatter {
    /// Skill identifier. If omitted, defaults to the parent directory name.
    pub name: Option<String>,
    /// Short description used when injecting into the system prompt.
    pub description: String,
    /// When true, the skill is not listed in `<available_skills>` and can only
    /// be invoked explicitly via `/skill:name`.
    #[serde(rename = "disable-model-invocation", default)]
    pub disable_model_invocation: bool,
}

/// Where a skill was discovered from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillSource {
    /// User-level directory, e.g. `~/.agents/skills/`.
    User,
    /// Project-level directory, e.g. `<cwd>/.agents/skills/`.
    Project,
    /// Explicitly provided path.
    Path,
}

/// Metadata for a discovered skill.  **Does not cache file content** — content
/// is read on-demand (e.g. when `/skill:name` is invoked) to avoid memory
/// bloat in multi-tenant scenarios.
#[derive(Debug, Clone)]
pub struct Skill {
    /// Skill identifier (lowercase a-z, 0-9, hyphens).
    pub name: String,
    /// Human-readable description for the skill index.
    pub description: String,
    /// Absolute path to the SKILL.md file.
    pub file_path: String,
    /// Absolute path to the directory containing the skill.
    pub base_dir: String,
    /// Source of discovery.
    pub source: SkillSource,
    /// Whether automatic prompt injection is disabled.
    pub disable_model_invocation: bool,
}

/// Severity of a skill-loading diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillDiagnosticKind {
    /// A non-fatal issue (malformed file, invalid name, etc.). The skill is
    /// skipped and processing continues.
    Warning,
    /// Two skills share the same name; the earlier one is kept.
    Collision,
}

/// A diagnostic message emitted during skill discovery/loading.
#[derive(Debug, Clone)]
pub struct SkillDiagnostic {
    /// Path of the file or directory related to the diagnostic.
    pub path: String,
    /// Severity / category.
    pub kind: SkillDiagnosticKind,
    /// Human-readable explanation.
    pub message: String,
}

/// Result of a `SkillLoader::load_skills()` call.  Never returns `Err` — all
/// problems are captured as `diagnostics` and the successfully loaded skills
/// are returned in `skills`.
#[derive(Debug, Clone)]
pub struct LoadSkillsResult {
    pub skills: Vec<Skill>,
    pub diagnostics: Vec<SkillDiagnostic>,
}
