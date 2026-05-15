use std::path::PathBuf;

use async_trait::async_trait;

use super::scanner::scan_skill_dirs;
use super::types::{LoadSkillsResult, SkillSource};

/// Abstract loader for skills.  Implementations may load from the file system,
/// a database, an in-memory cache, etc.
#[async_trait]
pub trait SkillLoader: Send + Sync {
    /// Load all available skills.
    ///
    /// Returns a `LoadSkillsResult` where:
    /// - `skills` contains successfully loaded skills (may be empty)
    /// - `diagnostics` contains all warnings and collisions encountered
    ///
    /// This method **never** returns `Err`; all errors are captured as
    /// diagnostics.
    async fn load_skills(&self) -> LoadSkillsResult;
}

/// Default skill loader that scans the file system.
///
/// Scans three sources in order:
/// 1. `user_skills_dir`
/// 2. `project_skills_dir`
/// 3. `explicit_paths`
///
/// Earlier sources win on name collisions.
#[derive(Debug, Clone)]
pub struct FileSystemSkillLoader {
    pub user_skills_dir: String,
    pub project_skills_dir: String,
    pub explicit_paths: Vec<String>,
}

#[async_trait]
impl SkillLoader for FileSystemSkillLoader {
    async fn load_skills(&self) -> LoadSkillsResult {
        let user = self.user_skills_dir.clone();
        let project = self.project_skills_dir.clone();
        let explicit = self.explicit_paths.clone();

        tokio::task::spawn_blocking(move || {
            let mut dirs: Vec<(PathBuf, SkillSource)> = Vec::new();

            let user_path = PathBuf::from(&user);
            if user_path.is_dir() {
                dirs.push((user_path, SkillSource::User));
            }

            let project_path = PathBuf::from(&project);
            if project_path.is_dir() {
                dirs.push((project_path, SkillSource::Project));
            }

            for path in &explicit {
                let p = PathBuf::from(path);
                if p.is_dir() {
                    dirs.push((p, SkillSource::Path));
                }
            }

            scan_skill_dirs(&dirs)
        })
        .await
        .unwrap_or_else(|e| LoadSkillsResult {
            skills: Vec::new(),
            diagnostics: vec![super::types::SkillDiagnostic {
                path: String::new(),
                kind: super::types::SkillDiagnosticKind::Warning,
                message: format!("skill loader panicked: {}", e),
            }],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[tokio::test]
    async fn test_filesystem_loader_basic() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join("test-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        let mut file = std::fs::File::create(skill_dir.join("SKILL.md")).unwrap();
        file.write_all(b"---\nname: test-skill\ndescription: A test skill\n---\n# Body")
            .unwrap();

        let loader = FileSystemSkillLoader {
            user_skills_dir: tmp.path().display().to_string(),
            project_skills_dir: String::new(),
            explicit_paths: vec![],
        };

        let result = loader.load_skills().await;
        assert_eq!(result.skills.len(), 1);
        assert_eq!(result.skills[0].name, "test-skill");
        assert!(result.diagnostics.is_empty());
    }

    #[tokio::test]
    async fn test_filesystem_loader_nonexistent_dirs() {
        let loader = FileSystemSkillLoader {
            user_skills_dir: "/does/not/exist".to_string(),
            project_skills_dir: "/also/does/not/exist".to_string(),
            explicit_paths: vec![],
        };

        let result = loader.load_skills().await;
        assert!(result.skills.is_empty());
        assert!(result.diagnostics.is_empty());
    }

    #[tokio::test]
    async fn test_filesystem_loader_partial_failure() {
        let tmp = tempfile::tempdir().unwrap();

        // Valid skill
        let good = tmp.path().join("good");
        std::fs::create_dir_all(&good).unwrap();
        std::fs::write(
            good.join("SKILL.md"),
            "---\nname: good\ndescription: good\n---",
        )
        .unwrap();

        // Invalid skill (missing description)
        let bad = tmp.path().join("bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("SKILL.md"), "---\nname: bad\n---").unwrap();

        let loader = FileSystemSkillLoader {
            user_skills_dir: tmp.path().display().to_string(),
            project_skills_dir: String::new(),
            explicit_paths: vec![],
        };

        let result = loader.load_skills().await;
        assert_eq!(result.skills.len(), 1);
        assert_eq!(result.skills[0].name, "good");
        assert_eq!(result.diagnostics.len(), 1);
        assert!(result.diagnostics[0].message.contains("description"));
    }
}
