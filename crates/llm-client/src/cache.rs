use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheRetention {
    None,
    Short,
    Long,
}

impl CacheRetention {
    pub fn resolve(explicit: Option<Self>) -> Self {
        if let Some(explicit) = explicit {
            return explicit;
        }
        if std::env::var("PANDARIA_CACHE_RETENTION")
            .map(|v| v == "long")
            .unwrap_or(false)
        {
            return Self::Long;
        }
        Self::Short
    }
}

impl Default for CacheRetention {
    fn default() -> Self {
        Self::Short
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_resolve_explicit() {
        assert_eq!(
            CacheRetention::resolve(Some(CacheRetention::Long)),
            CacheRetention::Long
        );
        assert_eq!(
            CacheRetention::resolve(Some(CacheRetention::None)),
            CacheRetention::None
        );
        assert_eq!(
            CacheRetention::resolve(Some(CacheRetention::Short)),
            CacheRetention::Short
        );
    }

    #[test]
    fn test_cache_resolve_default() {
        // Without PANDARIA_CACHE_RETENTION set, default should be Short
        // (env var may or may not be set — explicit tests cover all cases)
        let result = CacheRetention::resolve(None);
        assert!(matches!(
            result,
            CacheRetention::Short | CacheRetention::Long
        ));
    }

    #[test]
    fn test_cache_default() {
        assert_eq!(CacheRetention::default(), CacheRetention::Short);
    }

    #[test]
    fn test_cache_resolve_env_var_all_cases() {
        // std::env is global state; run all env-var cases in one test
        // to avoid race conditions with concurrent tests.
        let old = std::env::var("PANDARIA_CACHE_RETENTION").ok();

        // Case 1: env = "long" -> Long
        unsafe { std::env::set_var("PANDARIA_CACHE_RETENTION", "long"); }
        assert_eq!(CacheRetention::resolve(None), CacheRetention::Long);

        // Case 2: env = "short" -> Short
        unsafe { std::env::set_var("PANDARIA_CACHE_RETENTION", "short"); }
        assert_eq!(CacheRetention::resolve(None), CacheRetention::Short);

        // Case 3: env = "invalid_value" -> Short (fallback)
        unsafe { std::env::set_var("PANDARIA_CACHE_RETENTION", "invalid_value"); }
        assert_eq!(CacheRetention::resolve(None), CacheRetention::Short);

        // Case 4: explicit overrides env
        unsafe { std::env::set_var("PANDARIA_CACHE_RETENTION", "long"); }
        assert_eq!(CacheRetention::resolve(Some(CacheRetention::None)), CacheRetention::None);

        // Restore
        unsafe {
            match old {
                Some(v) => std::env::set_var("PANDARIA_CACHE_RETENTION", v),
                None => std::env::remove_var("PANDARIA_CACHE_RETENTION"),
            }
        }
    }
}
