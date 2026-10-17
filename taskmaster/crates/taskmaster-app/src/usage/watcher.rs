use std::path::PathBuf;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

#[derive(Debug)]
pub(crate) enum TranscriptInvalidation {
    Paths(Vec<PathBuf>),
    Reconcile,
}

/// Owns the platform's native filesystem watcher (FSEvents on macOS). Its
/// callback only forwards paths; parsing and cache mutation stay on the usage
/// worker after a debounce.
pub(crate) struct TranscriptWatcher {
    _watcher: RecommendedWatcher,
    rx: mpsc::UnboundedReceiver<TranscriptInvalidation>,
}

impl TranscriptWatcher {
    pub(crate) fn new(roots: &[PathBuf]) -> notify::Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let invalidation = match event {
                    Ok(event) if matches!(event.kind, EventKind::Access(_)) => return,
                    Ok(event) if event.paths.is_empty() => TranscriptInvalidation::Reconcile,
                    Ok(event) => TranscriptInvalidation::Paths(event.paths),
                    Err(_) => TranscriptInvalidation::Reconcile,
                };
                let _ = tx.send(invalidation);
            })?;

        for root in roots.iter().filter(|root| root.is_dir()) {
            watcher.watch(root, RecursiveMode::Recursive)?;
        }
        Ok(Self {
            _watcher: watcher,
            rx,
        })
    }

    pub(crate) async fn recv(&mut self) -> Option<TranscriptInvalidation> {
        self.rx.recv().await
    }
}
