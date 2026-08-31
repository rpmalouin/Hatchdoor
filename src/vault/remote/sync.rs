//! Recursive remote↔mirror sync for a WebDAV Vault source.
//!
//! A WebDAV source keeps a **local mirror checkout** (the authoritative read
//! path per ADR-01; see `vault_registry::source_vault_path`) and this module
//! reconciles the remote WebDAV collection with that mirror during a sync turn:
//!
//! - **Pull:** files (and subcollections) present on the remote that are
//!   missing locally are downloaded (`GET`) / created (`create_dir_all`).
//! - **Push:** local files/dirs with no remote counterpart (new local content)
//!   are uploaded (`PUT`) / created remotely (`MKCOL`).
//!
//! It mirrors Hatchdoor's `ManagedGit` execution model: run as a background
//! `VaultWorkKind` turn under the Vault's mutation lock, with the mirror being
//! what the index/watcher/write layer already operate on. Phase D of the WebDAV
//! work packet; it never serves a per-request note read directly.

#![allow(dead_code)] // wired into dispatch by the follow-up adding the WebDAV work kind.

use std::path::Path;

use crate::vault::remote::{WebDavClient, WebDavError};

/// Outcome of one WebDAV sync turn.
#[derive(Debug, Default)]
pub struct WebDavSyncOutcome {
    pub pulled: usize,
    pub pushed: usize,
    pub created_dirs: usize,
    pub errors: usize,
}

/// Boxed-recursive entry points (recursive async fns must be boxed).
pub struct Sync;

impl Sync {
    pub async fn sync_once(
        client: &WebDavClient,
        mirror_root: &Path,
        root_rel: &str,
        outcome: &mut WebDavSyncOutcome,
    ) -> Result<(), WebDavError> {
        let entries = client.list(root_rel).await?;

        for entry in &entries {
            if entry.path.is_empty() {
                continue; // the collection itself
            }
            let rel = if root_rel.is_empty() {
                entry.path.clone()
            } else {
                format!("{}/{}", root_rel.trim_end_matches('/'), entry.path)
            };
            let local_path = mirror_root.join(&rel);

            if entry.is_dir {
                if !local_path.is_dir() {
                    std::fs::create_dir_all(&local_path).map_err(|e| {
                        WebDavError(format!(
                            "webdav sync: mkdir {}: {e}",
                            local_path.display()
                        ))
                    })?;
                }
                // Boxed recursion via Box::pin so the async fn stays sized.
                let fut = Self::sync_once(client, mirror_root, &rel, outcome);
                Box::pin(fut).await?;
                continue;
            }

            // A remote file missing locally -> pull.
            if !local_path.is_file() {
                match client.get(&rel).await {
                    Ok(bytes) => {
                        if let Some(parent) = local_path.parent() {
                            std::fs::create_dir_all(parent).map_err(|e| {
                                WebDavError(format!(
                                    "webdav sync: mkdir {}: {e}",
                                    parent.display()
                                ))
                            })?;
                        }
                        std::fs::write(&local_path, &bytes).map_err(|e| {
                            WebDavError(format!(
                                "webdav sync: write {}: {e}",
                                local_path.display()
                            ))
                        })?;
                        outcome.pulled += 1;
                    }
                    Err(_) => outcome.errors += 1,
                }
            }
        }

        Self::push_new_local(client, mirror_root, root_rel, outcome).await;
        Ok(())
    }

    /// Push local files/dirs under `dir_rel` that the remote does not list.
    async fn push_new_local(
        client: &WebDavClient,
        mirror_root: &Path,
        dir_rel: &str,
        outcome: &mut WebDavSyncOutcome,
    ) {
        // Which children does the remote already list here?
        let known_local: Vec<String> = match client.list(dir_rel).await {
            Ok(entries) => entries
                .iter()
                .filter(|e| !e.path.is_empty())
                .map(|e| e.path.clone())
                .collect(),
            Err(_) => return,
        };

        let dir = mirror_root.join(dir_rel);
        let Ok(read) = std::fs::read_dir(&dir) else {
            return;
        };
        for item in read.flatten() {
            let path = item.path();
            let name = match item.file_name().into_string() {
                Ok(name) => name,
                Err(_) => continue, // non-UTF8 filename: skip
            };
            if name.starts_with(".hatchdoor") {
                continue; // Hatchdoor's own marker files are never synced
            }
            let rel = if dir_rel.is_empty() {
                name.clone()
            } else {
                format!("{}/{}", dir_rel.trim_end_matches('/'), name)
            };
            if known_local.contains(&name) {
                continue; // remote already lists it
            }
            if path.is_dir() {
                if let Err(_) = client.mkdir(&rel).await {
                    outcome.errors += 1;
                } else {
                    outcome.created_dirs += 1;
                }
                let fut = Self::push_new_local(client, mirror_root, &rel, outcome);
                Box::pin(fut).await;
                continue;
            }
            match std::fs::read(&path) {
                Ok(bytes) => match client.put(&rel, &bytes, None).await {
                    Ok(()) => outcome.pushed += 1,
                    Err(_) => outcome.errors += 1,
                },
                Err(_) => {}
            }
        }
    }
}