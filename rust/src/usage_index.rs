//! Parse-once index of the records inside local transcript files.
//!
//! Every surface that shows local spend (Charts, Estimated API value, the
//! activity heatmap, Compare) aggregates the same Codex and Claude JSONL
//! transcripts, and each one used to re-read and re-parse them from scratch. On
//! a working machine that is gigabytes per open.
//!
//! This sits between "a path" and "the records in it". A file is parsed once;
//! after that a scan reads its records back from a packed index. Claude
//! transcripts are append-only, so a file that grew is resumed from the byte
//! offset the last parse stopped at rather than re-read from the top.
//!
//! What is *not* stored here is any aggregation: no day buckets, no windows, no
//! summaries. Reset windows land on arbitrary instants, so anything coarser
//! than a record would quietly round the numbers this app exists to report.
//! Callers keep their own folds, unchanged, and only the reading gets cheaper.

use crate::core::CodexUsageRecord;
use crate::cost_scanner::ClaudeUsageRecord;
use chrono::{DateTime, TimeZone, Utc};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock, RwLockReadGuard};

/// Format tag. The trailing byte is the layout version: bump it and every
/// existing index is discarded instead of being read with the wrong shape.
const MAGIC: &[u8; 8] = b"CBUIDX\x00\x02";
/// Sentinel for an absent interned string.
const NONE_ID: u32 = u32::MAX;
/// Sentinel for a record with no timestamp.
const NO_TIMESTAMP: i64 = i64::MIN;
/// Bytes of a file's head that are hashed to notice a rewrite.
///
/// Length and mtime alone cannot tell "appended to" from "replaced": a rewrite
/// that lands on the same length would be resumed from a stale offset, and the
/// records after it would be read out of the middle of a different file.
const HEAD_SAMPLE_BYTES: usize = 4096;
/// Entries not used by a scan for this long are dropped when the index is
/// written. Transcripts get deleted and projects get archived; without this the
/// index would keep their records for the life of the install.
const ENTRY_TTL_MS: i64 = 14 * 24 * 60 * 60 * 1000;

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or(0)
}

/// What the index needs to know about a file to decide whether its records are
/// still good.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileFacts {
    pub len: u64,
    pub mtime_ms: i64,
    pub head_hash: u64,
}

/// Read a file's identifying facts, or `None` when it cannot be read at all.
pub fn file_facts(path: &Path) -> Option<FileFacts> {
    let metadata = fs::metadata(path).ok()?;
    let mtime_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_millis() as i64)
        .unwrap_or(0);
    let mut head = vec![0_u8; HEAD_SAMPLE_BYTES];
    let read = fs::File::open(path)
        .and_then(|mut file| file.read(&mut head))
        .unwrap_or(0);
    head.truncate(read);
    Some(FileFacts {
        len: metadata.len(),
        mtime_ms,
        head_hash: fnv1a64(&head),
    })
}

/// Interned strings shared by every record in one index.
///
/// Model names, projects, plans, and Codex day keys repeat across hundreds of
/// thousands of records; storing each one inline would dwarf the numbers.
#[derive(Default)]
pub struct StringTable {
    values: Vec<String>,
    ids: HashMap<String, u32>,
}

impl StringTable {
    fn intern(&mut self, value: &str) -> u32 {
        if let Some(id) = self.ids.get(value) {
            return *id;
        }
        let id = self.values.len() as u32;
        self.values.push(value.to_string());
        self.ids.insert(value.to_string(), id);
        id
    }

    fn intern_opt(&mut self, value: Option<&str>) -> u32 {
        match value {
            Some(value) => self.intern(value),
            None => NONE_ID,
        }
    }

    fn get(&self, id: u32) -> Option<&str> {
        self.values.get(id as usize).map(String::as_str)
    }

    /// A required string. A table that does not carry the id is a corrupt
    /// index, and the caller drops the whole entry rather than inventing one.
    fn required(&self, id: u32) -> Option<String> {
        self.get(id).map(str::to_string)
    }

    fn optional(&self, id: u32) -> Option<String> {
        if id == NONE_ID {
            return None;
        }
        self.get(id).map(str::to_string)
    }
}

/// Bounds-checked reader over an index file. Every read returns `Option`, so a
/// truncated or corrupt file drops the index instead of panicking on a slice.
pub struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(count)?;
        let slice = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(slice)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn f64(&mut self) -> Option<f64> {
        Some(f64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn string(&mut self) -> Option<String> {
        let len = self.u32()? as usize;
        String::from_utf8(self.take(len)?.to_vec()).ok()
    }

    fn optional_string(&mut self) -> Option<Option<String>> {
        let len = self.u32()?;
        if len == NONE_ID {
            return Some(None);
        }
        let bytes = self.take(len as usize)?;
        Some(String::from_utf8(bytes.to_vec()).ok())
    }

    fn timestamp(&mut self) -> Option<Option<DateTime<Utc>>> {
        let millis = self.i64()?;
        if millis == NO_TIMESTAMP {
            return Some(None);
        }
        Some(Utc.timestamp_millis_opt(millis).single())
    }
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_i64(out: &mut Vec<u8>, value: i64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_f64(out: &mut Vec<u8>, value: f64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_string(out: &mut Vec<u8>, value: &str) {
    push_u32(out, value.len() as u32);
    out.extend_from_slice(value.as_bytes());
}

fn push_optional_string(out: &mut Vec<u8>, value: Option<&str>) {
    match value {
        Some(value) => push_string(out, value),
        None => push_u32(out, NONE_ID),
    }
}

fn push_timestamp(out: &mut Vec<u8>, value: Option<DateTime<Utc>>) {
    push_i64(
        out,
        value
            .map(|time| time.timestamp_millis())
            .unwrap_or(NO_TIMESTAMP),
    );
}

/// A record shape the index can store.
pub trait IndexedRecord: Sized + Clone + Send + Sync {
    /// Whether an appended file can be resumed from its last offset, or has to
    /// be re-read whole.
    const RESUMABLE: bool;

    fn encode(&self, strings: &mut StringTable, out: &mut Vec<u8>);
    fn decode(cursor: &mut Cursor<'_>, strings: &StringTable) -> Option<Self>;
}

impl IndexedRecord for ClaudeUsageRecord {
    // Claude writes one self-contained event per line and only ever appends, so
    // records already read stay valid and only the new tail needs parsing.
    const RESUMABLE: bool = true;

    fn encode(&self, strings: &mut StringTable, out: &mut Vec<u8>) {
        let model = strings.intern(&self.model);
        let project = strings.intern_opt(self.project.as_deref());
        push_u32(out, model);
        push_u32(out, project);
        push_timestamp(out, self.timestamp);
        // The de-duplication key is a per-record message id, so interning it
        // would grow the table by one entry per record. It stays inline.
        push_optional_string(out, self.dedup_key.as_deref());
        push_u64(out, self.input);
        push_u64(out, self.output);
        push_u64(out, self.cache_create);
        push_u64(out, self.cache_read);
        push_f64(out, self.cost);
    }

    fn decode(cursor: &mut Cursor<'_>, strings: &StringTable) -> Option<Self> {
        let model = strings.required(cursor.u32()?)?;
        let project = strings.optional(cursor.u32()?);
        Some(Self {
            model,
            project,
            timestamp: cursor.timestamp()?,
            dedup_key: cursor.optional_string()?,
            input: cursor.u64()?,
            output: cursor.u64()?,
            cache_create: cursor.u64()?,
            cache_read: cursor.u64()?,
            cost: cursor.f64()?,
        })
    }
}

impl IndexedRecord for CodexUsageRecord {
    // A Codex rollout's token counts are cumulative, and the parser carries
    // replay gates and a running baseline across lines. Resuming mid-file would
    // have to reproduce that state, so a changed rollout is re-read whole. Only
    // the session being written right now changes, which is one small file.
    const RESUMABLE: bool = false;

    fn encode(&self, strings: &mut StringTable, out: &mut Vec<u8>) {
        let day_key = strings.intern(&self.day_key);
        let model = strings.intern(&self.model);
        let effort = strings.intern_opt(self.effort.as_deref());
        let project = strings.intern_opt(self.project.as_deref());
        let plan = strings.intern_opt(self.plan.as_deref());
        push_u32(out, day_key);
        push_u32(out, model);
        push_u32(out, effort);
        push_u32(out, project);
        push_u32(out, plan);
        push_timestamp(out, self.timestamp);
        push_u64(out, self.input);
        push_u64(out, self.cached);
        push_u64(out, self.output);
    }

    fn decode(cursor: &mut Cursor<'_>, strings: &StringTable) -> Option<Self> {
        let day_key = strings.required(cursor.u32()?)?;
        let model = strings.required(cursor.u32()?)?;
        let effort = strings.optional(cursor.u32()?);
        let project = strings.optional(cursor.u32()?);
        let plan = strings.optional(cursor.u32()?);
        Some(Self {
            day_key,
            model,
            effort,
            project,
            plan,
            timestamp: cursor.timestamp()?,
            input: cursor.u64()?,
            cached: cursor.u64()?,
            output: cursor.u64()?,
        })
    }
}

struct Entry<R> {
    len: u64,
    mtime_ms: i64,
    head_hash: u64,
    /// Byte offset the last parse stopped at. For a resumable record type this
    /// is where the next one starts.
    parsed_bytes: u64,
    /// Oldest instant this entry can answer for. A transcript appended to for a
    /// year holds records no window in the app can ask about, and indexing them
    /// costs real time on the pass that builds it, so the builder may stop at a
    /// horizon. A scan that reaches further back than this cannot use the entry.
    covers_from_ms: i64,
    used_at_ms: i64,
    records: Vec<R>,
}

/// What an index has for one file.
pub enum Lookup<'a, R> {
    /// The file is unchanged; these are all of its records.
    Hit(&'a [R]),
    /// The file grew. Parse from `from` and add the result to `prior`.
    Append {
        from: u64,
        prior: &'a [R],
        /// Carried through so the rewritten entry keeps claiming the coverage
        /// its older records actually give it.
        covers_from_ms: i64,
    },
    /// Nothing usable. Parse the whole file.
    Miss,
}

/// One file's records, ready to be written into the index.
pub struct NewEntry<R> {
    pub path: PathBuf,
    pub facts: FileFacts,
    /// Byte offset the parse stopped at.
    pub parsed_bytes: u64,
    /// Oldest instant these records cover. `i64::MIN` when the whole file was
    /// kept, whatever it holds.
    pub covers_from_ms: i64,
    pub records: Vec<R>,
}

/// One provider's parsed records, keyed by transcript path.
pub struct UsageIndex<R> {
    /// Fingerprint of the inputs that produced the stored numbers. An index
    /// built under different prices is discarded rather than trusted.
    fingerprint: u64,
    entries: HashMap<PathBuf, Entry<R>>,
}

impl<R: IndexedRecord> UsageIndex<R> {
    fn empty(fingerprint: u64) -> Self {
        Self {
            fingerprint,
            entries: HashMap::new(),
        }
    }

    /// Records already held for `path`, given what the file looks like now and
    /// how far back the scan needs to see.
    pub fn lookup(&self, path: &Path, facts: &FileFacts, needs_from_ms: i64) -> Lookup<'_, R> {
        let Some(entry) = self.entries.get(path) else {
            return Lookup::Miss;
        };
        // A file whose head changed is a different file wearing the same name.
        if entry.head_hash != facts.head_hash {
            return Lookup::Miss;
        }
        // The entry was built for a shallower window than this scan wants.
        if entry.covers_from_ms > needs_from_ms {
            return Lookup::Miss;
        }
        if entry.len == facts.len && entry.mtime_ms == facts.mtime_ms {
            return Lookup::Hit(&entry.records);
        }
        if R::RESUMABLE && facts.len > entry.len && entry.parsed_bytes <= facts.len {
            return Lookup::Append {
                from: entry.parsed_bytes,
                prior: &entry.records,
                covers_from_ms: entry.covers_from_ms,
            };
        }
        Lookup::Miss
    }

    /// Record the full set of records for one file as it looks now.
    pub fn insert(&mut self, entry: NewEntry<R>) {
        self.entries.insert(
            entry.path,
            Entry {
                len: entry.facts.len,
                mtime_ms: entry.facts.mtime_ms,
                head_hash: entry.facts.head_hash,
                parsed_bytes: entry.parsed_bytes,
                covers_from_ms: entry.covers_from_ms,
                used_at_ms: now_ms(),
                records: entry.records,
            },
        );
    }

    /// Mark an untouched entry as still in use, so it survives the TTL sweep.
    pub fn touch(&mut self, path: &Path) {
        if let Some(entry) = self.entries.get_mut(path) {
            entry.used_at_ms = now_ms();
        }
    }

    fn encode(&self) -> Vec<u8> {
        // Records are encoded first so the string table is complete before it
        // is written; the reader needs the table before the records.
        let mut strings = StringTable::default();
        let mut body = Vec::new();
        push_u32(&mut body, self.entries.len() as u32);
        for (path, entry) in &self.entries {
            push_string(&mut body, &path.to_string_lossy());
            push_u64(&mut body, entry.len);
            push_i64(&mut body, entry.mtime_ms);
            push_u64(&mut body, entry.head_hash);
            push_u64(&mut body, entry.parsed_bytes);
            push_i64(&mut body, entry.covers_from_ms);
            push_i64(&mut body, entry.used_at_ms);
            push_u32(&mut body, entry.records.len() as u32);
            for record in &entry.records {
                record.encode(&mut strings, &mut body);
            }
        }

        let mut out = Vec::with_capacity(body.len() + 1024);
        out.extend_from_slice(MAGIC);
        push_u64(&mut out, self.fingerprint);
        push_u32(&mut out, strings.values.len() as u32);
        for value in &strings.values {
            push_string(&mut out, value);
        }
        out.extend_from_slice(&body);
        out
    }

    fn decode(bytes: &[u8], fingerprint: u64) -> Option<Self> {
        let mut cursor = Cursor::new(bytes);
        if cursor.take(MAGIC.len())? != MAGIC {
            return None;
        }
        if cursor.u64()? != fingerprint {
            return None;
        }

        let mut strings = StringTable::default();
        let string_count = cursor.u32()?;
        for _ in 0..string_count {
            let value = cursor.string()?;
            strings.intern(&value);
        }

        let mut entries = HashMap::new();
        let entry_count = cursor.u32()?;
        let cutoff_ms = now_ms().saturating_sub(ENTRY_TTL_MS);
        for _ in 0..entry_count {
            let path = PathBuf::from(cursor.string()?);
            let len = cursor.u64()?;
            let mtime_ms = cursor.i64()?;
            let head_hash = cursor.u64()?;
            let parsed_bytes = cursor.u64()?;
            let covers_from_ms = cursor.i64()?;
            let used_at_ms = cursor.i64()?;
            let record_count = cursor.u32()?;
            let mut records = Vec::with_capacity(record_count as usize);
            for _ in 0..record_count {
                records.push(R::decode(&mut cursor, &strings)?);
            }
            // Drop stale entries while reading rather than in a separate sweep,
            // so a transcript that was deleted months ago costs one read and
            // then leaves for good.
            if used_at_ms < cutoff_ms {
                continue;
            }
            entries.insert(
                path,
                Entry {
                    len,
                    mtime_ms,
                    head_hash,
                    parsed_bytes,
                    covers_from_ms,
                    used_at_ms,
                    records,
                },
            );
        }
        Some(Self {
            fingerprint,
            entries,
        })
    }
}

/// A process-wide index kept in memory and mirrored to disk.
///
/// Scans take a read guard for their whole parse, so the two cards can scan at
/// the same time; the short write that follows is what updates changed files.
pub struct IndexStore<R: 'static> {
    file_name: &'static str,
    state: OnceLock<RwLock<UsageIndex<R>>>,
}

impl<R: IndexedRecord> IndexStore<R> {
    pub const fn new(file_name: &'static str) -> Self {
        Self {
            file_name,
            state: OnceLock::new(),
        }
    }

    /// Borrow the index for the length of a scan.
    pub fn read(&'static self) -> RwLockReadGuard<'static, UsageIndex<R>> {
        self.state()
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Apply one scan's newly parsed files and write the index back.
    ///
    /// `touched` are the paths the scan read from the index unchanged; they are
    /// marked so an idle-but-live transcript does not age out.
    pub fn commit(&'static self, updates: Vec<NewEntry<R>>, touched: &[PathBuf]) {
        if updates.is_empty() && touched.is_empty() {
            return;
        }
        let mut guard = self
            .state()
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for path in touched {
            guard.touch(path);
        }
        let changed = !updates.is_empty();
        for entry in updates {
            guard.insert(entry);
        }
        if !changed {
            return;
        }
        if let Some(path) = self.path() {
            let bytes = guard.encode();
            drop(guard);
            if let Some(parent) = path.parent()
                && let Err(error) = fs::create_dir_all(parent)
            {
                tracing::warn!("failed to create usage index directory: {error}");
                return;
            }
            if let Err(error) = crate::secure_file::atomic_write(&path, &bytes) {
                tracing::warn!("failed to persist usage index: {error}");
            }
        }
    }

    fn state(&'static self) -> &'static RwLock<UsageIndex<R>> {
        self.state.get_or_init(|| RwLock::new(self.load()))
    }

    fn load(&self) -> UsageIndex<R> {
        let fingerprint = pricing_fingerprint();
        let Some(path) = self.path() else {
            return UsageIndex::empty(fingerprint);
        };
        fs::read(&path)
            .ok()
            .and_then(|bytes| UsageIndex::decode(&bytes, fingerprint))
            .unwrap_or_else(|| UsageIndex::empty(fingerprint))
    }

    fn path(&self) -> Option<PathBuf> {
        crate::settings::Settings::settings_path().and_then(|path| {
            path.parent()
                .map(|parent| parent.join("usage-index").join(self.file_name))
        })
    }
}

/// Fingerprint of everything outside the transcripts that the stored numbers
/// depend on.
///
/// Claude records carry the dollar figure computed when they were parsed, so a
/// price change has to invalidate them. The catalog's *contents* are hashed
/// rather than its file time: it is refetched on a daily cadence and rewritten
/// even when no price moved, and an mtime would throw the index away every day
/// for nothing. The app version rides along because the built-in rate card
/// ships with the binary.
fn pricing_fingerprint() -> u64 {
    let catalog = crate::core::pricing_catalog_path()
        .and_then(|path| fs::read(path).ok())
        .map(|bytes| fnv1a64(&bytes))
        .unwrap_or(0);
    fnv1a64(env!("CARGO_PKG_VERSION").as_bytes()) ^ catalog.rotate_left(17)
}

pub(crate) static CLAUDE_INDEX: IndexStore<ClaudeUsageRecord> = IndexStore::new("claude.bin");
pub(crate) static CODEX_INDEX: IndexStore<CodexUsageRecord> = IndexStore::new("codex.bin");

#[cfg(test)]
mod tests {
    use super::*;

    /// A scan with no lower bound on how far back it looks.
    const ANY_TIME: i64 = i64::MIN;

    fn facts(len: u64, mtime_ms: i64) -> FileFacts {
        FileFacts {
            len,
            mtime_ms,
            head_hash: 42,
        }
    }

    fn claude_record(model: &str, cost: f64) -> ClaudeUsageRecord {
        ClaudeUsageRecord {
            model: model.to_string(),
            timestamp: Utc.timestamp_millis_opt(1_700_000_000_000).single(),
            dedup_key: Some("msg-1:req-1".to_string()),
            project: Some("ceiling".to_string()),
            input: 10,
            output: 20,
            cache_create: 30,
            cache_read: 40,
            cost,
        }
    }

    fn codex_record(day: &str) -> CodexUsageRecord {
        CodexUsageRecord {
            day_key: day.to_string(),
            timestamp: Utc.timestamp_millis_opt(1_700_000_000_000).single(),
            model: "gpt-5".to_string(),
            effort: Some("high".to_string()),
            project: None,
            plan: Some("plus".to_string()),
            input: 1,
            cached: 2,
            output: 3,
        }
    }

    fn entry<R>(path: &str, facts: FileFacts, records: Vec<R>) -> NewEntry<R> {
        NewEntry {
            path: PathBuf::from(path),
            facts,
            parsed_bytes: facts.len,
            covers_from_ms: ANY_TIME,
            records,
        }
    }

    #[test]
    fn claude_records_survive_a_round_trip() {
        let mut index = UsageIndex::<ClaudeUsageRecord>::empty(7);
        index.insert(entry(
            "/logs/a.jsonl",
            facts(100, 5),
            vec![
                claude_record("claude-opus-4-8", 1.5),
                claude_record("claude-haiku-4-5", 0.25),
            ],
        ));

        let decoded = UsageIndex::<ClaudeUsageRecord>::decode(&index.encode(), 7).expect("decode");

        match decoded.lookup(Path::new("/logs/a.jsonl"), &facts(100, 5), ANY_TIME) {
            Lookup::Hit(records) => {
                assert_eq!(records.len(), 2);
                assert_eq!(records[0].model, "claude-opus-4-8");
                assert_eq!(records[0].cost, 1.5);
                assert_eq!(records[0].dedup_key.as_deref(), Some("msg-1:req-1"));
                assert_eq!(records[0].project.as_deref(), Some("ceiling"));
                assert_eq!(records[1].cost, 0.25);
            }
            _ => panic!("expected a hit for an unchanged file"),
        }
    }

    #[test]
    fn codex_records_survive_a_round_trip() {
        let mut index = UsageIndex::<CodexUsageRecord>::empty(1);
        index.insert(entry(
            "/logs/rollout.jsonl",
            facts(10, 1),
            vec![codex_record("2026-08-17")],
        ));

        let decoded = UsageIndex::<CodexUsageRecord>::decode(&index.encode(), 1).expect("decode");

        match decoded.lookup(Path::new("/logs/rollout.jsonl"), &facts(10, 1), ANY_TIME) {
            Lookup::Hit(records) => {
                assert_eq!(records[0].day_key, "2026-08-17");
                assert_eq!(records[0].plan.as_deref(), Some("plus"));
                assert_eq!(records[0].project, None);
                assert_eq!(records[0].cached, 2);
            }
            _ => panic!("expected a hit for an unchanged file"),
        }
    }

    #[test]
    fn a_grown_claude_file_resumes_from_its_last_offset() {
        let mut index = UsageIndex::<ClaudeUsageRecord>::empty(1);
        index.insert(entry(
            "/logs/a.jsonl",
            facts(100, 5),
            vec![claude_record("claude-opus-4-8", 1.0)],
        ));

        match index.lookup(Path::new("/logs/a.jsonl"), &facts(250, 9), ANY_TIME) {
            Lookup::Append { from, prior, .. } => {
                assert_eq!(from, 100);
                assert_eq!(prior.len(), 1);
            }
            _ => panic!("a longer file should resume, not re-read"),
        }
    }

    #[test]
    fn a_grown_codex_rollout_is_read_whole() {
        let mut index = UsageIndex::<CodexUsageRecord>::empty(1);
        index.insert(entry(
            "/logs/rollout.jsonl",
            facts(100, 5),
            vec![codex_record("2026-08-17")],
        ));

        // Cumulative token counts cannot be resumed from an offset.
        assert!(matches!(
            index.lookup(Path::new("/logs/rollout.jsonl"), &facts(250, 9), ANY_TIME),
            Lookup::Miss
        ));
    }

    #[test]
    fn a_rewritten_file_is_not_resumed() {
        let mut index = UsageIndex::<ClaudeUsageRecord>::empty(1);
        index.insert(entry(
            "/logs/a.jsonl",
            facts(100, 5),
            vec![claude_record("claude-opus-4-8", 1.0)],
        ));

        let replaced = FileFacts {
            len: 250,
            mtime_ms: 9,
            head_hash: 43,
        };
        assert!(matches!(
            index.lookup(Path::new("/logs/a.jsonl"), &replaced, ANY_TIME),
            Lookup::Miss
        ));
    }

    #[test]
    fn a_shrunken_file_is_read_whole() {
        let mut index = UsageIndex::<ClaudeUsageRecord>::empty(1);
        index.insert(entry(
            "/logs/a.jsonl",
            facts(100, 5),
            vec![claude_record("claude-opus-4-8", 1.0)],
        ));

        assert!(matches!(
            index.lookup(Path::new("/logs/a.jsonl"), &facts(50, 9), ANY_TIME),
            Lookup::Miss
        ));
    }

    #[test]
    fn an_entry_that_does_not_reach_far_enough_back_is_a_miss() {
        // This entry only kept records from 2026 on. A scan that wants 2023 has
        // to read the file itself, or it would quietly report a shorter history
        // than the one that was asked for.
        let mut index = UsageIndex::<ClaudeUsageRecord>::empty(1);
        index.insert(NewEntry {
            covers_from_ms: 1_767_225_600_000,
            ..entry(
                "/logs/a.jsonl",
                facts(100, 5),
                vec![claude_record("claude-opus-4-8", 1.0)],
            )
        });

        assert!(matches!(
            index.lookup(
                Path::new("/logs/a.jsonl"),
                &facts(100, 5),
                1_700_000_000_000
            ),
            Lookup::Miss
        ));
        assert!(matches!(
            index.lookup(
                Path::new("/logs/a.jsonl"),
                &facts(100, 5),
                1_800_000_000_000
            ),
            Lookup::Hit(_)
        ));
    }

    #[test]
    fn an_index_built_under_other_prices_is_discarded() {
        let mut index = UsageIndex::<ClaudeUsageRecord>::empty(7);
        index.insert(entry(
            "/logs/a.jsonl",
            facts(100, 5),
            vec![claude_record("claude-opus-4-8", 1.0)],
        ));

        assert!(UsageIndex::<ClaudeUsageRecord>::decode(&index.encode(), 8).is_none());
    }

    #[test]
    fn entries_untouched_past_the_ttl_are_dropped_on_read() {
        let mut index = UsageIndex::<ClaudeUsageRecord>::empty(1);
        index.insert(entry(
            "/logs/live.jsonl",
            facts(10, 1),
            vec![claude_record("claude-opus-4-8", 1.0)],
        ));
        index.insert(entry(
            "/logs/gone.jsonl",
            facts(10, 1),
            vec![claude_record("claude-opus-4-8", 1.0)],
        ));
        if let Some(entry) = index.entries.get_mut(Path::new("/logs/gone.jsonl")) {
            entry.used_at_ms = now_ms() - ENTRY_TTL_MS - 1;
        }

        let decoded = UsageIndex::<ClaudeUsageRecord>::decode(&index.encode(), 1).expect("decode");

        assert!(matches!(
            decoded.lookup(Path::new("/logs/live.jsonl"), &facts(10, 1), ANY_TIME),
            Lookup::Hit(_)
        ));
        assert!(matches!(
            decoded.lookup(Path::new("/logs/gone.jsonl"), &facts(10, 1), ANY_TIME),
            Lookup::Miss
        ));
    }

    #[test]
    fn a_truncated_index_file_is_rejected_rather_than_panicking() {
        let mut index = UsageIndex::<ClaudeUsageRecord>::empty(1);
        index.insert(entry(
            "/logs/a.jsonl",
            facts(100, 5),
            vec![claude_record("claude-opus-4-8", 1.0)],
        ));
        let bytes = index.encode();

        for cut in [0, 4, MAGIC.len(), bytes.len() / 2, bytes.len() - 1] {
            assert!(UsageIndex::<ClaudeUsageRecord>::decode(&bytes[..cut], 1).is_none());
        }
    }
}
