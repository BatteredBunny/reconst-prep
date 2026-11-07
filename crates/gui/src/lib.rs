use std::path::PathBuf;
mod app;
mod clips;
mod native;
mod preview;
mod settings;
use app::plural;

#[derive(Default, Debug)]
pub struct Start {
    pub inputs: Vec<PathBuf>,
    pub profile: Option<PathBuf>,
    pub out_dir: Option<PathBuf>,
}

/// Launch the native Dioxus/Blitz desktop window.
pub fn run(start: Start) -> anyhow::Result<()> {
    native::run(start)
}

#[cfg(test)]
mod screenshots;
