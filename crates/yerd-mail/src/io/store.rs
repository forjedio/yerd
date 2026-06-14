//! On-disk store of captured emails.
//!
//! Layout under the store directory:
//! - `<id>.eml` — the verbatim captured message (one per email).
//! - `index.json` — an ordered (oldest-first) list of [`MailSummary`] metadata,
//!   so listing doesn't re-parse every `.eml`.
//!
//! All mutations go through a single [`tokio::sync::Mutex`] so concurrent SMTP
//! connections appending at once can't lose an index update (advisory file locks
//! / `fs2` are forbidden by the workspace dep-graph gate). Ids are a monotonic
//! counter, zero-padded, so they never collide and sort in receipt order.

use std::path::{Path, PathBuf};

use tokio::sync::Mutex;
use yerd_ipc::{MailDetail, MailSummary};

use crate::error::MailError;
use crate::pure::{mime, retention};

/// A persistent capture store. Cheap to clone the `Arc` the daemon holds.
pub struct Store {
    dir: PathBuf,
    cap: usize,
    inner: Mutex<Inner>,
}

struct Inner {
    /// Oldest-first metadata cache, mirrored to `index.json`.
    entries: Vec<MailSummary>,
    /// Next id to assign. Monotonic; never reused, even across `clear`.
    next_id: u64,
}

impl Store {
    /// Open (creating if absent) a store at `dir`, loading any existing index.
    /// Uses the default retention cap ([`retention::DEFAULT_CAP`]).
    ///
    /// # Errors
    /// Returns [`MailError::Io`] if the directory can't be created, or
    /// [`MailError::Index`] if a present `index.json` is corrupt.
    pub fn open(dir: PathBuf) -> Result<Self, MailError> {
        Self::open_with_cap(dir, retention::DEFAULT_CAP)
    }

    /// Open with an explicit retention cap (used by tests).
    ///
    /// # Errors
    /// As [`Self::open`].
    pub fn open_with_cap(dir: PathBuf, cap: usize) -> Result<Self, MailError> {
        std::fs::create_dir_all(&dir).map_err(|source| MailError::Io {
            path: dir.clone(),
            source,
        })?;
        let entries = load_index(&dir)?;
        // Seed the id counter from the max of the index AND any `.eml` on disk.
        // Scanning the directory too means an `.eml` that was written but never
        // recorded in `index.json` (e.g. a crash between the two writes) can never
        // have its id reused, honouring the "ids never reused" guarantee.
        let max_index = entries.iter().filter_map(|e| e.id.parse::<u64>().ok()).max();
        let max_disk = max_eml_id(&dir);
        let next_id = max_index.max(max_disk).map_or(0, |m| m + 1);
        Ok(Self {
            dir,
            cap,
            inner: Mutex::new(Inner { entries, next_id }),
        })
    }

    /// Capture a raw message: write its `.eml`, record its summary, and evict the
    /// oldest entries beyond the cap.
    ///
    /// # Errors
    /// [`MailError::Io`] / [`MailError::Index`] on a filesystem or serialise failure.
    pub async fn append(&self, raw: &[u8]) -> Result<(), MailError> {
        let mut inner = self.inner.lock().await;
        let id = format!("{:06}", inner.next_id);
        inner.next_id += 1;

        let eml = self.eml_path(&id);
        tokio::fs::write(&eml, raw)
            .await
            .map_err(|source| MailError::Io { path: eml, source })?;

        inner.entries.push(mime::summary(&id, raw));

        // Evict oldest beyond the cap.
        let evict = retention::evict_count(inner.entries.len(), self.cap);
        for old in inner.entries.drain(0..evict).collect::<Vec<_>>() {
            let p = self.eml_path(&old.id);
            let _ = tokio::fs::remove_file(&p).await;
        }

        self.write_index(&inner.entries).await
    }

    /// All captured emails (metadata only), newest first.
    pub async fn list(&self) -> Vec<MailSummary> {
        let inner = self.inner.lock().await;
        inner.entries.iter().rev().cloned().collect()
    }

    /// The number of captured emails currently stored.
    pub async fn count(&self) -> u32 {
        let inner = self.inner.lock().await;
        u32::try_from(inner.entries.len()).unwrap_or(u32::MAX)
    }

    /// Fetch one captured email's full decoded content by id, or `None` if no
    /// such id is stored.
    ///
    /// # Errors
    /// [`MailError::Io`] if the `.eml` exists in the index but can't be read.
    pub async fn get(&self, id: &str) -> Result<Option<MailDetail>, MailError> {
        let inner = self.inner.lock().await;
        if !inner.entries.iter().any(|e| e.id == id) {
            return Ok(None);
        }
        let eml = self.eml_path(id);
        let raw = tokio::fs::read(&eml)
            .await
            .map_err(|source| MailError::Io { path: eml, source })?;
        Ok(Some(mime::detail(id, &raw)))
    }

    /// Delete a specific set of captured emails by id (others are kept). Unknown
    /// ids are ignored. The id counter is not reset.
    ///
    /// # Errors
    /// [`MailError::Io`] / [`MailError::Index`] on a filesystem failure.
    pub async fn delete_many(&self, ids: &[String]) -> Result<(), MailError> {
        let mut inner = self.inner.lock().await;
        let remove: std::collections::HashSet<&str> = ids.iter().map(String::as_str).collect();
        let (drop, keep): (Vec<MailSummary>, Vec<MailSummary>) = std::mem::take(&mut inner.entries)
            .into_iter()
            .partition(|e| remove.contains(e.id.as_str()));
        inner.entries = keep;
        for e in drop {
            let p = self.eml_path(&e.id);
            let _ = tokio::fs::remove_file(&p).await;
        }
        self.write_index(&inner.entries).await
    }

    /// Delete every captured email. The id counter is **not** reset, so a later
    /// capture never reuses an id of a cleared message.
    ///
    /// # Errors
    /// [`MailError::Io`] / [`MailError::Index`] on a filesystem failure.
    pub async fn clear(&self) -> Result<(), MailError> {
        let mut inner = self.inner.lock().await;
        for e in inner.entries.drain(..).collect::<Vec<_>>() {
            let p = self.eml_path(&e.id);
            let _ = tokio::fs::remove_file(&p).await;
        }
        self.write_index(&inner.entries).await
    }

    fn eml_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.eml"))
    }

    async fn write_index(&self, entries: &[MailSummary]) -> Result<(), MailError> {
        let path = self.dir.join("index.json");
        let json = serde_json::to_vec_pretty(entries)?;
        tokio::fs::write(&path, json)
            .await
            .map_err(|source| MailError::Io { path, source })
    }
}

/// The largest numeric id among `<id>.eml` files on disk, or `None` if there are
/// none. Used (with the index) to seed the monotonic id counter so a previously
/// written `.eml` can never have its id reused after a restart.
fn max_eml_id(dir: &Path) -> Option<u64> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let name = name.to_str()?;
            name.strip_suffix(".eml")?.parse::<u64>().ok()
        })
        .max()
}

/// Load `index.json` if present; an absent file is an empty store.
fn load_index(dir: &Path) -> Result<Vec<MailSummary>, MailError> {
    let path = dir.join("index.json");
    match std::fs::read(&path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(source) => Err(MailError::Io { path, source }),
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn msg(subject: &str) -> Vec<u8> {
        format!("From: a@b.c\r\nTo: d@e.f\r\nSubject: {subject}\r\n\r\nbody\r\n").into_bytes()
    }

    #[tokio::test]
    async fn append_list_get_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().to_path_buf()).unwrap();
        store.append(&msg("First")).await.unwrap();
        store.append(&msg("Second")).await.unwrap();

        let list = store.list().await;
        assert_eq!(list.len(), 2);
        // Newest first.
        assert_eq!(list[0].subject, "Second");
        assert_eq!(list[1].subject, "First");
        assert_eq!(store.count().await, 2);

        let detail = store.get(&list[0].id).await.unwrap().unwrap();
        assert_eq!(detail.subject, "Second");
        assert!(store.get("999999").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn clear_empties_but_keeps_id_monotonic() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().to_path_buf()).unwrap();
        store.append(&msg("A")).await.unwrap();
        store.clear().await.unwrap();
        assert_eq!(store.count().await, 0);
        store.append(&msg("B")).await.unwrap();
        // Id did not reset to 000000.
        assert_eq!(store.list().await[0].id, "000001");
    }

    #[tokio::test]
    async fn delete_many_removes_only_the_given_ids() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().to_path_buf()).unwrap();
        for s in ["a", "b", "c"] {
            store.append(&msg(s)).await.unwrap();
        }
        // Delete "a" (000000) and "c" (000002); keep "b" (000001).
        store
            .delete_many(&["000000".to_string(), "000002".to_string()])
            .await
            .unwrap();
        let list = store.list().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].subject, "b");
        assert!(!dir.path().join("000000.eml").exists());
        assert!(dir.path().join("000001.eml").exists());
        assert!(!dir.path().join("000002.eml").exists());
        // Unknown ids are ignored.
        store.delete_many(&["999999".to_string()]).await.unwrap();
        assert_eq!(store.count().await, 1);
    }

    #[tokio::test]
    async fn retention_cap_evicts_oldest() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_with_cap(dir.path().to_path_buf(), 2).unwrap();
        for s in ["one", "two", "three"] {
            store.append(&msg(s)).await.unwrap();
        }
        let list = store.list().await;
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].subject, "three");
        assert_eq!(list[1].subject, "two");
        // The evicted .eml is gone from disk.
        assert!(!dir.path().join("000000.eml").exists());
    }

    #[tokio::test]
    async fn reopen_loads_existing_index() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = Store::open(dir.path().to_path_buf()).unwrap();
            store.append(&msg("Persisted")).await.unwrap();
        }
        let store = Store::open(dir.path().to_path_buf()).unwrap();
        let list = store.list().await;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].subject, "Persisted");
        // Next id continues after the loaded max.
        store.append(&msg("Next")).await.unwrap();
        assert_eq!(store.list().await[0].id, "000001");
    }

    #[tokio::test]
    async fn next_id_skips_orphaned_eml_not_in_index() {
        // Simulate a crash between writing an `.eml` and updating index.json: an
        // orphan `000007.eml` exists with no index entry. A reopened store must
        // NOT reuse id 7 (or any id ≤ 7).
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("000007.eml"), msg("orphan")).unwrap();
        let store = Store::open(dir.path().to_path_buf()).unwrap();
        store.append(&msg("fresh")).await.unwrap();
        assert_eq!(store.list().await[0].id, "000008");
    }

    #[tokio::test]
    async fn concurrent_appends_do_not_lose_updates() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(Store::open(dir.path().to_path_buf()).unwrap());
        let mut handles = Vec::new();
        for i in 0..20 {
            let s = store.clone();
            handles.push(tokio::spawn(async move {
                s.append(&msg(&format!("m{i}"))).await.unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(store.count().await, 20);
    }
}
