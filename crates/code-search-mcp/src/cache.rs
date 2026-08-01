//! Disk cache format for code-search indexes.
//!
//! The cache is deliberately disposable: old versions, malformed metadata, or a
//! missing binary vector sidecar are treated as cache misses instead of being
//! repaired in place. Metadata stays in JSON so it remains easy to inspect while
//! dense vectors live in a row-major little-endian f32 file to avoid large, slow
//! JSON arrays. Every file record stores a range into that flat matrix, so loads
//! must validate both the metadata shape and the binary row count before the
//! index can trust any cached chunk id.

use std::fmt::Write as _;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Read as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::files::FileManifestEntry;
use crate::matrix::EmbeddingMatrix;
use crate::types::{Chunk, CodeSearchError, ContentFilter};

const CACHE_VERSION: u32 = 5;
const CHUNKER_VERSION: u32 = 2;
const F32_BYTES: usize = 4;

#[derive(Debug, Clone, PartialEq)]
pub struct CachedIndex {
    pub payload: CachedIndexPayloadV4,
    pub embeddings: EmbeddingMatrix,
}

/// JSON metadata for the current cache layout.
///
/// The version is bumped whenever the on-disk interpretation changes. There is
/// no migration path because rebuilding from the workspace is simpler and safer
/// than preserving stale semantic vectors.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CachedIndexPayloadV4 {
    pub cache_version: u32,
    pub chunker_version: u32,
    pub root: PathBuf,
    pub content: ContentFilter,
    pub model_id: String,
    pub embedding_format: EmbeddingFormat,
    pub embedding_dimensions: usize,
    pub embedding_rows: usize,
    pub embedding_sha256: String,
    pub files: Vec<CachedFileRecord>,
}

impl CachedIndexPayloadV4 {
    /// Creates metadata tied to the supplied flat embedding matrix.
    ///
    /// File records must already point at valid matrix row ranges; construction
    /// keeps the caller responsible for ordering so refresh can rebuild row ids
    /// deterministically after files are added or deleted.
    pub fn new(
        root: PathBuf,
        content: ContentFilter,
        model_id: String,
        embeddings: &EmbeddingMatrix,
        files: Vec<CachedFileRecord>,
    ) -> Self {
        Self {
            cache_version: CACHE_VERSION,
            chunker_version: CHUNKER_VERSION,
            root,
            content,
            model_id,
            embedding_format: EmbeddingFormat::F32LittleEndian,
            embedding_dimensions: embeddings.dimensions(),
            embedding_rows: embeddings.row_count(),
            embedding_sha256: embedding_sha256(embeddings),
            files,
        }
    }

    /// Checks whether cached metadata is usable for a requested index.
    ///
    /// Root, content filter, and model id are part of the identity because any
    /// mismatch would make either the chunk set or the vector space incompatible
    /// with the current query.
    pub fn is_valid_for(&self, root: &Path, content: ContentFilter, model_id: &str) -> bool {
        self.cache_version == CACHE_VERSION
            && self.chunker_version == CHUNKER_VERSION
            && self.root == root
            && self.content == content
            && self.model_id == model_id
            && self.embedding_format == EmbeddingFormat::F32LittleEndian
            && self.is_internally_consistent()
    }

    /// Validates metadata after the binary vector sidecar has been read.
    ///
    /// This second-stage check prevents a JSON payload from claiming row ranges
    /// that the sidecar cannot satisfy.
    fn is_loadable_with(&self, embeddings: &EmbeddingMatrix) -> bool {
        self.cache_version == CACHE_VERSION
            && self.chunker_version == CHUNKER_VERSION
            && self.embedding_format == EmbeddingFormat::F32LittleEndian
            && self.embedding_dimensions == embeddings.dimensions()
            && self.embedding_rows == embeddings.row_count()
            && self.embedding_sha256 == embedding_sha256(embeddings)
            && self.is_internally_consistent()
    }

    /// Verifies record-local ranges without reading vector contents.
    fn is_internally_consistent(&self) -> bool {
        self.files.iter().all(|record| {
            record.is_consistent()
                && record
                    .embedding_start
                    .checked_add(record.embedding_count)
                    .is_some_and(|end| end <= self.embedding_rows)
        })
    }
}

/// Binary vector encoding used by the sidecar file.
///
/// Keeping this explicit lets a future f16 or quantized cache reject old files
/// cleanly without guessing from byte length.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingFormat {
    F32LittleEndian,
}

/// Per-file cache entry used for incremental refresh.
///
/// The manifest is the cheap reuse key, while the content hash records what was
/// actually chunked. `embedding_start..embedding_start + embedding_count` must
/// line up with `chunks`; zero-chunk records are valid for empty or unreadable
/// files and can be reused until their manifest changes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CachedFileRecord {
    pub manifest: FileManifestEntry,
    pub content_hash: String,
    pub chunks: Vec<Chunk>,
    pub embedding_start: usize,
    pub embedding_count: usize,
}

impl CachedFileRecord {
    /// Builds a record after chunking and embedding have assigned matrix rows.
    pub fn new(
        manifest: FileManifestEntry,
        content_hash: String,
        chunks: Vec<Chunk>,
        embedding_start: usize,
        embedding_count: usize,
    ) -> Self {
        Self {
            manifest,
            content_hash,
            chunks,
            embedding_start,
            embedding_count,
        }
    }

    /// Returns true when a current manifest can reuse this file record.
    ///
    /// Size and nanosecond mtime are the fast path. If either changes, refresh
    /// re-reads the file so only that file pays chunking and embedding cost.
    pub fn can_reuse_for(&self, manifest: &FileManifestEntry) -> bool {
        &self.manifest == manifest && self.is_consistent()
    }

    /// Checks the row-count invariant that makes file-local cache records safe
    /// to flatten into the search index.
    fn is_consistent(&self) -> bool {
        self.chunks.len() == self.embedding_count
    }
}

/// Returns the default persistent cache directory for code-search indexes.
pub fn default_cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("devo")
        .join("code-search")
        .join("indexes")
}

/// Derives the JSON metadata path for an index identity.
pub fn cache_file_path(
    cache_dir: &Path,
    root: &Path,
    content: ContentFilter,
    model_id: &str,
) -> PathBuf {
    let mut file_name = cache_key(root, content, model_id);
    file_name.push_str(".json");
    cache_dir.join(file_name)
}

/// Returns the binary vector sidecar path for a metadata file.
pub fn embedding_file_path(metadata_path: &Path) -> PathBuf {
    metadata_path.with_extension("embeddings.f32")
}

/// Loads a cached index, returning `None` for any stale or partial cache.
///
/// Cache failures intentionally do not surface as user-visible errors because
/// the cache is an optimization. The service can rebuild the index from source
/// files when the JSON, version, row ranges, or binary sidecar do not validate.
pub fn load_payload(path: &Path) -> Option<CachedIndex> {
    let file = std::fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    let payload = serde_json::from_reader::<_, CachedIndexPayloadV4>(reader).ok()?;
    let embeddings = read_embeddings(&embedding_file_path(path), &payload)?;
    payload
        .is_loadable_with(&embeddings)
        .then_some(CachedIndex {
            payload,
            embeddings,
        })
}

/// Saves cache metadata and row-major embeddings for a completed refresh.
///
/// The binary sidecar is written before JSON metadata, and the JSON contains a
/// digest of the sidecar bytes. If the process dies between writes, stale JSON
/// can no longer validate against a newer sidecar with the same shape.
pub fn save_payload(
    path: &Path,
    payload: &CachedIndexPayloadV4,
    embeddings: &EmbeddingMatrix,
) -> Result<(), CodeSearchError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    validate_payload_embeddings(payload, embeddings)?;
    write_embeddings(&embedding_file_path(path), embeddings)?;
    let file = std::fs::File::create(path)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, payload)
        .map_err(|error| CodeSearchError::Io(error.to_string()))?;
    writer.flush()?;
    Ok(())
}

/// Hashes file text so cache records can describe what was chunked.
pub fn content_hash(text: &str) -> String {
    hex_sha256(text.as_bytes())
}

fn validate_payload_embeddings(
    payload: &CachedIndexPayloadV4,
    embeddings: &EmbeddingMatrix,
) -> Result<(), CodeSearchError> {
    if payload.cache_version != CACHE_VERSION
        || payload.chunker_version != CHUNKER_VERSION
        || payload.embedding_dimensions != embeddings.dimensions()
        || payload.embedding_rows != embeddings.row_count()
        || payload.embedding_sha256 != embedding_sha256(embeddings)
        || !payload.is_internally_consistent()
    {
        return Err(CodeSearchError::Index(
            "cache metadata and embedding matrix do not match".to_string(),
        ));
    }
    Ok(())
}

fn read_embeddings(path: &Path, payload: &CachedIndexPayloadV4) -> Option<EmbeddingMatrix> {
    let expected_values = payload
        .embedding_rows
        .checked_mul(payload.embedding_dimensions)?;
    let expected_bytes = expected_values.checked_mul(F32_BYTES)?;
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() != u64::try_from(expected_bytes).ok()? {
        return None;
    }
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut rows = Vec::with_capacity(expected_values);
    let mut bytes = [0_u8; F32_BYTES];
    for _ in 0..expected_values {
        reader.read_exact(&mut bytes).ok()?;
        hasher.update(bytes);
        rows.push(f32::from_le_bytes(bytes));
    }
    if reader.read(&mut [0_u8; 1]).ok()? != 0 {
        return None;
    }
    if hex_digest(hasher) != payload.embedding_sha256 {
        return None;
    }
    EmbeddingMatrix::new(payload.embedding_dimensions, rows).ok()
}

fn write_embeddings(path: &Path, embeddings: &EmbeddingMatrix) -> Result<(), CodeSearchError> {
    let file = std::fs::File::create(path)?;
    let mut writer = BufWriter::new(file);
    for value in embeddings.rows() {
        writer.write_all(&value.to_le_bytes())?;
    }
    writer.flush()?;
    Ok(())
}

fn embedding_sha256(embeddings: &EmbeddingMatrix) -> String {
    let mut hasher = Sha256::new();
    for value in embeddings.rows() {
        hasher.update(value.to_le_bytes());
    }
    hex_digest(hasher)
}

fn cache_key(root: &Path, content: ContentFilter, model_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(root.to_string_lossy().as_bytes());
    let content_bytes: &[u8] = match content {
        ContentFilter::Code => b"Code",
        ContentFilter::Docs => b"Docs",
        ContentFilter::Config => b"Config",
        ContentFilter::All => b"All",
    };
    hasher.update(content_bytes);
    hasher.update(model_id.as_bytes());
    hex_digest(hasher)
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_digest(hasher)
}

fn hex_digest(hasher: Sha256) -> String {
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex
}

#[cfg(test)]
mod tests {
    use std::fs;

    use pretty_assertions::assert_eq;

    use super::*;

    fn cached_index() -> CachedIndex {
        let embeddings = EmbeddingMatrix::from_vectors(vec![vec![1.0, 0.0]]).expect("matrix");
        let record = CachedFileRecord::new(
            FileManifestEntry {
                path: PathBuf::from("src/lib.rs"),
                size: 10,
                modified_unix_nanos: 1,
            },
            content_hash("fn parse() {}"),
            vec![Chunk {
                content: "fn parse() {}".to_string(),
                file_path: PathBuf::from("src/lib.rs"),
                start_line: 1,
                end_line: 1,
                language: "rust".to_string(),
            }],
            0,
            1,
        );
        let payload = CachedIndexPayloadV4::new(
            PathBuf::from("/repo"),
            ContentFilter::Code,
            "model-a".to_string(),
            &embeddings,
            vec![record],
        );
        CachedIndex {
            payload,
            embeddings,
        }
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: cache metadata validates model, chunker, content filter, and embedding ranges.
    #[test]
    fn cached_payload_validates_header_and_records() {
        let cached = cached_index();

        let mut wrong_chunker = cached.payload.clone();
        wrong_chunker.chunker_version = 1;
        let validity = vec![
            cached
                .payload
                .is_valid_for(Path::new("/repo"), ContentFilter::Code, "model-a"),
            cached
                .payload
                .is_valid_for(Path::new("/repo"), ContentFilter::Code, "model-b"),
            wrong_chunker.is_valid_for(Path::new("/repo"), ContentFilter::Code, "model-a"),
        ];

        assert_eq!(validity, vec![true, false, false]);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: cache persists JSON metadata separately from row-major f32 embedding bytes.
    #[test]
    fn cache_round_trips_metadata_and_binary_embeddings() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("index.json");
        let cached = cached_index();

        save_payload(&path, &cached.payload, &cached.embeddings).expect("save");
        let json = fs::read_to_string(&path).expect("json");
        let embedding_bytes = fs::read(embedding_file_path(&path)).expect("binary");
        let loaded = load_payload(&path);

        assert_eq!(json.contains("embeddings"), false);
        assert_eq!(json.contains("\"cache_version\":5"), true);
        assert_eq!(json.contains("\"chunker_version\":2"), true);
        assert_eq!(json.contains("\"embedding_sha256\""), true);
        assert_eq!(embedding_bytes.len(), 8);
        assert_eq!(loaded, Some(cached));
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: stale v1/v2/v3/v4 and malformed cache files are treated as disposable cache misses.
    #[test]
    fn load_payload_rejects_stale_and_malformed_cache_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let v2_path = temp.path().join("v2.json");
        let v3_path = temp.path().join("v3.json");
        let v4_path = temp.path().join("v4.json");
        let malformed_path = temp.path().join("malformed.json");
        fs::write(
            &v2_path,
            r#"{"cache_version":2,"root":"/repo","content":"code","model_id":"test","files":[]}"#,
        )
        .expect("write v2");
        fs::write(
            &v3_path,
            r#"{"cache_version":3,"root":"/repo","content":"code","model_id":"test","embedding_format":"f32_little_endian","embedding_dimensions":0,"embedding_rows":0,"files":[]}"#,
        )
        .expect("write v3");
        fs::write(
            &v4_path,
            r#"{"cache_version":4,"chunker_version":2,"root":"/repo","content":"code","model_id":"test","embedding_format":"f32_little_endian","embedding_dimensions":0,"embedding_rows":0,"files":[]}"#,
        )
        .expect("write v4");
        fs::write(&malformed_path, b"{").expect("write malformed");

        let loaded = vec![
            load_payload(&v2_path),
            load_payload(&v3_path),
            load_payload(&v4_path),
            load_payload(&malformed_path),
        ];

        assert_eq!(loaded, vec![None, None, None, None]);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: missing, truncated, or dimension-mismatched binary embedding files are cache misses.
    #[test]
    fn load_payload_rejects_invalid_binary_embeddings() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("index.json");
        let cached = cached_index();
        save_payload(&path, &cached.payload, &cached.embeddings).expect("save");

        fs::remove_file(embedding_file_path(&path)).expect("remove binary");
        let missing = load_payload(&path);

        save_payload(&path, &cached.payload, &cached.embeddings).expect("save");
        fs::write(embedding_file_path(&path), [0_u8; 4]).expect("truncate");
        let truncated = load_payload(&path);

        save_payload(&path, &cached.payload, &cached.embeddings).expect("save");
        let mut json =
            serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&path).expect("json"))
                .expect("json value");
        json["embedding_dimensions"] = serde_json::json!(3);
        fs::write(&path, serde_json::to_vec(&json).expect("json bytes")).expect("write json");
        let wrong_dimensions = load_payload(&path);

        assert_eq!(
            vec![missing, truncated, wrong_dimensions],
            vec![None, None, None]
        );
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: stale metadata cannot validate against a newer embedding sidecar.
    #[test]
    fn load_payload_rejects_sidecar_that_does_not_match_metadata_hash() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("index.json");
        let cached = cached_index();
        save_payload(&path, &cached.payload, &cached.embeddings).expect("save");
        let newer_embeddings =
            EmbeddingMatrix::from_vectors(vec![vec![0.0, 1.0]]).expect("newer matrix");

        write_embeddings(&embedding_file_path(&path), &newer_embeddings).expect("write sidecar");
        let loaded = load_payload(&path);

        assert_eq!(loaded, None);
    }

    /// Trace: L2-DES-TOOL-001
    /// Verifies: cache payloads require the current chunker version.
    #[test]
    fn load_payload_rejects_wrong_chunker_version() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("index.json");
        let cached = cached_index();
        save_payload(&path, &cached.payload, &cached.embeddings).expect("save");
        let mut json =
            serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&path).expect("json"))
                .expect("json value");
        json["chunker_version"] = serde_json::json!(1);
        fs::write(&path, serde_json::to_vec(&json).expect("json bytes")).expect("write json");

        let loaded = load_payload(&path);

        assert_eq!(loaded, None);
    }
}
