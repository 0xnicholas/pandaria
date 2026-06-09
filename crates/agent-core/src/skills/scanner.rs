use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::types::{LoadSkillsResult, Skill, SkillDiagnostic, SkillDiagnosticKind, SkillSource};

const MAX_NAME_LENGTH: usize = 64;
const MAX_DESCRIPTION_LENGTH: usize = 1024;

/// Extract YAML frontmatter and the remaining Markdown body from file content.
///
/// Expects the file to start with `---` on its own line, followed by YAML,
/// followed by another `---` on its own line. Returns `(yaml, body)`.
pub fn extract_frontmatter(content: &str) -> Option<(String, String)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return None;
    }
    let after_first = &trimmed[3..];
    // Handle "---" vs "----" etc.
    let rest = after_first.trim_start_matches('-').trim_start();

    // Find the closing "---" on its own line.
    let end_pos = if rest.starts_with("---") {
        // Empty YAML block
        0
    } else {
        rest.find("\n---")?
    };

    let yaml = rest[..end_pos].trim().to_string();
    let body = if end_pos == 0 {
        rest[3..].trim_start().to_string()
    } else {
        rest[end_pos + 4..].trim_start().to_string()
    };
    Some((yaml, body))
}

/// Validate a skill name against the naming rules.
///
/// Returns a list of human-readable error messages. An empty list means the
/// name is valid.
pub fn validate_skill_name(name: &str, parent_dir_name: &str) -> Vec<String> {
    let mut errors = Vec::new();
    if name != parent_dir_name {
        errors.push(format!(
            r#"name "{}" does not match parent directory "{}""#,
            name, parent_dir_name
        ));
    }
    if name.len() > MAX_NAME_LENGTH {
        errors.push(format!(
            "name exceeds {} characters ({})",
            MAX_NAME_LENGTH,
            name.len()
        ));
    }
    if !regex::Regex::new(r"^[a-z0-9-]+$")
        .expect("static regex")
        .is_match(name)
    {
        errors.push(
            "name contains invalid characters (must be lowercase a-z, 0-9, hyphens only)"
                .to_string(),
        );
    }
    if name.starts_with('-') || name.ends_with('-') {
        errors.push("name must not start or end with a hyphen".to_string());
    }
    if name.contains("--") {
        errors.push("name must not contain consecutive hyphens".to_string());
    }
    errors
}

/// Check whether a file or directory name matches a simple ignore pattern.
///
/// v0.1: prefix / suffix / exact match only. No glob support.
fn is_ignored(name: &str, patterns: &[String]) -> bool {
    for pat in patterns {
        let pat = pat.trim();
        if pat.is_empty() || pat.starts_with('#') {
            continue;
        }
        // Simple prefix match (e.g. "node_modules/" or ".git/")
        if pat.ends_with('/') && name.starts_with(pat.trim_end_matches('/')) {
            return true;
        }
        // Exact match
        if pat == name {
            return true;
        }
        // Suffix match (e.g. "*.log" → check if name ends with ".log")
        if let Some(suffix) = pat.strip_prefix("*.")
            && name.ends_with(suffix) {
                return true;
            }
    }
    false
}

/// Read ignore patterns from `.gitignore`, `.ignore`, or `.fdignore` in the
/// given directory, if any exist.
fn read_ignore_patterns(dir: &Path) -> Vec<String> {
    for filename in [".gitignore", ".ignore", ".fdignore"] {
        let path = dir.join(filename);
        if let Ok(content) = std::fs::read_to_string(&path) {
            return content.lines().map(|l| l.to_string()).collect();
        }
    }
    Vec::new()
}

/// Scan a single directory for skills.
///
/// Returns the list of discovered skills plus any diagnostics.
fn scan_dir(
    dir: &Path,
    source: SkillSource,
    seen_names: &mut HashSet<String>,
    diagnostics: &mut Vec<SkillDiagnostic>,
) -> Vec<Skill> {
    let mut skills = Vec::new();

    let Ok(entries) = std::fs::read_dir(dir) else {
        return skills;
    };

    let ignore_patterns = read_ignore_patterns(dir);

    // Check for SKILL.md in this directory first
    let skill_md = dir.join("SKILL.md");
    if skill_md.is_file() {
        if let Some(skill) = load_skill_from_file(&skill_md, source, seen_names, diagnostics) {
            skills.push(skill);
        }
        return skills; // stop recursing when SKILL.md is found
    }

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.starts_with('.') {
            continue;
        }
        if is_ignored(&name_str, &ignore_patterns) {
            continue;
        }

        let path = entry.path();
        if path.is_file() && name_str.ends_with(".md") {
            if let Some(skill) = load_skill_from_file(&path, source, seen_names, diagnostics) {
                skills.push(skill);
            }
        } else if path.is_dir() {
            let sub = scan_dir(&path, source, seen_names, diagnostics);
            skills.extend(sub);
        }
    }

    skills
}

/// Load a single skill from a Markdown file.
fn load_skill_from_file(
    path: &Path,
    source: SkillSource,
    seen_names: &mut HashSet<String>,
    diagnostics: &mut Vec<SkillDiagnostic>,
) -> Option<Skill> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            diagnostics.push(SkillDiagnostic {
                path: path.display().to_string(),
                kind: SkillDiagnosticKind::Warning,
                message: format!("failed to read file: {}", e),
            });
            return None;
        }
    };

    let (yaml, _body) = match extract_frontmatter(&content) {
        Some(v) => v,
        None => {
            diagnostics.push(SkillDiagnostic {
                path: path.display().to_string(),
                kind: SkillDiagnosticKind::Warning,
                message: "missing or malformed YAML frontmatter".to_string(),
            });
            return None;
        }
    };

    let frontmatter: super::types::SkillFrontmatter = match serde_yaml::from_str(&yaml) {
        Ok(f) => f,
        Err(e) => {
            diagnostics.push(SkillDiagnostic {
                path: path.display().to_string(),
                kind: SkillDiagnosticKind::Warning,
                message: format!("invalid YAML frontmatter: {}", e),
            });
            return None;
        }
    };

    if frontmatter.description.is_empty() {
        diagnostics.push(SkillDiagnostic {
            path: path.display().to_string(),
            kind: SkillDiagnosticKind::Warning,
            message: "missing 'description' in frontmatter".to_string(),
        });
        return None;
    }

    if frontmatter.description.len() > MAX_DESCRIPTION_LENGTH {
        diagnostics.push(SkillDiagnostic {
            path: path.display().to_string(),
            kind: SkillDiagnosticKind::Warning,
            message: format!(
                "description exceeds {} characters ({})",
                MAX_DESCRIPTION_LENGTH,
                frontmatter.description.len()
            ),
        });
        return None;
    }

    let parent_dir_name = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let name = frontmatter
        .name
        .clone()
        .unwrap_or_else(|| parent_dir_name.clone());

    let name_errors = validate_skill_name(&name, &parent_dir_name);
    if !name_errors.is_empty() {
        for err in name_errors {
            diagnostics.push(SkillDiagnostic {
                path: path.display().to_string(),
                kind: SkillDiagnosticKind::Warning,
                message: err,
            });
        }
        return None;
    }

    // Collision detection
    if seen_names.contains(&name) {
        diagnostics.push(SkillDiagnostic {
            path: path.display().to_string(),
            kind: SkillDiagnosticKind::Collision,
            message: format!(
                r#"skill name "{}" collides with a previously loaded skill; skipping"#,
                name
            ),
        });
        return None;
    }
    seen_names.insert(name.clone());

    let canonical_path = match std::fs::canonicalize(path) {
        Ok(p) => p.display().to_string(),
        Err(_) => path.display().to_string(),
    };
    let canonical_base = match std::fs::canonicalize(path.parent().unwrap_or(Path::new("/"))) {
        Ok(p) => p.display().to_string(),
        Err(_) => path
            .parent()
            .unwrap_or(Path::new("/"))
            .display()
            .to_string(),
    };

    Some(Skill {
        name,
        description: frontmatter.description,
        file_path: canonical_path,
        base_dir: canonical_base,
        source,
        disable_model_invocation: frontmatter.disable_model_invocation,
    })
}

/// Scan multiple directories for skills, respecting source priority.
///
/// Directories are scanned in the order given; earlier sources win on name
/// collisions.
pub fn scan_skill_dirs(dirs: &[(PathBuf, SkillSource)]) -> LoadSkillsResult {
    let mut seen_names = HashSet::new();
    let mut diagnostics = Vec::new();
    let mut skills = Vec::new();

    for (dir, source) in dirs {
        let dir_skills = scan_dir(dir, *source, &mut seen_names, &mut diagnostics);
        skills.extend(dir_skills);
    }

    LoadSkillsResult {
        skills,
        diagnostics,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_skill_file(dir: &std::path::Path, name: &str, content: &str) -> PathBuf {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn test_extract_frontmatter_normal() {
        let content = "---\nname: foo\ndescription: bar\n---\n\n# Body\nhello";
        let (yaml, body) = extract_frontmatter(content).unwrap();
        assert_eq!(yaml, "name: foo\ndescription: bar");
        assert_eq!(body, "# Body\nhello");
    }

    #[test]
    fn test_extract_frontmatter_no_frontmatter() {
        assert!(extract_frontmatter("# Just markdown").is_none());
    }

    #[test]
    fn test_extract_frontmatter_empty_yaml() {
        let content = "---\n\n---\n# Body";
        let (yaml, body) = extract_frontmatter(content).unwrap();
        assert_eq!(yaml, "");
        assert_eq!(body, "# Body");
    }

    #[test]
    fn test_validate_skill_name_valid() {
        assert!(validate_skill_name("rust-debug", "rust-debug").is_empty());
        assert!(validate_skill_name("a1", "a1").is_empty());
    }

    #[test]
    fn test_validate_skill_name_mismatch() {
        let errs = validate_skill_name("foo", "bar");
        assert_eq!(errs.len(), 1);
        assert!(errs[0].contains("does not match parent directory"));
    }

    #[test]
    fn test_validate_skill_name_too_long() {
        let name = "a".repeat(65);
        let errs = validate_skill_name(&name, &name);
        assert!(errs.iter().any(|e| e.contains("exceeds")));
    }

    #[test]
    fn test_validate_skill_name_invalid_chars() {
        let errs = validate_skill_name("Rust_Debug", "Rust_Debug");
        assert!(errs.iter().any(|e| e.contains("invalid characters")));
    }

    #[test]
    fn test_validate_skill_name_hyphen_edges() {
        let errs = validate_skill_name("-foo", "-foo");
        assert!(
            errs.iter()
                .any(|e| e.contains("start or end with a hyphen"))
        );
    }

    #[test]
    fn test_validate_skill_name_double_hyphen() {
        let errs = validate_skill_name("foo--bar", "foo--bar");
        assert!(errs.iter().any(|e| e.contains("consecutive hyphens")));
    }

    #[test]
    fn test_scan_dir_skill_md_stops_recursion() {
        let tmp = tempfile::tempdir().unwrap();
        write_skill_file(
            tmp.path(),
            "my-skill",
            "---\nname: my-skill\ndescription: test\n---\n# Hello",
        );
        // A nested dir should NOT be scanned because SKILL.md at root stops it
        let nested = tmp.path().join("my-skill").join("sub");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            nested.join("SKILL.md"),
            "---\nname: sub\ndescription: sub\n---",
        )
        .unwrap();

        let result = scan_skill_dirs(&[(tmp.path().to_path_buf(), SkillSource::Project)]);
        assert_eq!(result.skills.len(), 1);
        assert_eq!(result.skills[0].name, "my-skill");
    }

    #[test]
    fn test_scan_dir_orphan_md_files() {
        let tmp = tempfile::tempdir().unwrap();
        // No SKILL.md in root, so .md files in root are loaded
        let md_dir = tmp.path().join("orphan");
        std::fs::create_dir_all(&md_dir).unwrap();
        let md_path = md_dir.join("orphan.md");
        std::fs::write(
            &md_path,
            "---\nname: orphan\ndescription: orphan skill\n---\n# Orphan",
        )
        .unwrap();

        let result = scan_skill_dirs(&[(tmp.path().to_path_buf(), SkillSource::Project)]);
        assert_eq!(result.skills.len(), 1);
        assert_eq!(result.skills[0].name, "orphan");
    }

    #[test]
    fn test_scan_dir_collision() {
        let tmp = tempfile::tempdir().unwrap();
        let user_dir = tmp.path().join("user");
        let project_dir = tmp.path().join("project");
        std::fs::create_dir_all(&user_dir.join("dup")).unwrap();
        std::fs::create_dir_all(&project_dir.join("dup")).unwrap();
        std::fs::write(
            user_dir.join("dup/SKILL.md"),
            "---\nname: dup\ndescription: first\n---",
        )
        .unwrap();
        std::fs::write(
            project_dir.join("dup/SKILL.md"),
            "---\nname: dup\ndescription: second\n---",
        )
        .unwrap();

        let result = scan_skill_dirs(&[
            (user_dir, SkillSource::User),
            (project_dir, SkillSource::Project),
        ]);
        assert_eq!(result.skills.len(), 1);
        assert_eq!(result.skills[0].description, "first");
        assert_eq!(result.diagnostics.len(), 1);
        assert!(matches!(
            result.diagnostics[0].kind,
            SkillDiagnosticKind::Collision
        ));
    }

    #[test]
    fn test_scan_dir_ignore_patterns() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".gitignore"), "ignored-dir/\n*.tmp\n").unwrap();
        let ignored_dir = tmp.path().join("ignored-dir");
        std::fs::create_dir_all(&ignored_dir).unwrap();
        std::fs::write(
            ignored_dir.join("SKILL.md"),
            "---\ndescription: ignored\n---",
        )
        .unwrap();
        let kept_dir = tmp.path().join("kept");
        std::fs::create_dir_all(&kept_dir).unwrap();
        std::fs::write(kept_dir.join("SKILL.md"), "---\ndescription: kept\n---").unwrap();

        let result = scan_skill_dirs(&[(tmp.path().to_path_buf(), SkillSource::Project)]);
        assert_eq!(result.skills.len(), 1);
        assert_eq!(result.skills[0].name, "kept");
    }

    #[test]
    fn test_scan_dir_missing_description() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bad");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "---\nname: bad\n---").unwrap();

        let result = scan_skill_dirs(&[(tmp.path().to_path_buf(), SkillSource::Project)]);
        assert!(result.skills.is_empty());
        assert_eq!(result.diagnostics.len(), 1);
        assert!(result.diagnostics[0].message.contains("description"));
    }

    #[test]
    fn test_scan_dir_malformed_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bad");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), "---\nname: [\n---").unwrap();

        let result = scan_skill_dirs(&[(tmp.path().to_path_buf(), SkillSource::Project)]);
        assert!(result.skills.is_empty());
        assert_eq!(result.diagnostics.len(), 1);
        assert!(result.diagnostics[0].message.contains("YAML"));
    }

    #[test]
    fn test_scan_dir_name_from_parent_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("auto-name");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\ndescription: auto named\n---\n# Body",
        )
        .unwrap();

        let result = scan_skill_dirs(&[(tmp.path().to_path_buf(), SkillSource::Project)]);
        assert_eq!(result.skills.len(), 1);
        assert_eq!(result.skills[0].name, "auto-name");
    }
}
