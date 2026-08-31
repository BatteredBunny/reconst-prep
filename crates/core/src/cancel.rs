use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Cheap to clone and to poll, so worker loops can check it per frame.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// A distinct name so call sites do not read as if cancellation were wired up when it is not.
    pub fn never() -> Self {
        Self::new()
    }

    /// Returns whether it was *already* cancelled, which is how the CLI tells a second ^C from the first.
    pub fn cancel(&self) -> bool {
        self.0.swap(true, Ordering::Relaxed)
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// For `?` at the points where work must not be recorded as finished.
    pub fn check(&self) -> anyhow::Result<()> {
        if self.is_cancelled() {
            Err(Cancelled.into())
        } else {
            Ok(())
        }
    }
}

/// A distinct type so a frontend can tell an interruption from a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// Was this error (or anything it wraps) a cancellation?
pub fn is_cancelled(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<Cancelled>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context as _;

    #[test]
    fn clones_share_one_flag_and_cancel_reports_the_previous_state() {
        let a = CancelToken::new();
        let b = a.clone();
        assert!(!a.is_cancelled());
        assert!(!a.cancel(), "first cancel sees an uncancelled token");
        assert!(b.is_cancelled(), "the clone observes it");
        assert!(b.cancel(), "a second cancel reports it was already set");
    }

    /// `never()` is only about intent; a caller that later grows a stop button needs no new type.
    #[test]
    fn never_starts_uncancelled() {
        assert!(!CancelToken::never().is_cancelled());
    }

    /// A cancellation must stay recognisable through added context, or the CLI reports it as a failed run.
    #[test]
    fn cancellation_is_recognisable_through_context() {
        let token = CancelToken::new();
        assert!(token.check().is_ok());
        token.cancel();
        let e = token
            .check()
            .context("processing clip b")
            .expect_err("cancelled");
        assert!(is_cancelled(&e));
        assert!(!is_cancelled(&anyhow::anyhow!("disk full")));
    }
}
