//! Session-owned output capabilities with bounded, integrity-checked reads.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const PAGE_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CaptureState {
    Streaming,
    Complete,
    Incomplete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputArtifact {
    pub id: String,
    pub file_identity: String,
    pub owner: String,
    pub call_id: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub digest: String,
    pub state: CaptureState,
}

/// A store grants reads only to artifacts registered under its owning session.
#[derive(Debug)]
pub struct OutputStore {
    root: PathBuf,
    owner: String,
    registered: Mutex<std::collections::HashMap<PathBuf, OutputArtifact>>,
}

/// A live capture owns its open handle even if the directory entry is replaced.
#[derive(Debug)]
pub struct OutputCapture {
    store: Arc<OutputStore>,
    file: File,
    digest: Sha256,
    artifact: OutputArtifact,
}

impl OutputStore {
    pub fn new(root: PathBuf, owner: String) -> Self {
        Self {
            root,
            owner,
            registered: Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Restore only references read from this session's durable journal. A
    /// manifest on disk or an arbitrary path does not grant read authority.
    pub fn restore_references(&self, artifacts: impl IntoIterator<Item = OutputArtifact>) {
        let mut registered = self.registered.lock().expect("output registry lock");
        for artifact in artifacts {
            registered.insert(artifact.path.clone(), artifact);
        }
    }

    /// Remove an unreferenced generated file, refusing substituted identities.
    /// The caller must establish that no surviving session references it.
    pub fn delete_registered_artifact(artifact: &OutputArtifact) -> std::io::Result<()> {
        let path = &artifact.path;
        if uuid::Uuid::parse_str(&artifact.id).is_err()
            || path.file_name().and_then(|name| name.to_str())
                != Some(format!("{}.txt", artifact.id).as_str())
            || path
                .parent()
                .and_then(Path::extension)
                .and_then(|name| name.to_str())
                != Some("outputs")
        {
            return Err(std::io::Error::other("invalid generated output path"));
        }
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if std::fs::symlink_metadata(path)?.file_type().is_symlink()
            || crate::output_identity::file_identity(&file)? != artifact.file_identity
        {
            return Err(std::io::Error::other(
                "output artifact file identity changed",
            ));
        }
        drop(file);
        std::fs::remove_file(path)?;
        let manifest = path.with_extension("json");
        match std::fs::remove_file(manifest) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub fn capture(self: &Arc<Self>, call_id: &str) -> std::io::Result<OutputCapture> {
        std::fs::create_dir_all(&self.root)?;
        let id = uuid::Uuid::new_v4().to_string();
        let path = self.root.join(format!("{id}.txt"));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .read(true)
            .open(&path)?;
        let file_identity = crate::output_identity::file_identity(&file)?;
        Ok(OutputCapture {
            store: Arc::clone(self),
            file,
            digest: Sha256::new(),
            artifact: OutputArtifact {
                id,
                file_identity,
                owner: self.owner.clone(),
                call_id: call_id.into(),
                path,
                bytes: 0,
                digest: String::new(),
                state: CaptureState::Streaming,
            },
        })
    }

    pub fn save(
        self: &Arc<Self>,
        call_id: &str,
        content: &[u8],
    ) -> std::io::Result<OutputArtifact> {
        let mut capture = self.capture(call_id)?;
        capture.append(content)?;
        capture.finish()
    }

    /// Return None for an unregistered path; callers must use ordinary authorization.
    /// Validate captured content through the same handle subsequently used for paging.
    pub fn read(&self, path: &Path, input: &serde_json::Value) -> std::io::Result<Option<String>> {
        let Some(artifact) = self
            .registered
            .lock()
            .map_err(|_| std::io::Error::other("output registry lock poisoned"))?
            .get(path)
            .cloned()
        else {
            return Ok(None);
        };
        if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
            return Err(std::io::Error::other("output artifact path was replaced"));
        }
        let mut file = File::open(path)?;
        if crate::output_identity::file_identity(&file)? != artifact.file_identity {
            return Err(std::io::Error::other(
                "output artifact file identity changed",
            ));
        }
        let mut digest = Sha256::new();
        let mut remaining = artifact.bytes;
        let mut buffer = [0_u8; PAGE_BYTES];
        while remaining > 0 {
            let count = file.read(&mut buffer[..remaining.min(PAGE_BYTES as u64) as usize])?;
            if count == 0 {
                return Err(std::io::Error::other("output artifact is incomplete"));
            }
            digest.update(&buffer[..count]);
            remaining -= count as u64;
        }
        if format!("{:x}", digest.finalize()) != artifact.digest {
            return Err(std::io::Error::other(
                "output artifact content was replaced",
            ));
        }
        let mut offset = input["byteOffset"]
            .as_u64()
            .unwrap_or(0)
            .min(artifact.bytes);
        let limit = input["byteLimit"]
            .as_u64()
            .unwrap_or(PAGE_BYTES as u64)
            .clamp(4, PAGE_BYTES as u64);
        if input.get("byteOffset").is_none() {
            let wanted = input["offset"].as_u64().unwrap_or(1).max(1);
            file.seek(SeekFrom::Start(0))?;
            let mut line = 1;
            while line < wanted && offset < artifact.bytes {
                let count = file.read(
                    &mut buffer[..(artifact.bytes - offset).min(PAGE_BYTES as u64) as usize],
                )?;
                if count == 0 {
                    break;
                }
                for byte in &buffer[..count] {
                    offset += 1;
                    if *byte == b'\n' {
                        line += 1;
                    }
                    if line == wanted {
                        break;
                    }
                }
            }
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut page = vec![0; (artifact.bytes - offset).min(limit) as usize];
        file.read_exact(&mut page)?;
        if input.get("byteOffset").is_none()
            && let Some(lines) = input["limit"].as_u64()
        {
            let mut seen = 0;
            if let Some(end) = page.iter().position(|byte| {
                if *byte == b'\n' {
                    seen += 1;
                }
                seen >= lines.max(1)
            }) {
                page.truncate(end + 1);
            }
        }
        // Avoid splitting a UTF-8 codepoint at the end of a page.
        if let Err(error) = std::str::from_utf8(&page)
            && error.error_len().is_none()
            && error.valid_up_to() > 0
        {
            page.truncate(error.valid_up_to());
        }
        let next = offset + page.len() as u64;
        let text = String::from_utf8_lossy(&page);
        Ok(Some(format!(
            "{text}\n[Output artifact: bytes {offset}..{next} of {}; state {:?}. {}]",
            artifact.bytes,
            artifact.state,
            if next < artifact.bytes {
                format!("Continue with byteOffset={next}, byteLimit={PAGE_BYTES}.")
            } else {
                "End of captured output.".into()
            }
        )))
    }
}

impl OutputCapture {
    pub fn append(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if let Err(error) = self.file.write_all(bytes) {
            self.artifact.state = CaptureState::Incomplete;
            return Err(error);
        }
        self.digest.update(bytes);
        self.artifact.bytes += bytes.len() as u64;
        Ok(())
    }

    pub fn mark_incomplete(&mut self) {
        self.artifact.state = CaptureState::Incomplete;
    }

    pub fn snapshot(&mut self) -> std::io::Result<OutputArtifact> {
        self.file.sync_data()?;
        self.artifact.digest = format!("{:x}", self.digest.clone().finalize());
        let path = self.store.root.join(format!("{}.json", self.artifact.id));
        // Serialize callers through their capture mutex; reads see an old complete
        // manifest or a new complete manifest, never partially written metadata.
        let temp = self
            .store
            .root
            .join(format!("{}.tmp", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        file.write_all(&serde_json::to_vec(&self.artifact)?)?;
        file.sync_data()?;
        drop(file);
        std::fs::rename(&temp, path)?;
        self.store.restore_references([self.artifact.clone()]);
        Ok(self.artifact.clone())
    }

    pub fn finish(&mut self) -> std::io::Result<OutputArtifact> {
        if self.artifact.state == CaptureState::Streaming {
            self.artifact.state = CaptureState::Complete;
        }
        self.snapshot()
    }
}

pub type SharedOutputCapture = Arc<Mutex<OutputCapture>>;

impl OutputArtifact {
    pub fn notice(&self) -> String {
        let description = match self.state {
            CaptureState::Complete => "Full output saved",
            CaptureState::Streaming => "Output captured so far saved",
            CaptureState::Incomplete => "Output capture incomplete; saved prefix",
        };
        format!(
            "Output exceeded the inline limit. {description} to `{}`. Use `read` with `offset` and `limit` to inspect it. Reading this generated output file requires no additional authorization.",
            self.path.display()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    /// Trace: L2-DES-CONTEXT-004
    #[test]
    fn saved_output_survives_reopening_and_rejects_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(OutputStore::new(directory.path().into(), "session".into()));
        let artifact = store.save("call", "héllo\nworld".as_bytes()).unwrap();
        let reopened = OutputStore::new(directory.path().into(), "session".into());
        reopened.restore_references([artifact.clone()]);
        assert_eq!(reopened.read(&artifact.path, &serde_json::json!({"offset": 2, "limit": 1})).unwrap(),
            Some("world\n[Output artifact: bytes 7..12 of 12; state Complete. End of captured output.]".into()));
        let other = OutputStore::new(directory.path().into(), "another-session".into());
        assert_eq!(
            other.read(&artifact.path, &serde_json::json!({})).unwrap(),
            None
        );
        std::fs::write(&artifact.path, "other content").unwrap();
        assert!(
            reopened
                .read(&artifact.path, &serde_json::json!({}))
                .is_err()
        );
    }

    /// Trace: L2-DES-CONTEXT-004
    #[test]
    fn long_line_has_bounded_continuation() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(OutputStore::new(directory.path().into(), "session".into()));
        let artifact = store.save("call", &vec![b'x'; PAGE_BYTES + 5]).unwrap();
        let first = store
            .read(&artifact.path, &serde_json::json!({}))
            .unwrap()
            .unwrap();
        assert!(first.ends_with("Continue with byteOffset=32768, byteLimit=32768.]"));
        assert_eq!(store.read(&artifact.path, &serde_json::json!({"byteOffset": PAGE_BYTES})).unwrap(),
            Some("xxxxx\n[Output artifact: bytes 32768..32773 of 32773; state Complete. End of captured output.]".into()));
        assert_eq!(
            store
                .read(
                    &directory.path().join("unregistered.txt"),
                    &serde_json::json!({})
                )
                .unwrap(),
            None
        );
    }
    /// Trace: L2-DES-CONTEXT-004. Polls retain one file and UTF-8 pagination advances.
    #[test]
    fn growing_capture_and_tiny_unicode_pages() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(OutputStore::new(directory.path().into(), "session".into()));
        let mut capture = store.capture("process").unwrap();
        capture.append("😀".as_bytes()).unwrap();
        let streaming = capture.snapshot().unwrap();
        capture.append("é".as_bytes()).unwrap();
        let completed = capture.finish().unwrap();
        assert_eq!(
            (&streaming.id, &streaming.path),
            (&completed.id, &completed.path)
        );
        assert_eq!(store.read(&completed.path, &serde_json::json!({"byteOffset":0,"byteLimit":1})).unwrap(),
            Some("😀\n[Output artifact: bytes 0..4 of 6; state Complete. Continue with byteOffset=4, byteLimit=32768.]".into()));
        assert_eq!(
            store
                .read(
                    &completed.path,
                    &serde_json::json!({"byteOffset":4,"byteLimit":1})
                )
                .unwrap(),
            Some(
                "é\n[Output artifact: bytes 4..6 of 6; state Complete. End of captured output.]"
                    .into()
            )
        );
    }

    /// Trace: L2-DES-CONTEXT-004. Equal contents do not authorize another file.
    #[test]
    fn identical_replacement_is_rejected_by_identity() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(OutputStore::new(directory.path().into(), "session".into()));
        let artifact = store.save("call", b"same").unwrap();
        // Retain the old inode/file ID while replacing its directory entry.
        std::fs::rename(&artifact.path, directory.path().join("original")).unwrap();
        std::fs::write(&artifact.path, b"same").unwrap();
        assert!(store.read(&artifact.path, &serde_json::json!({})).is_err());
    }
}
