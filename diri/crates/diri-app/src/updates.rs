//! The app-side driver for [`diri_updater`].
//!
//! Owns a tokio task that holds the [`Updater`], runs the blocking steps on
//! the blocking pool, and publishes a [`UpdateState`] the UI renders from. The
//! UI never calls the updater directly — it sends [`UpdateCommand`]s, so a
//! click cannot block a frame and two clicks cannot start two downloads.
//!
//! Update *checks* are automatic; downloading and installing are not. A
//! background check only lights the pill in the sidebar footer, matching the
//! "gentle reminders" behavior the Swift app gets from Sparkle: nothing steals
//! focus or restarts the app mid-session without being asked.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use diri_updater::{Release, StagedUpdate, UpdateError, Updater, UpdaterConfig};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, watch};

/// How long after launch the first background check runs. Long enough to stay
/// out of the way of startup work.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(20);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, Default, PartialEq)]
pub enum UpdatePhase {
    /// This build cannot update itself (unsigned, or not in a bundle). Carries
    /// the reason, shown in Settings so a dev build is not mistaken for a bug.
    Unsupported(String),
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading {
        release: Release,
        progress: f32,
    },
    /// Verified and staged; the swap happens on the next quit.
    Ready(Release),
    /// The helper is running and the app is about to be told to quit.
    Installing,
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UpdateState {
    pub phase: UpdatePhase,
    pub current_version: String,
    pub last_checked_unix: Option<u64>,
    /// True when the user asked for the running check, which is the only case
    /// where "You're up to date" is worth showing.
    pub user_initiated: bool,
}

impl UpdateState {
    /// The version the sidebar pill advertises, if there is one to advertise.
    pub fn pending_version(&self) -> Option<&str> {
        match &self.phase {
            UpdatePhase::Available(release)
            | UpdatePhase::Downloading { release, .. }
            | UpdatePhase::Ready(release) => Some(release.version.as_str()),
            _ => None,
        }
    }

    /// Whether the footer should show anything at all. A background check that
    /// found nothing stays silent.
    pub fn is_noteworthy(&self) -> bool {
        match &self.phase {
            UpdatePhase::Available(_)
            | UpdatePhase::Downloading { .. }
            | UpdatePhase::Ready(_)
            | UpdatePhase::Installing => true,
            UpdatePhase::Checking | UpdatePhase::UpToDate | UpdatePhase::Failed(_) => {
                self.user_initiated
            }
            UpdatePhase::Idle | UpdatePhase::Unsupported(_) => false,
        }
    }

    /// One line for the account footer and the Settings row.
    pub fn summary(&self) -> String {
        match &self.phase {
            UpdatePhase::Unsupported(_) => "Updates off for this build".to_owned(),
            UpdatePhase::Idle => format!("diri {}", self.current_version),
            UpdatePhase::Checking => "Checking for updates…".to_owned(),
            UpdatePhase::UpToDate => format!("diri {} is up to date", self.current_version),
            UpdatePhase::Available(release) => format!("Update to {}", release.version),
            UpdatePhase::Downloading { progress, .. } => {
                format!("Downloading… {}%", (progress * 100.0).round() as u32)
            }
            UpdatePhase::Ready(release) => format!("Restart to update to {}", release.version),
            UpdatePhase::Installing => "Restarting…".to_owned(),
            UpdatePhase::Failed(reason) => reason.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum UpdateCommand {
    Check {
        user_initiated: bool,
    },
    Download,
    Install,
    /// Never offer the pending version again.
    Skip,
    /// Clear a finished check's transient state (up to date / failed).
    Dismiss,
    SetAutomatic(bool),
}

/// UI-side handle: a state stream plus a command sink.
#[derive(Clone)]
pub struct UpdateHandle {
    state: watch::Receiver<UpdateState>,
    commands: mpsc::UnboundedSender<UpdateCommand>,
    // Preview mode keeps the watch open without spawning the updater service.
    _inert_state: Option<watch::Sender<UpdateState>>,
}

impl UpdateHandle {
    pub fn state(&self) -> UpdateState {
        self.state.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<UpdateState> {
        self.state.clone()
    }

    pub fn send(&self, command: UpdateCommand) {
        // A closed channel means the service task is gone; the UI has nothing
        // useful to do about that, and the state stream stops updating anyway.
        let _ = self.commands.send(command);
    }

    pub fn check(&self, user_initiated: bool) {
        self.send(UpdateCommand::Check { user_initiated });
    }
}

/// Starts the update service on `runtime` and returns the UI handle.
///
/// Never fails: a build that cannot update itself still gets a handle, parked
/// in [`UpdatePhase::Unsupported`], so no caller needs an `Option`.
pub fn spawn(runtime: &Arc<Runtime>, automatic: bool, skipped: Option<String>) -> UpdateHandle {
    let initial = UpdateState {
        current_version: CURRENT_VERSION.to_owned(),
        ..UpdateState::default()
    };
    let (state_tx, state_rx) = watch::channel(initial.clone());
    let (command_tx, command_rx) = mpsc::unbounded_channel();

    let updater = match UpdaterConfig::for_running_app(CURRENT_VERSION) {
        Ok(config) => Some(Arc::new(Updater::new(config))),
        Err(error) => {
            state_tx.send_replace(UpdateState {
                phase: UpdatePhase::Unsupported(error.to_string()),
                ..initial
            });
            None
        }
    };

    let _guard = runtime.enter();
    tokio::spawn(service(updater, automatic, skipped, state_tx, command_rx));
    UpdateHandle {
        state: state_rx,
        commands: command_tx,
        _inert_state: None,
    }
}

/// A stable, task-free update handle for deterministic UI previews and
/// performance probes. It neither touches the network nor creates a timer.
pub fn inert() -> UpdateHandle {
    let state = UpdateState {
        current_version: CURRENT_VERSION.to_owned(),
        ..UpdateState::default()
    };
    let (state_tx, state_rx) = watch::channel(state);
    let (command_tx, _command_rx) = mpsc::unbounded_channel();
    UpdateHandle {
        state: state_rx,
        commands: command_tx,
        _inert_state: Some(state_tx),
    }
}

struct Service {
    updater: Arc<Updater>,
    state: watch::Sender<UpdateState>,
    automatic: bool,
    /// Set once a check finds something, cleared when it is superseded.
    pending: Option<Release>,
    staged: Option<StagedUpdate>,
    skipped: Option<String>,
    busy: bool,
}

async fn service(
    updater: Option<Arc<Updater>>,
    automatic: bool,
    skipped: Option<String>,
    state: watch::Sender<UpdateState>,
    mut commands: mpsc::UnboundedReceiver<UpdateCommand>,
) {
    let Some(updater) = updater else {
        // Nothing to drive, but keep draining commands so a click in Settings
        // on an unsupported build is a no-op instead of a channel error.
        while commands.recv().await.is_some() {}
        return;
    };
    {
        let updater = Arc::clone(&updater);
        let _ = tokio::task::spawn_blocking(move || updater.clean_cache()).await;
    }

    let mut service = Service {
        updater,
        state,
        automatic,
        pending: None,
        staged: None,
        // Persisted in Prefs, so a skip outlives the session that made it.
        skipped: skipped.filter(|version| !version.is_empty()),
        busy: false,
    };

    let mut ticker = tokio::time::interval_at(
        tokio::time::Instant::now() + FIRST_CHECK_DELAY,
        CHECK_INTERVAL,
    );
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { return };
                service.handle(command).await;
            }
            _ = ticker.tick() => {
                if service.automatic {
                    service.check(false).await;
                }
            }
        }
    }
}

impl Service {
    async fn handle(&mut self, command: UpdateCommand) {
        match command {
            UpdateCommand::Check { user_initiated } => self.check(user_initiated).await,
            UpdateCommand::Download => self.download().await,
            UpdateCommand::Install => self.install(),
            UpdateCommand::Skip => {
                if let Some(release) = self.pending.take() {
                    self.skipped = Some(release.version);
                }
                self.staged = None;
                self.publish(UpdatePhase::Idle, false);
            }
            UpdateCommand::Dismiss => {
                let phase = match self.state.borrow().phase.clone() {
                    // A staged or offered update survives a dismiss; only the
                    // transient results clear.
                    phase @ (UpdatePhase::Available(_)
                    | UpdatePhase::Downloading { .. }
                    | UpdatePhase::Ready(_)) => phase,
                    _ => UpdatePhase::Idle,
                };
                self.publish(phase, false);
            }
            UpdateCommand::SetAutomatic(enabled) => self.automatic = enabled,
        }
    }

    async fn check(&mut self, user_initiated: bool) {
        // A staged update is the end of the line until the app restarts;
        // re-checking would only offer what is already on disk. Re-publish it
        // so a manual check still visibly answers rather than doing nothing.
        if let Some(staged) = &self.staged {
            self.publish(UpdatePhase::Ready(staged.release.clone()), user_initiated);
            return;
        }
        if self.busy {
            return;
        }
        self.busy = true;
        self.publish(UpdatePhase::Checking, user_initiated);

        let updater = Arc::clone(&self.updater);
        let skipped = self.skipped.clone();
        let found = tokio::task::spawn_blocking(move || updater.check(skipped.as_deref())).await;
        self.busy = false;
        self.touch_last_checked();

        match found {
            Ok(Ok(Some(release))) => {
                self.pending = Some(release.clone());
                self.publish(UpdatePhase::Available(release), user_initiated);
            }
            Ok(Ok(None)) => {
                self.pending = None;
                self.publish(UpdatePhase::UpToDate, user_initiated);
            }
            Ok(Err(error)) => self.fail(&error, user_initiated),
            Err(_) => self.publish(
                UpdatePhase::Failed("The update check stopped unexpectedly".to_owned()),
                user_initiated,
            ),
        }
    }

    async fn download(&mut self) {
        let Some(release) = self.pending.clone() else {
            return;
        };
        if self.busy {
            return;
        }
        self.busy = true;
        self.publish(
            UpdatePhase::Downloading {
                release: release.clone(),
                progress: 0.0,
            },
            true,
        );

        // Progress arrives on the blocking thread; funnel it through a channel
        // so only this task ever writes the state.
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        let updater = Arc::clone(&self.updater);
        let downloading = release.clone();
        let worker = tokio::task::spawn_blocking(move || {
            let archive = updater.download(&downloading, |fraction| {
                let _ = progress_tx.send(fraction);
            })?;
            updater.stage(&downloading, &archive)
        });
        tokio::pin!(worker);

        let staged = loop {
            tokio::select! {
                Some(fraction) = progress_rx.recv() => {
                    self.publish(
                        UpdatePhase::Downloading { release: release.clone(), progress: fraction },
                        true,
                    );
                }
                result = &mut worker => break result,
            }
        };
        self.busy = false;

        match staged {
            Ok(Ok(staged)) => {
                self.staged = Some(staged);
                self.publish(UpdatePhase::Ready(release), true);
            }
            Ok(Err(error)) => self.fail(&error, true),
            Err(_) => self.publish(
                UpdatePhase::Failed("The download stopped unexpectedly".to_owned()),
                true,
            ),
        }
    }

    fn install(&mut self) {
        let Some(staged) = &self.staged else {
            return;
        };
        match self.updater.install(staged) {
            // RootView watches for Installing and quits; the helper is already
            // waiting for this process to go away.
            Ok(()) => self.publish(UpdatePhase::Installing, true),
            Err(error) => self.fail(&error, true),
        }
    }

    fn fail(&mut self, error: &UpdateError, user_initiated: bool) {
        eprintln!("diri updater: {error}");
        self.publish(UpdatePhase::Failed(error.user_facing()), user_initiated);
    }

    fn publish(&self, phase: UpdatePhase, user_initiated: bool) {
        let previous = self.state.borrow().clone();
        self.state.send_replace(UpdateState {
            phase,
            user_initiated,
            ..previous
        });
    }

    fn touch_last_checked(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .ok();
        let previous = self.state.borrow().clone();
        self.state.send_replace(UpdateState {
            last_checked_unix: now,
            ..previous
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: &str) -> Release {
        Release {
            version: version.to_owned(),
            ..Release::default()
        }
    }

    fn state(phase: UpdatePhase, user_initiated: bool) -> UpdateState {
        UpdateState {
            phase,
            current_version: "0.4.2".to_owned(),
            last_checked_unix: None,
            user_initiated,
        }
    }

    #[test]
    fn a_quiet_background_check_shows_nothing() {
        assert!(!state(UpdatePhase::Checking, false).is_noteworthy());
        assert!(!state(UpdatePhase::UpToDate, false).is_noteworthy());
        assert!(!state(UpdatePhase::Failed("nope".into()), false).is_noteworthy());
        assert!(!state(UpdatePhase::Idle, false).is_noteworthy());
    }

    #[test]
    fn a_manual_check_reports_its_outcome_either_way() {
        assert!(state(UpdatePhase::UpToDate, true).is_noteworthy());
        assert!(state(UpdatePhase::Failed("nope".into()), true).is_noteworthy());
    }

    #[test]
    fn a_found_update_surfaces_even_from_a_background_check() {
        let found = state(UpdatePhase::Available(release("0.5.0")), false);
        assert!(found.is_noteworthy());
        assert_eq!(found.pending_version(), Some("0.5.0"));
        assert_eq!(found.summary(), "Update to 0.5.0");
    }

    #[test]
    fn a_dev_build_stays_silent_in_the_sidebar() {
        let unsupported = state(UpdatePhase::Unsupported("unsigned".into()), true);
        assert!(!unsupported.is_noteworthy());
        assert_eq!(unsupported.summary(), "Updates off for this build");
    }

    #[test]
    fn download_progress_reads_as_a_whole_percentage() {
        let downloading = state(
            UpdatePhase::Downloading {
                release: release("0.5.0"),
                progress: 0.436,
            },
            true,
        );
        assert_eq!(downloading.summary(), "Downloading… 44%");
        assert_eq!(downloading.pending_version(), Some("0.5.0"));
    }

    #[test]
    fn a_staged_update_asks_for_a_restart() {
        assert_eq!(
            state(UpdatePhase::Ready(release("0.5.0")), true).summary(),
            "Restart to update to 0.5.0"
        );
    }

    #[test]
    fn an_idle_build_just_names_its_version() {
        assert_eq!(state(UpdatePhase::Idle, false).summary(), "diri 0.4.2");
        assert_eq!(
            state(UpdatePhase::UpToDate, true).summary(),
            "diri 0.4.2 is up to date"
        );
    }

    #[test]
    fn inert_preview_handle_stays_idle_without_a_service_task() {
        let handle = inert();
        assert_eq!(handle.state().phase, UpdatePhase::Idle);
        assert!(!handle.subscribe().has_changed().expect("watch stays open"));
        handle.check(true);
        assert_eq!(handle.state().phase, UpdatePhase::Idle);
    }
}
