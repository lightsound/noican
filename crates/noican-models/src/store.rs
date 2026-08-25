//! On-disk storage for model weights.
//!
//! Weights are too large to commit, so the catalog records a URL and a digest
//! and this module fetches and verifies them on demand. Verification is not
//! optional: a truncated download produces a graph that loads and runs and
//! outputs plausible-sounding rubbish, which is far harder to diagnose than a
//! checksum failure.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::catalog::{Artifact, ModelDescriptor};
use crate::error::{Error, Result};

/// Environment variable that overrides the default weights directory.
pub const MODEL_DIR_ENV: &str = "NOICAN_MODEL_DIR";

/// Bytes read per chunk while downloading and hashing.
const CHUNK_SIZE: usize = 64 * 1024;

/// Progress of a single artifact download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Bytes received so far.
    pub downloaded: u64,
    /// Total size, when the server reported one.
    pub total: Option<u64>,
}

/// A directory holding downloaded model weights.
#[derive(Debug, Clone)]
pub struct ModelStore {
    root: PathBuf,
}

impl ModelStore {
    /// Uses `root` as the weights directory.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The directory used when none is given: `$NOICAN_MODEL_DIR`, or `models`
    /// under the current directory.
    #[must_use]
    pub fn default_root() -> PathBuf {
        std::env::var_os(MODEL_DIR_ENV).map_or_else(|| PathBuf::from("models"), PathBuf::from)
    }

    /// A store rooted at [`Self::default_root`].
    #[must_use]
    pub fn from_environment() -> Self {
        Self::new(Self::default_root())
    }

    /// The weights directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where one artifact of `model` lives.
    #[must_use]
    pub fn path_of(&self, model: &ModelDescriptor, artifact: &Artifact) -> PathBuf {
        self.root.join(model.id).join(artifact.file_name)
    }

    /// Whether every artifact of `model` is present on disk.
    ///
    /// Presence only; use [`Self::verify`] to confirm the contents.
    #[must_use]
    pub fn is_present(&self, model: &ModelDescriptor) -> bool {
        model
            .artifacts
            .iter()
            .all(|artifact| self.path_of(model, artifact).is_file())
    }

    /// Returns the path of `model`'s primary artifact, checking it exists.
    ///
    /// # Errors
    ///
    /// Returns [`Error::MissingWeights`] if it has not been downloaded.
    pub fn require(&self, model: &ModelDescriptor) -> Result<PathBuf> {
        let path = self.path_of(model, model.primary_artifact());
        if path.is_file() {
            Ok(path)
        } else {
            Err(Error::MissingWeights {
                model: model.id.to_owned(),
                path,
            })
        }
    }

    /// Recomputes and checks the digest of every artifact of `model`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::MissingWeights`] if a file is absent,
    /// [`Error::ChecksumMismatch`] if its contents differ, or [`Error::Io`] on
    /// a read failure.
    pub fn verify(&self, model: &ModelDescriptor) -> Result<()> {
        for artifact in model.artifacts {
            let path = self.path_of(model, artifact);
            if !path.is_file() {
                return Err(Error::MissingWeights {
                    model: model.id.to_owned(),
                    path,
                });
            }
            let actual = hash_file(&path)?;
            if actual != artifact.sha256 {
                return Err(Error::ChecksumMismatch {
                    path,
                    expected: artifact.sha256.to_owned(),
                    actual,
                });
            }
        }
        Ok(())
    }

    /// Downloads whatever `model` is missing, verifying each file.
    ///
    /// Files already present are verified rather than re-downloaded. A file
    /// whose digest does not match is replaced.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Download`] if the transfer fails, [`Error::Io`] on a
    /// filesystem failure, or [`Error::ChecksumMismatch`] if a freshly
    /// downloaded file still does not match — which means the recorded digest
    /// or the upstream file has changed.
    pub fn fetch(
        &self,
        model: &ModelDescriptor,
        on_progress: &mut dyn FnMut(&Artifact, Progress),
    ) -> Result<()> {
        let directory = self.root.join(model.id);
        fs::create_dir_all(&directory).map_err(|source| Error::Io {
            operation: "create directory",
            path: directory.clone(),
            source,
        })?;

        for artifact in model.artifacts {
            let path = self.path_of(model, artifact);
            if path.is_file() {
                let actual = hash_file(&path)?;
                if actual == artifact.sha256 {
                    tracing::debug!(
                        model = model.id,
                        file = artifact.file_name,
                        "already present"
                    );
                    continue;
                }
                tracing::warn!(
                    model = model.id,
                    file = artifact.file_name,
                    "digest mismatch, re-downloading"
                );
            }
            download(artifact, &path, on_progress)?;

            let actual = hash_file(&path)?;
            if actual != artifact.sha256 {
                return Err(Error::ChecksumMismatch {
                    path,
                    expected: artifact.sha256.to_owned(),
                    actual,
                });
            }
        }
        Ok(())
    }
}

/// Streams `artifact` to `path` via a temporary file.
///
/// The download lands on a `.part` file and is renamed only once it is
/// complete, so an interrupted transfer can never be mistaken for a usable
/// model.
fn download(
    artifact: &Artifact,
    path: &Path,
    on_progress: &mut dyn FnMut(&Artifact, Progress),
) -> Result<()> {
    let mut response = ureq::get(artifact.url)
        .call()
        .map_err(|source| Error::Download {
            url: artifact.url.to_owned(),
            source: Box::new(source),
        })?;

    let total = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());

    let partial = path.with_extension("part");
    let mut file = fs::File::create(&partial).map_err(|source| Error::Io {
        operation: "create file",
        path: partial.clone(),
        source,
    })?;

    let mut reader = response.body_mut().as_reader();
    let mut buffer = vec![0u8; CHUNK_SIZE];
    let mut downloaded = 0u64;
    loop {
        let read = reader.read(&mut buffer).map_err(|source| Error::Io {
            operation: "read from",
            path: PathBuf::from(artifact.url),
            source,
        })?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])
            .map_err(|source| Error::Io {
                operation: "write to",
                path: partial.clone(),
                source,
            })?;
        downloaded += read as u64;
        on_progress(artifact, Progress { downloaded, total });
    }

    file.flush().map_err(|source| Error::Io {
        operation: "flush",
        path: partial.clone(),
        source,
    })?;
    drop(file);

    fs::rename(&partial, path).map_err(|source| Error::Io {
        operation: "rename into place",
        path: path.to_path_buf(),
        source,
    })
}

/// Computes the lowercase hex SHA-256 of a file.
fn hash_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).map_err(|source| Error::Io {
        operation: "open",
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK_SIZE];
    loop {
        let read = file.read(&mut buffer).map_err(|source| Error::Io {
            operation: "read",
            path: path.to_path_buf(),
            source,
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

/// Formats bytes as lowercase hex.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            // Writing to a String cannot fail.
            let _ = write!(out, "{byte:02x}");
            out
        })
}

#[cfg(test)]
mod tests {
    use super::{ModelStore, hash_file, hex};
    use crate::catalog;
    use std::io::Write as _;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("noican-store-test-{name}"));
        drop(std::fs::remove_dir_all(&dir));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn hex_formats_with_leading_zeros() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff]), "000fff");
    }

    #[test]
    fn hashes_a_known_value() {
        let dir = temp_dir("hash");
        let path = dir.join("empty");
        std::fs::File::create(&path).unwrap();
        // The SHA-256 of the empty string.
        assert_eq!(
            hash_file(&path).unwrap(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn paths_are_namespaced_by_model_id() {
        let store = ModelStore::new("/weights");
        let model = catalog::find("fastenhancer-t").unwrap();
        assert_eq!(
            store.path_of(model, model.primary_artifact()),
            std::path::Path::new("/weights/fastenhancer-t/fastenhancer_t.onnx")
        );
        assert_eq!(store.root(), std::path::Path::new("/weights"));
    }

    #[test]
    fn missing_weights_are_reported_not_ignored() {
        let store = ModelStore::new(temp_dir("missing"));
        let model = catalog::find("gtcrn").unwrap();
        assert!(!store.is_present(model));
        assert!(store.require(model).is_err());
        assert!(store.verify(model).is_err());
    }

    #[test]
    fn corrupted_weights_fail_verification() {
        let dir = temp_dir("corrupt");
        let store = ModelStore::new(&dir);
        let model = catalog::find("gtcrn").unwrap();
        let path = store.path_of(model, model.primary_artifact());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(b"not an onnx graph").unwrap();
        drop(file);

        assert!(store.is_present(model));
        let error = store.verify(model).unwrap_err();
        assert!(
            matches!(error, crate::Error::ChecksumMismatch { .. }),
            "expected a checksum mismatch, got {error}"
        );
    }
}
