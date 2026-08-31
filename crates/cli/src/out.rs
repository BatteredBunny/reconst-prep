//! `println!` panics when the reader has gone, so `profiles search ... | head` must not use it.

use std::io::{ErrorKind, Write};

/// A closed pipe ends the program quietly with success: `| head` is a normal way to read a listing.
pub fn listing(f: impl FnOnce(&mut dyn Write) -> std::io::Result<()>) -> anyhow::Result<()> {
    let mut out = std::io::stdout().lock();
    match f(&mut out).and_then(|()| out.flush()) {
        Err(e) if e.kind() == ErrorKind::BrokenPipe => std::process::exit(0),
        r => Ok(r?),
    }
}
