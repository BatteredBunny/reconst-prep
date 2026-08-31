//! `reconst-prep profiles`: the only subcommand that touches the network.

use anyhow::Result;

use reconst_prep_core::profiles;

use crate::cli::ProfileCmd;
use crate::out;

pub fn run(cmd: ProfileCmd) -> Result<()> {
    match cmd {
        ProfileCmd::Search { terms, offline } => {
            let entries = profiles::index(offline, false)?;
            let hits = profiles::search(&entries, &terms.join(" "));
            if hits.is_empty() {
                eprintln!("no profiles match. Try fewer or broader terms.");
                return Ok(());
            }
            out::listing(|w| {
                for e in &hits {
                    writeln!(w, "{}", e.path)?;
                }
                Ok(())
            })?;
            eprintln!(
                "\n{} of {} profiles. Download one with:\n  reconst-prep profiles get \"{}\"",
                hits.len(),
                entries.len(),
                hits[0].path
            );
        }
        ProfileCmd::Get { path } => {
            let local = profiles::fetch(&profiles::ProfileEntry { path })?;
            out::listing(|w| writeln!(w, "{}", local.display()))?;
        }
        ProfileCmd::Update => {
            let entries = profiles::index(false, true)?;
            eprintln!("cached {} profiles", entries.len());
        }
    }
    Ok(())
}
