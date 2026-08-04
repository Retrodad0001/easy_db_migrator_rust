use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Signals a running migration that it should stop before the next script.
///
/// A run checks the token between scripts rather than interrupting one, so a
/// cancelled run leaves the script it was in the middle of either fully applied
/// or not applied at all. Clones share a single flag, so a clone moved into
/// another task cancels the run the original is driving.
#[derive(Debug, Clone)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Creates a token, taking whether it is already cancelled to begin with.
    pub fn new(cancelled: bool) -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(cancelled)),
        }
    }

    /// Cancels this token and every clone of it. Cancelling twice does nothing the
    /// second time.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Returns whether [`cancel`](Self::cancel) has been called on this token or on
    /// any clone of it.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}
