//! Versioned atomic file checkpoints. Directory is trusted and owned by the caller.
//! Local filesystem only; synchronous bounded I/O, not a distributed CAS store.
use crate::CheckpointStore;
use async_trait::async_trait;
use bbgo_types::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

const MAX_PAYLOAD: usize = 4 * 1024 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u32,
    key: String,
    payload: Vec<u8>,
    sha256: Vec<u8>,
}
pub struct FileStore {
    root: PathBuf,
    lock: Mutex<()>,
}
fn adapter(error: impl std::fmt::Display) -> Error {
    Error::Adapter(error.to_string())
}
impl FileStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(adapter)?;
        Ok(Self {
            root: root.canonicalize().map_err(adapter)?,
            lock: Mutex::new(()),
        })
    }
    fn path(&self, key: &str) -> Result<PathBuf> {
        if key.is_empty() || key.len() > 1024 {
            return Err(Error::Invalid("invalid checkpoint key".into()));
        }
        let name: String = Sha256::digest(key.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Ok(self.root.join(format!("{name}.json")))
    }
}
#[async_trait]
impl CheckpointStore for FileStore {
    async fn load(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| adapter("checkpoint lock poisoned"))?;
        let file = match fs::File::open(self.path(key)?) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(adapter(error)),
        };
        let mut bytes = Vec::new();
        file.take((MAX_PAYLOAD * 6 + 8193) as u64)
            .read_to_end(&mut bytes)
            .map_err(adapter)?;
        if bytes.len() > MAX_PAYLOAD * 6 + 8192 {
            return Err(adapter("checkpoint file too large"));
        }
        let envelope: Envelope = serde_json::from_slice(&bytes).map_err(adapter)?;
        if envelope.version != 1
            || envelope.key != key
            || envelope.payload.len() > MAX_PAYLOAD
            || envelope.sha256 != Sha256::digest(&envelope.payload).to_vec()
        {
            return Err(adapter("checkpoint version, identity or checksum mismatch"));
        }
        Ok(Some(envelope.payload))
    }
    async fn save(&self, key: &str, value: &[u8]) -> Result<()> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| adapter("checkpoint lock poisoned"))?;
        let path = self.path(key)?;
        if value.len() > MAX_PAYLOAD {
            return Err(Error::Invalid("checkpoint payload too large".into()));
        }
        let bytes = serde_json::to_vec(&Envelope {
            version: 1,
            key: key.into(),
            payload: value.into(),
            sha256: Sha256::digest(value).to_vec(),
        })
        .map_err(adapter)?;
        let (temp, mut file) = loop {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let temp = self
                .root
                .join(format!(".checkpoint-{}-{id}.tmp", std::process::id()));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&temp) {
                Ok(file) => break (temp, file),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(adapter(error)),
            }
        };
        let result = (|| {
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, path)?;
            fs::File::open(&self.root)?.sync_all()
        })()
        .map_err(adapter);
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result
    }
}
