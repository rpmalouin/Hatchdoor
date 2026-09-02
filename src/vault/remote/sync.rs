//! Recursive remote↔mirror sync for a WebDAV Vault source.
//!
//! A WebDAV source keeps a **local mirror checkout** (the authoritative read
//! path per ADR-01; see `vault_registry::source_vault_path`) and this module
//! reconciles the remote WebDAV collection with that mirror during a sync
//! turn:
//!
//! - **Pull / refresh:** files on the remote that are missing locally are
//!   downloaded (`GET`); files present on both sides whose remote
//!   fingerprint (size + etag, as reported by the PROPFIND parser) changed
//!   since the last successful turn are downloaded again and overwrite the
//!   mirror copy, so remote edits reach the mirror.
//! - **Delete:** a file or dir present in the mirror but no longer listed on
//!   the remote is a stale remnant of a remote deletion and is removed from
//!   the mirror — never re-uploaded.
//! - **Restricted push:** only a local file that Hatchdoor itself created or
//!   modified since the last successful sync turn (`mtime > last_sync_at`)
//!   is uploaded (`PUT`). This engine is pull-side: remote `DELETE` is never
//!   issued.
//! - **404 tolerance:** a collection that answers 404 while the walk is in
//!   flight is treated as empty (deleted); its local subtree is reconciled
//!   away instead of aborting the turn.
//!
//! The fingerprints that drive those decisions persist in a per-vault
//! sidecar, `<mirror>/.hatchdoor/webdav-sync.json` (see [`SyncState`]),
//! written atomically at the end of a successful turn. Every name starting
//! with `.hatchdoor` is skipped by the push/prune logic and is not a note.
//!
//! It mirrors Hatchdoor's `ManagedGit` execution model: run as a background
//! `VaultWorkKind` turn under the Vault's mutation lock, with the mirror being
//! what the index/watcher/write layer already operate on. Phase D of the WebDAV
//! work packet; it never serves a per-request note read directly. The turn is
//! triggered and scheduled by [`webdav_scheduler`] (initial sync due on
//! activation, then every `poll_interval_secs`).

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::vault::remote::{WebDavClient, WebDavEntry, WebDavError};

/// Outcome of one WebDAV sync turn.
#[derive(Debug, Clone, Default)]
pub struct WebDavSyncOutcome {
    /// Remote files downloaded because they were missing from the mirror.
    pub pulled: usize,
    /// Remote files re-downloaded because their remote fingerprint changed.
    pub refreshed: usize,
    /// Local files uploaded to the remote (`PUT`).
    pub pushed: usize,
    /// Stale mirror files removed (remnants of remote deletions).
    pub deleted: usize,
    /// Remote collections created (`MKCOL`) as parents of pushed files.
    pub created_dirs: usize,
    /// Non-fatal errors (a failed `GET`/`PUT`/`MKCOL`, a failed local delete);
    /// the turn still counts as successful and retries next poll.
    pub errors: usize,
}

/// Directory (under the mirror root) holding Hatchdoor's own per-vault sync
/// state. Every `.hatchdoor*` name is skipped by the push/prune logic.
const STATE_REL_DIR: &str = ".hatchdoor";
const STATE_FILE_NAME: &str = "webdav-sync.json";
const STATE_VERSION: u32 = 1;

/// Fingerprint of one remote file: size + etag exactly as returned by the
/// PROPFIND parser. `etag` is `None` when the server reported none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredFile {
    size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
}

/// Per-vault sync state, persisted at `<mirror>/.hatchdoor/webdav-sync.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SyncState {
    version: u32,
    last_sync_at_unix: u64,
    files: BTreeMap<String, StoredFile>,
}

/// Mutable state shared across one sync turn's recursive pass. The recursion
/// is boxed/async, so the pass lives behind a mutex; no lock is ever held
/// across an `.await`.
struct SyncPass {
    state: SyncState,
    /// Rels of files the remote listed this turn (plus files just pushed), so
    /// end-of-turn pruning drops only fingerprints of files the remote no
    /// longer has.
    seen_files: HashSet<String>,
    /// Collection rels known to exist on the remote this turn (listed or just
    /// `MKCOL`ed), so ancestor creation before a `PUT` only hits dirs that
    /// are genuinely missing.
    remote_dirs: HashSet<String>,
    outcome: WebDavSyncOutcome,
}

fn lock_pass(pass: &Arc<Mutex<SyncPass>>) -> MutexGuard<'_, SyncPass> {
    pass.lock().expect("webdav sync pass mutex poisoned")
}

/// Boxed-recursive entry points (recursive async fns must be boxed).
pub struct Sync;

impl Sync {
    /// Reconcile the whole vault: load the persisted state (absent/corrupt ⇒
    /// first run), walk the remote collection against the mirror, then — only
    /// after a fully successful walk — prune fingerprints of remote-gone
    /// files, stamp `last_sync_at_unix`, and atomically persist the sidecar.
    /// A failed walk leaves the persisted state untouched.
    pub async fn sync_once(
        client: &WebDavClient,
        mirror_root: &Path,
        root_rel: &str,
        outcome: &mut WebDavSyncOutcome,
    ) -> Result<(), WebDavError> {
        let state_path = mirror_root.join(STATE_REL_DIR).join(STATE_FILE_NAME);
        let pass = Arc::new(Mutex::new(SyncPass {
            state: load_state(&state_path),
            seen_files: HashSet::new(),
            remote_dirs: HashSet::new(),
            outcome: WebDavSyncOutcome::default(),
        }));

        Self::reconcile_dir(client, mirror_root, root_rel, pass.clone()).await?;

        let mut guard = lock_pass(&pass);
        let seen_files = std::mem::take(&mut guard.seen_files);
        guard.state.files.retain(|rel, _| seen_files.contains(rel));
        guard.seen_files = seen_files;
        guard.state.last_sync_at_unix = now_unix_secs();
        if let Err(error) = persist_state(&state_path, &guard.state) {
            // Never fail the sync over the sidecar; the next successful turn
            // re-fingerprints from the remote listing anyway.
            guard.outcome.errors += 1;
            tracing::warn!(
                path = %state_path.display(),
                error = %error,
                "failed to persist webdav sync state; next turn re-derives it"
            );
        }
        *outcome = guard.outcome.clone();
        Ok(())
    }

    /// Reconcile one remote collection `dir_rel` against its local mirror
    /// directory, then reconcile mirror children the remote did not list.
    async fn reconcile_dir(
        client: &WebDavClient,
        mirror_root: &Path,
        dir_rel: &str,
        pass: Arc<Mutex<SyncPass>>,
    ) -> Result<(), WebDavError> {
        // A 404 on a SUBcollection means it was deleted while the turn runs:
        // treat it as an empty listing so the local-only pass below removes
        // the mirror subtree instead of aborting the turn. Any other listing
        // error is turn-fatal — and so is a 404 on the ROOT collection: that
        // means the source itself is gone or misconfigured, and treating it
        // as empty would classify the whole mirror as stale remnants and
        // delete it (first run: everything has mtime <= last_sync_at).
        let entries = match client.list(dir_rel).await {
            Ok(entries) => entries,
            Err(error) if is_http_404(&error) && !dir_rel.is_empty() => Vec::new(),
            Err(error) => return Err(error),
        };

        // Remote pass: pull missing files, refresh changed ones, recurse dirs.
        let local_dir = mirror_root.join(dir_rel);
        let mut remote_names = HashSet::new();
        for entry in &entries {
            if entry.path.is_empty() {
                continue; // the collection itself
            }
            remote_names.insert(entry.path.clone());
            let rel = join_rel(dir_rel, &entry.path);

            if entry.is_dir {
                lock_pass(&pass).remote_dirs.insert(rel.clone());
                let local = local_dir.join(&entry.path);
                if !local.is_dir() {
                    fs::create_dir_all(&local).map_err(|e| {
                        WebDavError(format!("webdav sync: mkdir {}: {e}", local.display()))
                    })?;
                }
                let fut = Self::reconcile_dir(client, mirror_root, &rel, pass.clone());
                Box::pin(fut).await?;
                continue;
            }

            // Remote file. Its rel counts as seen this turn so end-of-turn
            // pruning keeps (or drops) its fingerprint accordingly.
            lock_pass(&pass).seen_files.insert(rel.clone());
            let stored = lock_pass(&pass).state.files.get(&rel).cloned();
            let local = local_dir.join(&entry.path);

            if !local.is_file() {
                // Missing locally -> pull.
                match Self::fetch_and_write(client, &rel, &local).await {
                    Ok(()) => {
                        let mut guard = lock_pass(&pass);
                        guard.state.files.insert(rel.clone(), fingerprint_of(entry));
                        guard.outcome.pulled += 1;
                    }
                    // Deleted between the listing and the GET: nothing to mirror.
                    Err(error) if is_http_404(&error) => {}
                    Err(_) => lock_pass(&pass).outcome.errors += 1,
                }
            } else if fingerprint_changed(stored.as_ref(), entry.size, entry.etag.as_deref()) {
                // Exists on both sides and the remote fingerprint changed ->
                // refresh. No stored fingerprint (first run, or a pushed file
                // the server reports an etag for) also counts as changed, so
                // this heals pre-existing staleness exactly once.
                match Self::fetch_and_write(client, &rel, &local).await {
                    Ok(()) => {
                        let mut guard = lock_pass(&pass);
                        guard.state.files.insert(rel.clone(), fingerprint_of(entry));
                        guard.outcome.refreshed += 1;
                    }
                    // Deleted between listing and GET: keep the current local
                    // copy; the next turn's local-only pass removes it.
                    Err(error) if is_http_404(&error) => {}
                    Err(_) => lock_pass(&pass).outcome.errors += 1,
                }
            }
        }

        Self::reconcile_local_children(client, mirror_root, dir_rel, &remote_names, pass.clone())
            .await?;
        Ok(())
    }

    /// Reconcile local mirror children of `dir_rel` whose names the remote
    /// did not list this turn. A directory is emptied by recursion and then
    /// removed; a file that Hatchdoor modified since the last successful sync
    /// is pushed, otherwise it is a stale remnant of a remote deletion and is
    /// removed locally. `.hatchdoor*` names are always skipped.
    async fn reconcile_local_children(
        client: &WebDavClient,
        mirror_root: &Path,
        dir_rel: &str,
        remote_names: &HashSet<String>,
        pass: Arc<Mutex<SyncPass>>,
    ) -> Result<(), WebDavError> {
        let local_dir = mirror_root.join(dir_rel);
        let Ok(read) = fs::read_dir(&local_dir) else {
            return Ok(());
        };
        for item in read.flatten() {
            let path = item.path();
            let Ok(name) = item.file_name().into_string() else {
                continue; // non-UTF8 filename: never touched
            };
            if is_hatchdoor_name(&name) {
                continue; // Hatchdoor's own files (the state sidecar) never sync
            }
            if remote_names.contains(&name) {
                continue; // the remote lists it; the remote pass handled it
            }
            let rel = join_rel(dir_rel, &name);
            let file_type = match item.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                // The remote no longer lists this dir (a depth-1 listing
                // would have shown a dir that still exists). Reconcile its
                // contents, then remove it once emptied. Never MKCOL it.
                let fut = Self::reconcile_dir(client, mirror_root, &rel, pass.clone());
                Box::pin(fut).await?;
                let _ = fs::remove_dir(&path);
                continue;
            }
            if !file_type.is_file() {
                // Symlinks and specials are never followed, pushed, or deleted.
                continue;
            }
            let last_sync_at_unix = lock_pass(&pass).state.last_sync_at_unix;
            let Some(mtime_unix) = mtime_unix_secs(&path) else {
                lock_pass(&pass).outcome.errors += 1;
                continue;
            };
            if is_new_since_last_sync(mtime_unix, last_sync_at_unix) {
                Self::push_file(client, &rel, &path, pass.clone()).await;
            } else {
                match fs::remove_file(&path) {
                    Ok(()) => lock_pass(&pass).outcome.deleted += 1,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => lock_pass(&pass).outcome.errors += 1,
                }
            }
        }
        Ok(())
    }

    /// Upload `path` (a local file Hatchdoor created since the last successful
    /// sync) to `rel`, creating any missing remote ancestor collections first.
    ///
    /// On success a provisional fingerprint (`etag: None`, local size) is
    /// recorded: the `PUT` response does not carry the server etag, so the
    /// next turn's fingerprint comparison refreshes once if the server
    /// reports a different etag — and not at all when the server reports no
    /// etag and the size matches.
    async fn push_file(client: &WebDavClient, rel: &str, path: &Path, pass: Arc<Mutex<SyncPass>>) {
        if Self::ensure_remote_parents(client, rel, &pass)
            .await
            .is_err()
        {
            lock_pass(&pass).outcome.errors += 1;
            return;
        }
        let Ok(bytes) = fs::read(path) else {
            lock_pass(&pass).outcome.errors += 1;
            return;
        };
        match client.put(rel, &bytes, None).await {
            Ok(()) => {
                let mut guard = lock_pass(&pass);
                // The file is on the remote now, so it counts as seen this
                // turn and keeps its (provisional) fingerprint.
                guard.seen_files.insert(rel.to_string());
                guard.state.files.insert(
                    rel.to_string(),
                    StoredFile {
                        size: bytes.len() as u64,
                        etag: None,
                    },
                );
                guard.outcome.pushed += 1;
            }
            Err(_) => lock_pass(&pass).outcome.errors += 1,
        }
    }

    /// Ensure each ancestor collection of `file_rel` exists on the remote by
    /// MKCOL'ing the ones not seen this turn, shallowest first (a MKCOL needs
    /// its parent to exist already). MKCOL on an existing collection answers
    /// 405/301 — tolerated and treated as success.
    async fn ensure_remote_parents(
        client: &WebDavClient,
        file_rel: &str,
        pass: &Arc<Mutex<SyncPass>>,
    ) -> Result<(), WebDavError> {
        for dir_rel in ancestor_dirs(file_rel) {
            let already_remote = lock_pass(pass).remote_dirs.contains(&dir_rel);
            if already_remote {
                continue;
            }
            match client.mkdir(&dir_rel).await {
                Ok(()) => {
                    let mut guard = lock_pass(pass);
                    guard.remote_dirs.insert(dir_rel);
                    guard.outcome.created_dirs += 1;
                }
                Err(error) if is_mkcol_already_exists(&error) => {
                    lock_pass(pass).remote_dirs.insert(dir_rel);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// `GET` `rel` and write the bytes to `local`, creating parent dirs as
    /// needed. The returned `Err` keeps the remote error so callers can
    /// tolerate a 404 (file deleted between the listing and the `GET`).
    async fn fetch_and_write(
        client: &WebDavClient,
        rel: &str,
        local: &Path,
    ) -> Result<(), WebDavError> {
        let bytes = client.get(rel).await?;
        if let Some(parent) = local.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                WebDavError(format!("webdav sync: mkdir {}: {e}", parent.display()))
            })?;
        }
        fs::write(local, &bytes)
            .map_err(|e| WebDavError(format!("webdav sync: write {}: {e}", local.display())))?;
        Ok(())
    }
}

/// Join a child name onto a directory rel to form a vault-root-relative rel
/// with `/` separators (an empty `dir_rel` is the vault root).
fn join_rel(dir_rel: &str, child: &str) -> String {
    if dir_rel.is_empty() {
        child.to_string()
    } else {
        format!("{}/{}", dir_rel.trim_end_matches('/'), child)
    }
}

/// A `.hatchdoor*` name is Hatchdoor's own (the sync sidecar, markers):
/// never pushed, never deleted, never treated as a note.
fn is_hatchdoor_name(name: &str) -> bool {
    name.starts_with(".hatchdoor")
}

/// The fingerprint to record for a remote file this turn.
fn fingerprint_of(entry: &WebDavEntry) -> StoredFile {
    StoredFile {
        size: entry.size,
        etag: entry.etag.clone(),
    }
}

/// True when a stored fingerprint no longer matches the remote file: no
/// stored fingerprint (first run / freshly pushed), or size/etag differ
/// (a missing etag is never equal to a reported one).
fn fingerprint_changed(
    stored: Option<&StoredFile>,
    remote_size: u64,
    remote_etag: Option<&str>,
) -> bool {
    match stored {
        None => true,
        Some(stored) => stored.size != remote_size || stored.etag.as_deref() != remote_etag,
    }
}

/// Whether a local file whose remote counterpart is gone was created or
/// modified by Hatchdoor after the last successful sync turn (`mtime >`
/// `last_sync_at`) and therefore must be pushed, as opposed to a stale
/// remnant of a remote deletion (`mtime <= last_sync_at`) that is removed.
fn is_new_since_last_sync(mtime_unix: u64, last_sync_at_unix: u64) -> bool {
    mtime_unix > last_sync_at_unix
}

/// Direct ancestor collection rels of `file_rel`, shallowest first.
fn ancestor_dirs(file_rel: &str) -> Vec<String> {
    let mut dirs = Vec::new();
    let mut segments = file_rel.split('/');
    segments.next_back(); // the file name itself
    let mut prefix = String::new();
    for segment in segments {
        if segment.is_empty() {
            continue;
        }
        if prefix.is_empty() {
            prefix = segment.to_string();
        } else {
            prefix.push('/');
            prefix.push_str(segment);
        }
        dirs.push(prefix.clone());
    }
    dirs
}

/// Last-modification time of `path` in whole unix seconds (`None` when the
/// file cannot be stat'ed or the clock is not readable).
fn mtime_unix_secs(path: &Path) -> Option<u64> {
    let metadata = fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    Some(
        modified
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs()),
    )
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Load the persisted state. Absent, corrupt, or wrong-version files are
/// treated as absent — the first-run rule below. A state-write failure never
/// fails a sync turn; see `sync_once`.
fn load_state(path: &Path) -> SyncState {
    let parsed = fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<SyncState>(&text).ok());
    match parsed {
        Some(state) if state.version == STATE_VERSION => state,
        _ => first_run_state(),
    }
}

/// First-run state: `last_sync_at_unix` = now and `files = {}`.
///
/// Consequence: every pre-existing local-only file has `mtime <= last_sync_at`
/// and is removed on the first turn — correct for this deployment because the
/// old engine re-uploaded every local-only file each turn, so one present at
/// upgrade time can only be a stale remnant of a remote deletion. Remote files
/// that exist locally with no stored fingerprint are refreshed (one `GET`)
/// once, which heals any pre-existing staleness.
fn first_run_state() -> SyncState {
    SyncState {
        version: STATE_VERSION,
        last_sync_at_unix: now_unix_secs(),
        files: BTreeMap::new(),
    }
}

/// Persist `state` atomically: write a temp file in the same directory, then
/// rename over the target. Callers never fail a sync turn on an error here.
fn persist_state(path: &Path, state: &SyncState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("json.tmp");
    let json = serde_json::to_vec(state).expect("sync state serializes: only strings and integers");
    fs::write(&temp, json)?;
    fs::rename(temp, path)?;
    Ok(())
}

/// Whether a WebDAV error is an HTTP 404 (collection/file not found). The
/// error is an opaque string (`WebDavError(String)`); reqwest includes
/// "404 Not Found" in it.
fn is_http_404(error: &WebDavError) -> bool {
    error.0.contains("404")
}

/// Whether a MKCOL error means the collection already exists (rclone answers
/// 405 Method Not Allowed, some servers 301). Treated as success.
fn is_mkcol_already_exists(error: &WebDavError) -> bool {
    error.0.contains("405") || error.0.contains("301")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(size: u64, etag: Option<&str>) -> StoredFile {
        StoredFile {
            size,
            etag: etag.map(str::to_string),
        }
    }

    #[test]
    fn fingerprint_changed_compares_size_and_etag() {
        // No stored fingerprint (first run / freshly pushed): always changed.
        assert!(fingerprint_changed(None, 10, Some("\"a\"")));
        // Same size and same etag -> unchanged.
        let same = stored(10, Some("\"a\""));
        assert!(!fingerprint_changed(Some(&same), 10, Some("\"a\"")));
        // Etag differs (server reports a new one) -> changed.
        assert!(fingerprint_changed(Some(&same), 10, Some("\"b\"")));
        // Size differs -> changed even when the etag matches.
        assert!(fingerprint_changed(Some(&same), 11, Some("\"a\"")));
        // No etag reported on either side: size is the only signal.
        let no_etag = stored(10, None);
        assert!(!fingerprint_changed(Some(&no_etag), 10, None));
        assert!(fingerprint_changed(Some(&no_etag), 11, None));
        // One side reports an etag and the other does not -> changed.
        assert!(fingerprint_changed(Some(&no_etag), 10, Some("\"a\"")));
        assert!(fingerprint_changed(Some(&same), 10, None));
    }

    #[test]
    fn mtime_decision_pushes_only_files_newer_than_last_sync() {
        assert!(is_new_since_last_sync(11, 10));
        assert!(!is_new_since_last_sync(10, 10)); // equal: predates the last sync
        assert!(!is_new_since_last_sync(9, 10));
    }

    #[test]
    fn hatchdoor_names_are_always_skipped() {
        assert!(is_hatchdoor_name(".hatchdoor"));
        assert!(is_hatchdoor_name(".hatchdoor/webdav-sync.json"));
        assert!(is_hatchdoor_name(".hatchdoor-cache.sqlite3"));
        assert!(is_hatchdoor_name(".hatchdoor-trash"));
        assert!(is_hatchdoor_name(".hatchdoorish.md")); // prefix rule, no separator
        assert!(!is_hatchdoor_name("notes.md"));
        assert!(!is_hatchdoor_name(".obsidian/app.json")); // only `.hatchdoor` is reserved
    }

    #[test]
    fn state_round_trips_through_the_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".hatchdoor").join("webdav-sync.json");
        let state = SyncState {
            version: 1,
            last_sync_at_unix: 1_788_360_000,
            files: BTreeMap::from([
                ("Home.md".to_string(), stored(1234, Some("\"abc123\""))),
                ("Attachments/img.png".to_string(), stored(99, None)),
            ]),
        };
        persist_state(&path, &state).unwrap();
        assert_eq!(load_state(&path), state);
        // On-disk schema matches the spec.
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(json["version"], 1);
        assert_eq!(json["last_sync_at_unix"], 1_788_360_000);
        assert_eq!(json["files"]["Home.md"]["etag"], "\"abc123\"");
        // `None` etags are omitted rather than written as null.
        assert!(json["files"]["Attachments/img.png"].get("etag").is_none());
    }

    #[test]
    fn state_load_tolerates_null_and_omitted_etags() {
        let with_null: SyncState = serde_json::from_str(
            r#"{"version":1,"last_sync_at_unix":7,"files":{"a.md":{"size":1,"etag":null}}}"#,
        )
        .unwrap();
        let omitted: SyncState = serde_json::from_str(
            r#"{"version":1,"last_sync_at_unix":7,"files":{"a.md":{"size":1}}}"#,
        )
        .unwrap();
        assert_eq!(with_null, omitted);
        assert_eq!(omitted.files["a.md"].etag, None);
    }

    #[test]
    fn missing_corrupt_and_wrong_version_state_fall_back_to_first_run() {
        let dir = tempfile::tempdir().unwrap();
        // Missing file -> first run.
        let missing = load_state(&dir.path().join(".hatchdoor").join("webdav-sync.json"));
        assert_eq!(missing.version, 1);
        assert!(missing.files.is_empty());
        assert!(missing.last_sync_at_unix > 0);
        // Corrupt content -> first run.
        let path = dir.path().join("webdav-sync.json");
        std::fs::write(&path, "not json").unwrap();
        assert!(load_state(&path).files.is_empty());
        // Wrong version -> first run.
        std::fs::write(&path, r#"{"version":99,"last_sync_at_unix":1,"files":{}}"#).unwrap();
        assert!(load_state(&path).files.is_empty());
    }

    #[test]
    fn error_text_helpers_detect_404_and_existing_mkcol() {
        let not_found =
            WebDavError("webdav PROPFIND x: HTTP status client error (404 Not Found)".to_string());
        let server_error = WebDavError(
            "webdav PROPFIND x: HTTP status client error (500 Internal Server Error)".to_string(),
        );
        assert!(is_http_404(&not_found));
        assert!(!is_http_404(&server_error));
        let mkcol_405 = WebDavError(
            "webdav MKCOL x: HTTP status client error (405 Method Not Allowed)".to_string(),
        );
        let mkcol_301 = WebDavError("webdav MKCOL x: (301)".to_string());
        let mkcol_409 =
            WebDavError("webdav MKCOL x: HTTP status client error (409 Conflict)".to_string());
        assert!(is_mkcol_already_exists(&mkcol_405));
        assert!(is_mkcol_already_exists(&mkcol_301));
        assert!(!is_mkcol_already_exists(&mkcol_409));
    }

    #[test]
    fn ancestor_dirs_skips_the_file_name_and_orders_shallowest_first() {
        assert_eq!(ancestor_dirs("x.md"), Vec::<String>::new());
        assert_eq!(ancestor_dirs("a/x.md"), vec!["a"]);
        assert_eq!(ancestor_dirs("a/b/c.md"), vec!["a", "a/b"]);
    }

    #[test]
    fn join_rel_joins_under_the_root() {
        assert_eq!(join_rel("", "x.md"), "x.md");
        assert_eq!(join_rel("a", "x.md"), "a/x.md");
        assert_eq!(join_rel("a/", "x.md"), "a/x.md");
    }
}
