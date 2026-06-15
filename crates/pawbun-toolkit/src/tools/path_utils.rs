//! Path resolution and sandbox validation utilities.

use std::path::{Path, PathBuf};

use crate::ToolError;

/// Resolve a path within a sandbox directory, preventing path traversal.
///
/// 1. Canonicalizes `base_dir` (falls back to current dir if None).
/// 2. Resolves the target path (absolute paths are canonicalized directly;
///    relative paths are joined to `base` then canonicalized).
/// 3. Verifies the resolved path still lies within `base`.
pub fn resolve_sandbox_path(base_dir: Option<&Path>, input: &str) -> Result<PathBuf, ToolError> {
    let base = match base_dir {
        Some(p) => p.canonicalize().map_err(|e| {
            ToolError::Io { message: format!("cannot resolve base_dir: {e}"), kind: e.kind() }
        })?,
        None => std::env::current_dir()
            .map_err(|e| ToolError::Io { message: format!("cannot get current dir: {e}"), kind: e.kind() })?,
    };

    let target = if Path::new(input).is_absolute() {
        PathBuf::from(input)
    } else {
        base.join(input)
    };

    let target = target
        .canonicalize()
        .map_err(|e| ToolError::Io { message: format!("invalid path: {e}"), kind: e.kind() })?;

    if !target.starts_with(&base) {
        return Err(ToolError::invalid_input("path traversal detected"));
    }

    Ok(target)
}

/// Semantic path normalization for write operations where the target may not yet exist.
///
/// Uses `components()` to resolve `.` and `..` without requiring the file to exist.
/// Verifies the normalized path does not escape the base directory.
///
/// ⚠️ **TOCTOU Warning**: Because this does not use `canonicalize()`, a symlink
/// placed between validation and actual write could bypass the sandbox.
/// Callers should perform a second `canonicalize()` check before writing.
pub fn resolve_write_path(base_dir: Option<&Path>, input: &str) -> Result<PathBuf, ToolError> {
    let base = base_dir.unwrap_or(Path::new("."));
    let base = base
        .canonicalize()
        .map_err(|e| ToolError::Io { message: format!("cannot resolve base_dir: {e}"), kind: e.kind() })?;

    let raw = if Path::new(input).is_absolute() {
        PathBuf::from(input)
    } else {
        base.join(input)
    };

    let mut normalized = PathBuf::new();
    for comp in raw.components() {
        match comp {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                normalized.push(comp);
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return Err(ToolError::invalid_input("path traversal detected"));
                }
            }
            std::path::Component::Normal(c) => {
                normalized.push(c);
            }
        }
    }

    if normalized.is_absolute() && !normalized.starts_with(&base) {
        return Err(ToolError::invalid_input("path traversal detected"));
    }

    Ok(normalized)
}
