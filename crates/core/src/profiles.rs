// Never runs implicitly: only the `profiles` subcommand and the GUI profile browser reach the network.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};

/// `recursive=1` returns every path in one request, so search can be local.
const INDEX_URL: &str =
    "https://api.github.com/repos/gyroflow/lens_profiles/git/trees/main?recursive=1";
const RAW_BASE: &str = "https://raw.githubusercontent.com/gyroflow/lens_profiles/main/";

use crate::USER_AGENT;

/// Re-fetch the index when the cached copy is older than this.
const INDEX_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The GUI holds a spinner up until one of these returns, so a stalled connection must fail rather than wedge it.
const TIMEOUT: Duration = Duration::from_secs(30);

/// One profile in the upstream database.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProfileEntry {
    /// Path inside the repository, e.g. `DJI/DJI_O4 Pro_..._4k.json`.
    pub path: String,
}

/// `~/.cache/reconst-prep/lens-profiles` or the platform equivalent.
pub fn cache_dir() -> Result<PathBuf> {
    crate::paths::cache_dir("lens-profiles")
}

fn get(url: &str) -> Result<String> {
    ureq::get(url)
        // GitHub rejects requests without one.
        .header("User-Agent", USER_AGENT)
        .config()
        .timeout_global(Some(TIMEOUT))
        .build()
        .call()
        .with_context(|| format!("fetching {url}"))?
        .into_body()
        .read_to_string()
        .with_context(|| format!("reading response from {url}"))
}

/// `offline` never touches the network.
pub fn index(offline: bool, refresh: bool) -> Result<Vec<ProfileEntry>> {
    let path = cache_dir()?.join("index.json");
    let age = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok());
    let fresh = age.is_some_and(|a| a < INDEX_MAX_AGE);

    if !refresh
        && (fresh || offline)
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        return serde_json::from_str(&text).context("parsing cached profile index");
    }
    if offline {
        bail!(
            "no cached lens-profile index at {}. Run `reconst-prep profiles update` once with \
             network access",
            path.display()
        );
    }

    log::info!("fetching lens-profile index from {INDEX_URL}");
    let body = get(INDEX_URL)?;
    let tree: GitTree = serde_json::from_str(&body).context("parsing the GitHub tree listing")?;
    let mut entries: Vec<ProfileEntry> = tree
        .tree
        .into_iter()
        .filter(|n| n.kind == "blob" && n.path.to_ascii_lowercase().ends_with(".json"))
        .map(|n| ProfileEntry { path: n.path })
        .collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    if entries.is_empty() {
        bail!("the lens-profile index came back empty; upstream layout may have changed");
    }
    log::info!("lens-profile index updated: {} profiles", entries.len());
    let _ = crate::paths::write_atomic(&path, serde_json::to_string(&entries)?.as_bytes());
    Ok(entries)
}

#[derive(serde::Deserialize)]
struct GitTree {
    tree: Vec<GitNode>,
}

#[derive(serde::Deserialize)]
struct GitNode {
    path: String,
    #[serde(rename = "type")]
    kind: String,
}

/// Matched against the whole path, so a maker name filters as well as a model name.
pub fn search<'a>(entries: &'a [ProfileEntry], query: &str) -> Vec<&'a ProfileEntry> {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|t| t.to_ascii_lowercase())
        .collect();
    if terms.is_empty() {
        return entries.iter().collect();
    }
    entries
        .iter()
        .filter(|e| {
            let hay = e.path.to_ascii_lowercase();
            terms.iter().all(|t| hay.contains(t))
        })
        .collect()
}

/// A cached profile is reused; the manifest records the hash of whatever was used.
pub fn fetch(entry: &ProfileEntry) -> Result<PathBuf> {
    if entry.path.trim().is_empty() || !entry.path.ends_with(".json") {
        bail!(
            "{:?} is not a profile path; copy one verbatim from `reconst-prep profiles search`",
            entry.path
        );
    }
    let dir = cache_dir()?;
    // Flattened: a single directory is easier to point `--profile` at by hand.
    let local = dir.join(entry.path.replace('/', "_"));
    if local.is_file() {
        return Ok(local);
    }
    let url = format!("{RAW_BASE}{}", urlencode_path(&entry.path));
    log::info!(
        "downloading lens profile {} -> {}",
        entry.path,
        local.display()
    );
    let body = get(&url)?;
    // So a redirect or error page cannot masquerade as a cached profile.
    serde_json::from_str::<serde_json::Value>(&body)
        .with_context(|| format!("{} is not valid JSON", entry.path))?;
    crate::paths::write_atomic(&local, body.as_bytes())?;
    Ok(local)
}

/// RFC 3986 unreserved characters, plus `/`, which stays a path separator.
const PROFILE_PATH: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~')
    .remove(b'/');

/// Percent-encode profile paths (spaces, brackets, commas).
fn urlencode_path(path: &str) -> String {
    percent_encoding::utf8_percent_encode(path, PROFILE_PATH).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<ProfileEntry> {
        [
            "DJI/DJI_O4 Pro_stock_wide_4k.json",
            "GoPro/HERO11_wide.json",
        ]
        .iter()
        .map(|p| ProfileEntry {
            path: p.to_string(),
        })
        .collect()
    }

    #[test]
    fn search_requires_every_term() {
        let e = entries();
        assert_eq!(search(&e, "dji o4").len(), 1);
        assert_eq!(search(&e, "dji hero").len(), 0);
        assert_eq!(search(&e, "").len(), 2);
    }

    #[test]
    fn paths_are_encoded_but_keep_separators() {
        assert_eq!(
            urlencode_path("DJI/DJI_O4 Pro.json"),
            "DJI/DJI_O4%20Pro.json"
        );
    }
}
