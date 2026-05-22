use std::path::{Path, PathBuf};

/// Unified agent space directory layout.
///
/// All runtime data (workspaces, config, cache, logs, temp, skills)
/// lives under a single root, defaulting to `~/.pandaria`.
///
/// Override the root via the `PANDARIA_SPACE_ROOT` environment variable.
///
/// # Directory Layout
///
/// ```text
/// {root}/
///   ├── config/              # Configuration files
///   ├── cache/               # LLM response cache, build artifacts
///   ├── logs/                # File logs (when tracing-appender is used)
///   ├── temp/                # Temporary files (replaces system temp_dir)
///   ├── skills/              # Global skill definition files
///   └── workspaces/
///         └── {tenant_id}/   # Tenant-scoped agent working files
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSpace {
    root: PathBuf,
}

impl AgentSpace {
    /// Create an `AgentSpace` with an explicit root path.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Resolve the agent space root from the environment or defaults.
    ///
    /// Priority:
    /// 1. `PANDARIA_SPACE_ROOT` environment variable
    /// 2. `~/.pandaria`
    /// 3. Fallback to `./.pandaria` in the current working directory
    pub fn from_env_or_default() -> Self {
        if let Ok(root) = std::env::var("PANDARIA_SPACE_ROOT") {
            return Self::new(root);
        }

        if let Ok(home) = std::env::var("HOME") {
            return Self::new(PathBuf::from(home).join(".pandaria"));
        }

        Self::new("./.pandaria")
    }

    /// The root of the agent space.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `{root}/config/`
    pub fn config_dir(&self) -> PathBuf {
        self.root.join("config")
    }

    /// `{root}/cache/`
    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// `{root}/logs/`
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// `{root}/temp/`
    pub fn temp_dir(&self) -> PathBuf {
        self.root.join("temp")
    }

    /// `{root}/skills/`
    pub fn skills_dir(&self) -> PathBuf {
        self.root.join("skills")
    }

    /// `{root}/workspaces/{tenant_id}/`
    pub fn workspace_for(&self, tenant_id: &str) -> PathBuf {
        self.root.join("workspaces").join(tenant_id)
    }

    /// `{root}/workspaces/{tenant_id}/media/`
    pub fn media_dir(&self, tenant_id: &str) -> PathBuf {
        self.workspace_for(tenant_id).join("media")
    }

    /// Ensure all standard sub-directories exist.
    ///
    /// Idempotent — safe to call multiple times.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        for dir in [
            self.config_dir(),
            self.cache_dir(),
            self.logs_dir(),
            self.temp_dir(),
            self.skills_dir(),
            self.root.join("workspaces"),
        ] {
            std::fs::create_dir_all(&dir)?;
        }
        Ok(())
    }
}

impl Default for AgentSpace {
    fn default() -> Self {
        Self::from_env_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_space_paths() {
        let space = AgentSpace::new("/tmp/pandaria-test");
        assert_eq!(space.root(), Path::new("/tmp/pandaria-test"));
        assert_eq!(
            space.config_dir(),
            PathBuf::from("/tmp/pandaria-test/config")
        );
        assert_eq!(space.cache_dir(), PathBuf::from("/tmp/pandaria-test/cache"));
        assert_eq!(space.logs_dir(), PathBuf::from("/tmp/pandaria-test/logs"));
        assert_eq!(space.temp_dir(), PathBuf::from("/tmp/pandaria-test/temp"));
        assert_eq!(
            space.skills_dir(),
            PathBuf::from("/tmp/pandaria-test/skills")
        );
        assert_eq!(
            space.workspace_for("tenant-42"),
            PathBuf::from("/tmp/pandaria-test/workspaces/tenant-42")
        );
    }

    #[test]
    fn test_media_dir() {
        let space = AgentSpace::new("/tmp/pandaria-test");
        assert_eq!(
            space.media_dir("tenant-42"),
            PathBuf::from("/tmp/pandaria-test/workspaces/tenant-42/media")
        );
    }

    #[test]
    fn test_agent_space_ensure_dirs() {
        let temp = std::env::temp_dir().join(format!("pandaria-space-test-{}", std::process::id()));
        let space = AgentSpace::new(&temp);
        space.ensure_dirs().expect("ensure_dirs should succeed");

        assert!(space.config_dir().exists());
        assert!(space.cache_dir().exists());
        assert!(space.logs_dir().exists());
        assert!(space.temp_dir().exists());
        assert!(space.skills_dir().exists());
        assert!(space.root().join("workspaces").exists());

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn test_agent_space_from_env() {
        let key = "PANDARIA_SPACE_ROOT";
        let old = std::env::var(key).ok();
        unsafe {
            std::env::set_var(key, "/custom/pandaria");
        }
        let space = AgentSpace::from_env_or_default();
        assert_eq!(space.root(), Path::new("/custom/pandaria"));
        unsafe {
            match old {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }
}
