use std::fs::File;
use std::io::copy;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tempfile::{TempDir, tempdir};
use zip::ZipArchive;

#[allow(dead_code)]
pub struct ExtractedPack {
    pub temp_dir: TempDir,
    pub root: PathBuf,
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub fn discover_packs(explicit: &[PathBuf], packs_dir: Option<&Path>) -> Result<Vec<PathBuf>> {
    let mut discovered = Vec::new();

    for path in explicit {
        let canonical = normalize_path(path);
        discovered.push(canonical);
    }

    if let Some(dir) = packs_dir {
        for entry in std::fs::read_dir(dir)
            .with_context(|| format!("failed to read packs directory {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("gtpack"))
                .unwrap_or(false)
            {
                discovered.push(normalize_path(&path));
            }
        }
    }

    discovered.sort();
    discovered.dedup();
    Ok(discovered)
}

pub fn extract_pack_to_temp(gtpack: &Path) -> Result<ExtractedPack> {
    let temp_dir = tempdir().context("failed to create temp dir for pack extraction")?;
    let root = temp_dir.path().to_path_buf();

    let file =
        File::open(gtpack).with_context(|| format!("failed to open {}", gtpack.display()))?;
    let mut archive = ZipArchive::new(file).with_context(|| "invalid gtpack archive")?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name();
        if name.ends_with('/') {
            std::fs::create_dir_all(root.join(name))?;
            continue;
        }
        if let Some(parent) = Path::new(name).parent() {
            std::fs::create_dir_all(root.join(parent))?;
        }
        let mut outfile =
            File::create(root.join(name)).with_context(|| format!("failed to create {name}"))?;
        copy(&mut entry, &mut outfile)?;
    }

    Ok(ExtractedPack { temp_dir, root })
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use zip::{ZipWriter, write::FileOptions};

    use super::*;
    fn create_gtpack(path: &Path) -> Result<()> {
        let file = File::create(path)?;
        let mut writer = ZipWriter::new(file);
        let options: FileOptions<'_, ()> = FileOptions::default();
        writer.start_file("manifest.cbor", options)?;
        writer.write_all(b"manifest")?;
        writer.start_file("contents/data.txt", options)?;
        writer.write_all(b"value")?;
        writer.finish()?;
        Ok(())
    }

    #[test]
    fn discover_explicit_and_dir() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let pack = dir.path().join("sample.gtpack");
        create_gtpack(&pack)?;
        let found = discover_packs(std::slice::from_ref(&pack), Some(dir.path()))?;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0], pack);
        Ok(())
    }

    #[test]
    fn extract_pack_creates_files() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let pack = dir.path().join("extract.gtpack");
        create_gtpack(&pack)?;
        let extracted = extract_pack_to_temp(&pack)?;
        assert!(extracted.root.join("manifest.cbor").exists());
        assert!(extracted.root.join("contents/data.txt").exists());
        Ok(())
    }
}
