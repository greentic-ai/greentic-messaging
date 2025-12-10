use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Normalize a user-supplied path and ensure it stays within an allowed root.
/// Rejects absolute inputs and any path that escapes the root after
/// canonicalization.
pub fn normalize_under_root(root: &Path, candidate: &Path) -> Result<PathBuf> {
    if candidate.is_absolute() {
        anyhow::bail!("absolute paths are not allowed: {}", candidate.display());
    }

    let joined = root.join(candidate);
    let canon = joined
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", joined.display()))?;

    if !canon.starts_with(root) {
        anyhow::bail!(
            "path escapes root ({}): {}",
            root.display(),
            canon.display()
        );
    }

    Ok(canon)
}
