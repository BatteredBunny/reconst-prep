// Prepares datasets only, handing over PINHOLE intrinsics per clip. Links gyroflow-core (GPL-3.0-or-later); this is a derivative work.

pub mod cancel;
pub mod decode;
pub mod frame;
pub mod gray;
pub mod mask;
pub mod models;
pub mod output;
pub mod paths;
pub mod pipeline;
pub mod profiles;
pub mod seg;
pub mod select;
pub mod undistort;

/// Must match the `rev` in the workspace Cargo.toml; the golden-frame test is the tripwire for bumping it.
pub const GYROFLOW_CORE_REV: &str = "f6cd36bfd1a0cd0152c8b87cd5eead2a232824e1";

pub const TOOL_NAME: &str = "reconst-prep";
pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Every network request goes through here, so none goes out without the user agent GitHub insists on, or without a timeout.
pub(crate) fn http_get(
    url: &str,
    timeout: std::time::Duration,
) -> anyhow::Result<ureq::http::Response<ureq::Body>> {
    use anyhow::Context as _;
    ureq::get(url)
        .header(
            "User-Agent",
            concat!("reconst-prep/", env!("CARGO_PKG_VERSION")),
        )
        .config()
        .timeout_global(Some(timeout))
        .build()
        .call()
        .with_context(|| format!("fetching {url}"))
}

#[cfg(test)]
mod tests {
    /// A bump that misses `GYROFLOW_CORE_REV` would silently make every dataset's provenance wrong.
    #[test]
    fn the_recorded_gyroflow_rev_is_the_one_we_build_against() {
        // crates/core -> crates -> workspace root.
        let manifest = include_str!("../../../Cargo.toml");
        let declared = manifest
            .lines()
            .find(|l| l.starts_with("gyroflow-core = "))
            .expect("no gyroflow-core line in the workspace Cargo.toml");
        assert!(
            declared.contains(&format!("rev = \"{}\"", super::GYROFLOW_CORE_REV)),
            "GYROFLOW_CORE_REV is {}, but the workspace pins: {declared}",
            super::GYROFLOW_CORE_REV
        );
    }
}
