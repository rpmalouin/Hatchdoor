//! Schedule and trigger the WebDAV sync turn (work-packet Phase D trigger).
//!
//! A WebDAV-sourced Vault keeps a local mirror checkout that the
//! index/watcher/atomic-write layer operates on (ADR-01: the mirror is the
//! authoritative read path). This scheduler makes the background sync turn
//! actually run: it registers active WebDAV-sourced Vaults, fires a
//! `VaultWorkKind::WebDav` request when a Vault's poll interval comes due
//! (due immediately on activation, so the first turn creates the mirror and
//! pulls the vault), and re-arms the schedule from the turn's outcome —
//! bounded exponential backoff after a failure. It deliberately mirrors
//! `ManagedGitScheduler`'s tick model without any of its checkout-lease
//! machinery: WebDAV has no Git, so no lease.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::vault_registry::VaultId;
use crate::vault_work::{VaultWorkCoordinator, VaultWorkKind};

/// How often [`spawn_webdav_tick`] checks for due Vaults in production.
/// Controls how often the schedule is *checked*, not how often a Vault
/// actually syncs — keep it well below every Vault's own poll interval and
/// [`WEBDAV_BACKOFF_BASE`].
pub const WEBDAV_TICK_INTERVAL: Duration = Duration::from_secs(15);

/// Floor for a failed WebDAV turn's next attempt; doubles up to
/// [`WEBDAV_BACKOFF_MAX`], mirroring the managed-Git scheduler's backoff.
const WEBDAV_BACKOFF_BASE: Duration = Duration::from_secs(30);
const WEBDAV_BACKOFF_MAX: Duration = Duration::from_secs(60);

#[derive(Clone, Copy)]
struct WebDavScheduleState {
    next_attempt: Instant,
    backoff: Option<Duration>,
}

struct WebDavScheduleEntry {
    schedule: WebDavScheduleState,
    /// This Vault's own poll interval, read from
    /// `VaultSource::WebDav::poll_interval_secs` at activation.
    poll_interval: Duration,
}

/// Per-Vault schedule for WebDAV sync turns, owned by the server and driven
/// by one tick task.
pub struct WebDavScheduler {
    coordinator: VaultWorkCoordinator,
    entries: Mutex<BTreeMap<VaultId, WebDavScheduleEntry>>,
}

impl WebDavScheduler {
    pub fn new(coordinator: VaultWorkCoordinator) -> Self {
        Self {
            coordinator,
            entries: Mutex::new(BTreeMap::new()),
        }
    }

    /// Register a Vault so it participates in scheduled polling, due
    /// immediately (the first turn creates the mirror). Idempotent for the
    /// schedule itself: re-activating an already-tracked Vault leaves its
    /// current schedule (including an in-progress backoff) untouched — but
    /// `poll_interval` is always applied, even to an already-tracked Vault,
    /// so an edit that changes only the interval takes effect on the next
    /// re-arm (mirrors `ManagedGitScheduler::activate`).
    pub fn activate(&self, vault_id: VaultId, poll_interval: Duration) {
        let mut entries = self.entries.lock().expect("WebDAV scheduler poisoned");
        match entries.entry(vault_id) {
            std::collections::btree_map::Entry::Occupied(mut occupied) => {
                occupied.get_mut().poll_interval = poll_interval;
            }
            std::collections::btree_map::Entry::Vacant(vacant) => {
                vacant.insert(WebDavScheduleEntry {
                    schedule: WebDavScheduleState {
                        next_attempt: Instant::now(),
                        backoff: None,
                    },
                    poll_interval,
                });
            }
        }
    }

    /// Stop tracking a Vault (disabled, disconnected, retired, or its source
    /// changed away from WebDAV). Any coordinator-side pending work is
    /// discarded separately by `VaultWorkCoordinator::drain_vault`.
    pub fn deactivate(&self, vault_id: VaultId) {
        self.entries
            .lock()
            .expect("WebDAV scheduler poisoned")
            .remove(&vault_id);
    }

    /// Fire a WebDAV turn for every Vault whose schedule is due. Idempotent
    /// per Vault: a Vault with an already-active (or queued) WebDAV turn is
    /// skipped, so a turn that outlasts a tick can never pre-queue a
    /// zero-delay rerun that defeats `record_outcome`'s re-arm.
    pub fn tick(&self, now: Instant) {
        let due = {
            let entries = self.entries.lock().expect("WebDAV scheduler poisoned");
            entries
                .iter()
                .filter(|(_, entry)| entry.schedule.next_attempt <= now)
                .map(|(vault_id, _)| *vault_id)
                .collect::<Vec<_>>()
        };
        for vault_id in due {
            if self.coordinator.has_work(vault_id, VaultWorkKind::WebDav) {
                continue;
            }
            self.coordinator.request(vault_id, VaultWorkKind::WebDav);
        }
    }

    /// Re-arm the schedule from one turn's outcome: the Vault's own
    /// `poll_interval` after success, bounded exponential backoff after a
    /// failure (mirrors the managed-Git scheduler). No-op for a Vault the
    /// scheduler no longer tracks.
    pub fn record_outcome(&self, vault_id: VaultId, succeeded: bool) {
        let mut entries = self.entries.lock().expect("WebDAV scheduler poisoned");
        let Some(entry) = entries.get_mut(&vault_id) else {
            return;
        };
        entry.schedule = if succeeded {
            WebDavScheduleState {
                next_attempt: Instant::now() + entry.poll_interval,
                backoff: None,
            }
        } else {
            let next_backoff = entry
                .schedule
                .backoff
                .map_or(WEBDAV_BACKOFF_BASE, |previous| {
                    (previous * 2).min(WEBDAV_BACKOFF_MAX)
                });
            WebDavScheduleState {
                next_attempt: Instant::now() + next_backoff,
                backoff: Some(next_backoff),
            }
        };
    }
}

/// Spawn the periodic tick that keeps every tracked WebDAV Vault's schedule
/// moving forward. Aborting the returned handle is sufficient to stop it:
/// the task holds no resources of its own and issues only coordinator
/// requests, which have their own independent shutdown draining.
pub fn spawn_webdav_tick(
    scheduler: std::sync::Arc<WebDavScheduler>,
    tick_interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tick_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            scheduler.tick(Instant::now());
        }
    })
}
