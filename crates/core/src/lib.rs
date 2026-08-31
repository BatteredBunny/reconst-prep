// Prepares datasets only, handing over PINHOLE intrinsics per clip. Links gyroflow-core (GPL-3.0-or-later); this is a derivative work.

pub mod cancel;
pub mod decode;
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
pub const GYROFLOW_CORE_REV: &str = "b5e8828f82c150676e48a7c2e3db39c97392f606";

pub const TOOL_NAME: &str = "reconst-prep";
pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Sent with every outbound HTTP request (GitHub rejects requests without one).
pub const USER_AGENT: &str = concat!("reconst-prep/", env!("CARGO_PKG_VERSION"));

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
