//! The on-disk Insights store: synchronous std-fs code the daemon runs on a
//! blocking thread.
//!
//! Layout under the root: `<site_dir>/<YYYYMMDDHH>.{roll,req,det}` plus
//! `<site_dir>/dismissed.json`, where `<site_dir>` is
//! [`site_dir_name`]. Dictionary lines live in the hour's `.roll`, so there
//! is no separate dictionary file and pruning an hour ages its entries out.
//!
//! Appends are crash tolerant without fsync: `.roll` and `.req` have a torn
//! tail truncated to the last newline before the next append, and `.det` is
//! never truncated because only complete `.req` lines point into it, so a
//! torn member is unreferenced bytes.
//!
//! The 1h/24h request list scans `.req` files newest first. The result is
//! cached as one entry per site, tagged with the store generation (bumped on
//! every append, prune and clear) and the clear epoch (bumped only by
//! [`Store::clear_site`]). An entry is reused while its key and epoch match
//! and either it is younger than [`SCAN_REUSE_MS`], or the generation is
//! unchanged and none of its rows has aged out of the range. Frequent flushes
//! rescan at most every 30 s, and an idle site rescans once a row ages out.

use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use yerd_ipc::{InsightsRange, InsightsSort};

use crate::error::InsightsError;
use crate::pure::aggregator::{select_requests, FlushBatch, HourBatch, LIST_LIMIT_MAX, RECENT_MS};
use crate::pure::codec::{
    decode_detail, decode_req_line, decode_roll_line, encode_detail, encode_req_line,
    encode_roll_delta, encode_roll_dict, DetailLoc, RollLine,
};
use crate::pure::dict::DictEntry;
use crate::pure::model::{key_hex, parse_key_hex, FinalRequest, RequestDetail};
use crate::pure::retention::{
    file_name, hour_of, parse_file_name, prune_plan, site_dir_name, site_from_dir_name, FileKind,
    GLOBAL_CAP_BYTES,
};
use crate::pure::rollup::MinuteRollup;

/// How long a cached request scan is reused after the store changed.
pub const SCAN_REUSE_MS: u64 = 30_000;

/// Hour files read per site on load: the current hour and the 24 before it.
const LOAD_HOURS: u64 = 25;

/// Largest `.det` member read back; anything bigger is a corrupt locator.
const DETAIL_READ_MAX: u64 = 64 * 1024 * 1024;

/// Bytes read per step when looking for a torn tail's last newline.
const TAIL_CHUNK: u64 = 64 * 1024;

const DISMISSED_FILE: &str = "dismissed.json";
const DISMISSED_TMP: &str = "dismissed.json.tmp";

/// One site's data read back by [`Store::load_all`].
#[derive(Debug, Default)]
pub struct LoadedSite {
    /// Site name.
    pub site: String,
    /// Rollup deltas from the retained `.roll` files (the aggregator merges
    /// them by minute).
    pub rollups: Vec<MinuteRollup>,
    /// `(hour, key, entry)` dictionary lines from the same files.
    pub dict_entries: Vec<(u64, u64, DictEntry)>,
    /// Requests from the last [`RECENT_MS`], already flushed.
    pub recent: Vec<FinalRequest>,
    /// Dismissed issue keys.
    pub dismissed: BTreeSet<u64>,
    /// Set when the site could not be read; the site is then returned empty.
    pub error: Option<InsightsError>,
}

type ScanKey = (InsightsRange, Option<String>, InsightsSort, u32);

#[derive(Debug)]
struct CachedScan {
    epoch: u64,
    generation: u64,
    scanned_at_ms: u64,
    key: ScanKey,
    results: Vec<FinalRequest>,
}

/// The on-disk store. See the module docs.
#[derive(Debug)]
pub struct Store {
    root: PathBuf,
    global_cap: u64,
    generations: Mutex<HashMap<String, u64>>,
    cache: Mutex<HashMap<String, CachedScan>>,
    clear_epochs: Mutex<HashMap<String, u64>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn io_at(path: &Path) -> impl FnOnce(io::Error) -> InsightsError + '_ {
    move |source| InsightsError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn len_u64(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// Read a whole file, `None` when it does not exist.
fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, InsightsError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_at(path)(e)),
    }
}

/// The newline-terminated UTF-8 lines of `bytes`. A final unterminated
/// fragment is a torn append that the next append truncates, so it is never
/// returned; other invalid lines are left to the decoder.
fn lines(bytes: &[u8]) -> impl Iterator<Item = &str> {
    bytes
        .split_inclusive(|b| *b == b'\n')
        .filter_map(|l| l.strip_suffix(b"\n"))
        .filter(|l| !l.is_empty())
        .filter_map(|l| std::str::from_utf8(l).ok())
}

/// Truncate a non-empty file that does not end in `\n` to just after its last
/// `\n`, or to empty. A missing file is left alone.
fn ensure_newline_terminated(path: &Path) -> Result<(), InsightsError> {
    let mut f = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(io_at(path)(e)),
    };
    let len = f.metadata().map_err(io_at(path))?.len();
    let mut end = len;
    while end > 0 {
        let start = end.saturating_sub(TAIL_CHUNK);
        let mut chunk = vec![0u8; usize::try_from(end - start).unwrap_or(0)];
        f.seek(SeekFrom::Start(start)).map_err(io_at(path))?;
        f.read_exact(&mut chunk).map_err(io_at(path))?;
        if end == len && chunk.last() == Some(&b'\n') {
            return Ok(());
        }
        if let Some(pos) = chunk.iter().rposition(|b| *b == b'\n') {
            return f.set_len(start + len_u64(pos) + 1).map_err(io_at(path));
        }
        end = start;
    }
    f.set_len(0).map_err(io_at(path))
}

fn append_bytes(path: &Path, bytes: &[u8]) -> Result<(), InsightsError> {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(io_at(path))?;
    f.write_all(bytes).map_err(io_at(path))
}

fn append_lines(path: &Path, lines: &[String]) -> Result<u64, InsightsError> {
    if lines.is_empty() {
        return Ok(0);
    }
    ensure_newline_terminated(path)?;
    let mut buf = String::with_capacity(lines.iter().map(|l| l.len() + 1).sum());
    for l in lines {
        buf.push_str(l);
        buf.push('\n');
    }
    append_bytes(path, buf.as_bytes())?;
    Ok(len_u64(buf.len()))
}

fn append_hour(dir: &Path, hour: u64, batch: &HourBatch) -> Result<u64, InsightsError> {
    let mut members: Vec<u8> = Vec::new();
    let mut locs: Vec<Option<DetailLoc>> = Vec::with_capacity(batch.requests.len());
    for (_, _, detail) in &batch.requests {
        match detail {
            Some(d) => {
                let bytes = encode_detail(d)?;
                locs.push(Some(DetailLoc {
                    offset: len_u64(members.len()),
                    len: len_u64(bytes.len()),
                }));
                members.extend_from_slice(&bytes);
            }
            None => locs.push(None),
        }
    }
    let mut roll = Vec::with_capacity(batch.dict_entries.len() + batch.rollup_deltas.len());
    for (key, entry) in &batch.dict_entries {
        roll.push(encode_roll_dict(*key, entry)?);
    }
    for delta in &batch.rollup_deltas {
        roll.push(encode_roll_delta(delta)?);
    }
    let mut written = 0;
    let mut base = 0;
    if !members.is_empty() {
        let path = dir.join(file_name(hour, FileKind::Det));
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(io_at(&path))?;
        base = f.metadata().map_err(io_at(&path))?.len();
        f.write_all(&members).map_err(io_at(&path))?;
        written += len_u64(members.len());
    }
    let mut req = Vec::with_capacity(batch.requests.len());
    for ((_, r, _), loc) in batch.requests.iter().zip(locs) {
        let loc = loc.map(|l| DetailLoc {
            offset: l.offset + base,
            len: l.len,
        });
        req.push(encode_req_line(r, loc)?);
    }
    written += append_lines(&dir.join(file_name(hour, FileKind::Roll)), &roll)?;
    written += append_lines(&dir.join(file_name(hour, FileKind::Req)), &req)?;
    Ok(written)
}

fn read_dismissed(dir: &Path) -> Result<BTreeSet<u64>, InsightsError> {
    let Some(bytes) = read_optional(&dir.join(DISMISSED_FILE))? else {
        return Ok(BTreeSet::new());
    };
    let keys: Vec<String> = serde_json::from_slice(&bytes).unwrap_or_default();
    Ok(keys.iter().filter_map(|k| parse_key_hex(k)).collect())
}

fn load_site_dir(dir: &Path, site: &str, now: u64) -> Result<LoadedSite, InsightsError> {
    let current = hour_of(now);
    let oldest = current.saturating_sub(LOAD_HOURS - 1);
    let mut roll_hours: Vec<u64> = Vec::new();
    for entry in fs::read_dir(dir).map_err(io_at(dir))? {
        let entry = entry.map_err(io_at(dir))?;
        if let Some((hour, FileKind::Roll)) = entry.file_name().to_str().and_then(parse_file_name) {
            if (oldest..=current).contains(&hour) {
                roll_hours.push(hour);
            }
        }
    }
    roll_hours.sort_unstable();
    let mut loaded = LoadedSite {
        site: site.to_owned(),
        ..LoadedSite::default()
    };
    for hour in roll_hours {
        let Some(bytes) = read_optional(&dir.join(file_name(hour, FileKind::Roll)))? else {
            continue;
        };
        for line in lines(&bytes) {
            match decode_roll_line(line) {
                Ok(RollLine::Delta(d)) => loaded.rollups.push(d),
                Ok(RollLine::Dict(key, entry)) => loaded.dict_entries.push((hour, key, entry)),
                Err(_) => {}
            }
        }
    }
    let since = now.saturating_sub(RECENT_MS);
    for hour in [current.saturating_sub(1), current] {
        let Some(bytes) = read_optional(&dir.join(file_name(hour, FileKind::Req)))? else {
            continue;
        };
        loaded.recent.extend(
            lines(&bytes)
                .filter_map(|l| decode_req_line(l).ok())
                .map(|(r, _)| r)
                .filter(|r| r.ts_ms >= since && r.ts_ms <= now),
        );
    }
    loaded.dismissed = read_dismissed(dir)?;
    Ok(loaded)
}

impl Store {
    /// A store rooted at `root` (created lazily on first write) with the
    /// default [`GLOBAL_CAP_BYTES`].
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self::with_cap(root, GLOBAL_CAP_BYTES)
    }

    /// A store with an explicit global disk cap.
    #[must_use]
    pub fn with_cap(root: PathBuf, global_cap: u64) -> Self {
        Self {
            root,
            global_cap,
            generations: Mutex::new(HashMap::new()),
            cache: Mutex::new(HashMap::new()),
            clear_epochs: Mutex::new(HashMap::new()),
        }
    }

    /// The store root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The site's store generation: bumped by every append, prune and clear.
    #[must_use]
    pub fn generation(&self, site: &str) -> u64 {
        lock(&self.generations).get(site).copied().unwrap_or(0)
    }

    fn clear_epoch(&self, site: &str) -> u64 {
        lock(&self.clear_epochs).get(site).copied().unwrap_or(0)
    }

    fn bump(map: &Mutex<HashMap<String, u64>>, site: &str) {
        let mut m = lock(map);
        let g = m.entry(site.to_owned()).or_insert(0);
        *g = g.wrapping_add(1);
    }

    fn site_dir(&self, site: &str) -> PathBuf {
        self.root.join(site_dir_name(site))
    }

    /// Append one flush batch: per hour, the detail members to `.det`, the
    /// dictionary then delta lines to `.roll`, and the summary lines (with
    /// their `.det` locators) to `.req`. Returns the bytes written. The
    /// site's generation is bumped even when a write fails part way.
    ///
    /// # Errors
    /// [`InsightsError::Io`] or [`InsightsError::Encode`]; hours before the
    /// failing one are already written.
    pub fn append_batch(&self, batch: &FlushBatch) -> Result<u64, InsightsError> {
        let dir = self.site_dir(&batch.site);
        let result = fs::create_dir_all(&dir)
            .map_err(io_at(&dir))
            .and_then(|()| {
                batch.per_hour.iter().try_fold(0u64, |acc, (hour, hb)| {
                    append_hour(&dir, *hour, hb).map(|n| acc.saturating_add(n))
                })
            });
        Self::bump(&self.generations, &batch.site);
        result
    }

    /// Read every site back: up to [`LOAD_HOURS`] hours of `.roll` per site
    /// (torn lines skipped), the current and previous hour's `.req` for
    /// requests newer than [`RECENT_MS`], and `dismissed.json`. A missing root
    /// gives nothing; a site that cannot be read comes back empty with
    /// [`LoadedSite::error`] set.
    ///
    /// # Errors
    /// [`InsightsError::Io`] when the root exists but cannot be listed.
    pub fn load_all(&self, now: u64) -> Result<Vec<LoadedSite>, InsightsError> {
        let mut out = Vec::new();
        for (site, dir) in self.site_dirs()? {
            out.push(
                load_site_dir(&dir, &site, now).unwrap_or_else(|e| LoadedSite {
                    site,
                    error: Some(e),
                    ..LoadedSite::default()
                }),
            );
        }
        out.sort_by(|a, b| a.site.cmp(&b.site));
        Ok(out)
    }

    fn site_dirs(&self) -> Result<Vec<(String, PathBuf)>, InsightsError> {
        let entries = match fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_at(&self.root)(e)),
        };
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry.map_err(io_at(&self.root))?;
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            if let Some(site) = entry.file_name().to_str().and_then(site_from_dir_name) {
                out.push((site, entry.path()));
            }
        }
        Ok(out)
    }

    /// Delete hour files past retention, then the oldest hours across every
    /// site while the total exceeds the global cap. Bumps each affected
    /// site's generation and returns the removed `(site, hour)` pairs. An
    /// hour whose files cannot all be removed is left for the next prune.
    ///
    /// # Errors
    /// [`InsightsError::Io`] when the root exists but cannot be listed.
    pub fn prune(&self, now: u64) -> Result<Vec<(String, u64)>, InsightsError> {
        let mut files: Vec<(String, String, u64)> = Vec::new();
        for (site, dir) in self.site_dirs()? {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if parse_file_name(&name).is_some() {
                    let len = entry.metadata().map_or(0, |m| m.len());
                    files.push((site.clone(), name, len));
                }
            }
        }
        let mut removed = Vec::new();
        for (site, hour) in prune_plan(&files, now, self.global_cap) {
            let dir = self.site_dir(&site);
            let kinds = [FileKind::Roll, FileKind::Req, FileKind::Det];
            let removed_files = kinds
                .into_iter()
                .filter(|kind| {
                    !matches!(
                        fs::remove_file(dir.join(file_name(hour, *kind))),
                        Err(e) if e.kind() != ErrorKind::NotFound
                    )
                })
                .count();
            let ok = removed_files == kinds.len();
            Self::bump(&self.generations, &site);
            if ok {
                removed.push((site, hour));
            }
        }
        Ok(removed)
    }

    /// Persisted requests of `site` within `range`, filtered by `query`,
    /// ordered by `sort` and limited (at most [`LIST_LIMIT_MAX`]). Served from
    /// the site's single cache entry when it is still valid.
    ///
    /// # Errors
    /// [`InsightsError::Io`] when an existing `.req` file cannot be read.
    pub fn scan_requests(
        &self,
        site: &str,
        range: InsightsRange,
        query: Option<&str>,
        sort: InsightsSort,
        limit: u32,
        now: u64,
    ) -> Result<Vec<FinalRequest>, InsightsError> {
        let key: ScanKey = (
            range,
            query.map(str::to_owned),
            sort,
            limit.min(LIST_LIMIT_MAX),
        );
        let generation = self.generation(site);
        let epoch = self.clear_epoch(site);
        if let Some(hit) = self.cached(site, &key, generation, epoch, now) {
            return Ok(hit);
        }
        let results = self.read_requests(site, &key, now)?;
        self.store_scan(site, key, generation, epoch, now, results.clone());
        Ok(results)
    }

    fn cached(
        &self,
        site: &str,
        key: &ScanKey,
        generation: u64,
        epoch: u64,
        now: u64,
    ) -> Option<Vec<FinalRequest>> {
        let cache = lock(&self.cache);
        let c = cache.get(site)?;
        let since = now.saturating_sub(key.0.millis());
        let fresh = now.saturating_sub(c.scanned_at_ms) < SCAN_REUSE_MS
            || (c.generation == generation && c.results.iter().all(|r| r.ts_ms >= since));
        (c.key == *key && c.epoch == epoch && fresh).then(|| c.results.clone())
    }

    fn store_scan(
        &self,
        site: &str,
        key: ScanKey,
        generation: u64,
        epoch: u64,
        now: u64,
        results: Vec<FinalRequest>,
    ) {
        let mut cache = lock(&self.cache);
        if self.clear_epoch(site) != epoch {
            return;
        }
        cache.insert(
            site.to_owned(),
            CachedScan {
                epoch,
                generation,
                scanned_at_ms: now,
                key,
                results,
            },
        );
    }

    fn read_requests(
        &self,
        site: &str,
        key: &ScanKey,
        now: u64,
    ) -> Result<Vec<FinalRequest>, InsightsError> {
        let (range, query, sort, limit) = key;
        let limit = *limit;
        if limit == 0 {
            return Ok(Vec::new());
        }
        let cap = usize::try_from(limit).unwrap_or(usize::MAX);
        let dir = self.site_dir(site);
        let since = now.saturating_sub(range.millis());
        let newest_only = matches!(sort, InsightsSort::Recent);
        let mut rows: Vec<FinalRequest> = Vec::new();
        let mut hour = hour_of(now);
        loop {
            if let Some(bytes) = read_optional(&dir.join(file_name(hour, FileKind::Req)))? {
                rows.extend(
                    lines(&bytes)
                        .filter_map(|l| decode_req_line(l).ok())
                        .map(|(r, _)| r)
                        .filter(|r| r.ts_ms >= since && r.ts_ms <= now)
                        .filter(|r| query.as_deref().map_or(true, |q| r.matches(q))),
                );
                if rows.len() > cap.saturating_mul(2) {
                    rows = select_requests(&rows, None, *sort, limit);
                }
            }
            if (newest_only && rows.len() >= cap) || hour == 0 || hour <= hour_of(since) {
                break;
            }
            hour -= 1;
        }
        Ok(select_requests(&rows, None, *sort, limit))
    }

    /// The persisted drill-in for a request: reads only the `.req` of
    /// `hour_of(ts_ms)` for its locator, then one positioned read of `.det`.
    /// `None` when the request is not there or the locator points at a
    /// different request. A request stored without detail gets an empty one.
    ///
    /// # Errors
    /// [`InsightsError::Io`] on a read failure, [`InsightsError::Decode`] on
    /// a corrupt member.
    pub fn find_detail(
        &self,
        site: &str,
        request_id: &str,
        ts_ms: u64,
    ) -> Result<Option<(FinalRequest, RequestDetail)>, InsightsError> {
        let dir = self.site_dir(site);
        let hour = hour_of(ts_ms);
        let Some(bytes) = read_optional(&dir.join(file_name(hour, FileKind::Req)))? else {
            return Ok(None);
        };
        let Some((req, loc)) = lines(&bytes)
            .filter_map(|l| decode_req_line(l).ok())
            .filter(|(r, _)| r.id == request_id)
            .last()
        else {
            return Ok(None);
        };
        let Some(loc) = loc else {
            return Ok(Some((req, RequestDetail::empty(request_id))));
        };
        if loc.len > DETAIL_READ_MAX {
            return Err(InsightsError::Decode(format!(
                "detail member of {} bytes",
                loc.len
            )));
        }
        let path = dir.join(file_name(hour, FileKind::Det));
        let mut f = File::open(&path).map_err(io_at(&path))?;
        let mut member = vec![0u8; usize::try_from(loc.len).unwrap_or(0)];
        f.seek(SeekFrom::Start(loc.offset)).map_err(io_at(&path))?;
        f.read_exact(&mut member).map_err(io_at(&path))?;
        let detail = decode_detail(&member)?;
        Ok((detail.id == request_id).then_some((req, detail)))
    }

    /// Delete everything stored for `site`, drop its cached scan, and bump
    /// its generation and clear epoch.
    ///
    /// # Errors
    /// [`InsightsError::Io`] when the directory exists but cannot be removed.
    pub fn clear_site(&self, site: &str) -> Result<(), InsightsError> {
        let dir = self.site_dir(site);
        let result = match fs::remove_dir_all(&dir) {
            Err(e) if e.kind() != ErrorKind::NotFound => Err(io_at(&dir)(e)),
            _ => Ok(()),
        };
        {
            let mut cache = lock(&self.cache);
            Self::bump(&self.clear_epochs, site);
            cache.remove(site);
        }
        Self::bump(&self.generations, site);
        result
    }

    /// Persist `site`'s dismissed issue keys (already filtered to keys still
    /// in its dictionary) with an atomic temp-then-rename. An empty set
    /// removes the file.
    ///
    /// # Errors
    /// [`InsightsError::Io`] or [`InsightsError::Encode`].
    pub fn save_dismissed(&self, site: &str, keys: &[u64]) -> Result<(), InsightsError> {
        let dir = self.site_dir(site);
        let path = dir.join(DISMISSED_FILE);
        if keys.is_empty() {
            return match fs::remove_file(&path) {
                Err(e) if e.kind() != ErrorKind::NotFound => Err(io_at(&path)(e)),
                _ => Ok(()),
            };
        }
        let hex: Vec<String> = keys.iter().map(|k| key_hex(*k)).collect();
        let json = serde_json::to_vec(&hex).map_err(|e| InsightsError::Encode(e.to_string()))?;
        fs::create_dir_all(&dir).map_err(io_at(&dir))?;
        let tmp = dir.join(DISMISSED_TMP);
        fs::write(&tmp, json).map_err(io_at(&tmp))?;
        fs::rename(&tmp, &path).map_err(io_at(&path))
    }

    /// The dismissed keys stored for `site`.
    ///
    /// # Errors
    /// [`InsightsError::Io`] when the file exists but cannot be read.
    pub fn load_dismissed(&self, site: &str) -> Result<BTreeSet<u64>, InsightsError> {
        read_dismissed(&self.site_dir(site))
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests;
