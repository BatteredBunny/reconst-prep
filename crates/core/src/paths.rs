//! `etcetera` over `directories`/`dirs`: its paths match the old hand-rolled cascade, so adopting it moved no files.

use std::path::PathBuf;

use anyhow::{Context, Result};
use etcetera::BaseStrategy;

fn strategy() -> Result<impl BaseStrategy> {
    etcetera::choose_base_strategy().context("no home directory")
}

/// Created if missing.
pub fn cache_dir(sub: &str) -> Result<PathBuf> {
    let dir = strategy()?.cache_dir().join(crate::TOOL_NAME).join(sub);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

/// `~/.config/reconst-prep` (`%APPDATA%\reconst-prep`). Not created here.
pub fn config_dir() -> Result<PathBuf> {
    Ok(strategy()?.config_dir().join(crate::TOOL_NAME))
}

/// Windows has no state folder, so this falls back to the cache base, NOT `data_dir()`, which is Roaming and would move existing files.
pub fn state_dir() -> Result<PathBuf> {
    let s = strategy()?;
    let base = s.state_dir().unwrap_or_else(|| s.cache_dir());
    Ok(base.join(crate::TOOL_NAME))
}

/// Hand-rolled because the two sha2 generations disagree about whether their output implements `LowerHex`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Temp file then rename, so a crash never leaves a half-written file; a truncated manifest makes a dataset unresumable.
pub fn write_atomic(path: &std::path::Path, contents: &[u8]) -> Result<()> {
    use std::io::Write as _;

    let dir = path.parent().context("no parent directory to write into")?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("creating a temp file in {}", dir.display()))?;
    tmp.write_all(contents)
        .with_context(|| format!("writing {}", path.display()))?;
    let _ = tmp.as_file().sync_all();
    tmp.persist(path)
        .with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}
