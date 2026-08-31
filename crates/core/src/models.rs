// Weights are fetched only when someone picks a catalogue entry; every entry is hash-pinned and states its licence.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use sha2::{Digest as _, Sha256};

use crate::USER_AGENT;
use crate::paths::hex;

/// A stalled connection should fail rather than hold the picker open forever.
const TIMEOUT: Duration = Duration::from_secs(300);

/// One offerable model.
#[derive(Debug, Clone, Copy)]
pub struct CatalogueEntry {
    /// Cache filename, and the id the UI addresses it by.
    pub file: &'static str,
    pub name: &'static str,
    /// One line: what it is good for, and what it costs.
    pub description: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub bytes: u64,
    /// Shown in the picker; several of these are non-commercial.
    pub license: &'static str,
    pub source: &'static str,
}

impl CatalogueEntry {
    pub fn size_mb(&self) -> f64 {
        self.bytes as f64 / (1024.0 * 1024.0)
    }
}

/// Verified 2026-08-06: hashed, and the label map read back to confirm class 2 = sky and 12 = person.
pub const CATALOGUE: &[CatalogueEntry] = &[CatalogueEntry {
    file: "segformer-b0-ade-512.onnx",
    name: "SegFormer-B0 (ADE20K, 512)",
    description: "150 ADE20K classes, ~15 MB. Small and fast enough to run \
                  on every frame. That is why the sky and people paths were \
                  written against this label set.",
    url: "https://huggingface.co/Xenova/segformer-b0-finetuned-ade-512-512/resolve/main/onnx/model.onnx",
    sha256: "3e5c18a4be395f16646438d54c42377ddc202edfa33d5eced0c9506de75c44c2",
    bytes: 15_335_446,
    license: "NVIDIA Source Code License-NC (research / non-commercial only)",
    source: "huggingface.co/Xenova/segformer-b0-finetuned-ade-512-512 \
             (ONNX export of nvidia/segformer-b0-finetuned-ade-512-512)",
}];

/// `~/.cache/reconst-prep/models` or the platform equivalent.
pub fn cache_dir() -> Result<PathBuf> {
    crate::paths::cache_dir("models")
}

/// Every `.onnx` in the cache, sorted, catalogued or hand-imported alike.
pub fn cached() -> Vec<PathBuf> {
    let Ok(dir) = cache_dir() else {
        return vec![];
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("onnx"))
        })
        .collect();
    out.sort();
    out
}

fn local_path(entry: &CatalogueEntry) -> Result<PathBuf> {
    Ok(cache_dir()?.join(entry.file))
}

fn sha256_file(path: &Path) -> Result<String> {
    use std::io::Read as _;

    let mut file =
        std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 128 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .with_context(|| format!("reading {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

/// A present file is reused only when its hash matches, otherwise a truncated download would be permanent.
pub fn fetch(
    entry: &CatalogueEntry,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &crate::cancel::CancelToken,
) -> Result<PathBuf> {
    use std::io::{Read as _, Write as _};

    let dest = local_path(entry)?;
    if dest.is_file() && sha256_file(&dest).is_ok_and(|h| h == entry.sha256) {
        return Ok(dest);
    }

    log::info!(
        "downloading {} ({:.1} MB) from {}",
        entry.name,
        entry.size_mb(),
        entry.url
    );
    let response = ureq::get(entry.url)
        .header("User-Agent", USER_AGENT)
        .config()
        .timeout_global(Some(TIMEOUT))
        .build()
        .call()
        .with_context(|| format!("fetching {}", entry.url))?;

    // Renamed into place only after the hash matches, so a partial write can never look like a finished model.
    let tmp = dest.with_extension("onnx.part");
    let mut out =
        std::fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    let mut reader = response.into_body().into_reader();
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 128 * 1024];
    let mut done: u64 = 0;
    loop {
        if cancel.is_cancelled() {
            let _ = std::fs::remove_file(&tmp);
            bail!("cancelled");
        }
        let n = reader.read(&mut buf).context("reading the download")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n]).context("writing the download")?;
        done += n as u64;
        progress(done, entry.bytes);
    }
    out.flush().ok();
    drop(out);

    let got = hex(&hasher.finalize());
    if got != entry.sha256 {
        let _ = std::fs::remove_file(&tmp);
        bail!(
            "{} does not match its pinned hash (got {got}, expected {}). Nothing was \
             installed.",
            entry.name,
            entry.sha256
        );
    }
    std::fs::rename(&tmp, &dest).with_context(|| format!("installing {}", dest.display()))?;
    log::info!("installed {} -> {}", entry.name, dest.display());
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_entries_are_complete() {
        for e in CATALOGUE {
            assert!(e.file.ends_with(".onnx"), "{}: cache name", e.name);
            assert!(
                e.url.starts_with("https://"),
                "{}: weights over plain http",
                e.name
            );
            assert_eq!(e.sha256.len(), 64, "{}: not a sha256", e.name);
            assert!(
                e.sha256.chars().all(|c| c.is_ascii_hexdigit()),
                "{}: not hex",
                e.name
            );
            assert!(e.bytes > 0, "{}: no size", e.name);
            assert!(!e.license.trim().is_empty(), "{}: no licence", e.name);
            assert!(!e.source.trim().is_empty(), "{}: no provenance", e.name);
        }
    }

    #[test]
    fn cache_names_are_unique() {
        let mut names: Vec<&str> = CATALOGUE.iter().map(|e| e.file).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "two entries share a cache filename");
    }
}
