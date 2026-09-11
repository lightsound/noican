//! Downloading and verifying model weights.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use crate::manifest::ModelSpec;

/// Errors from the weight fetcher.
#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    /// Network or HTTP failure.
    #[error("download of {url} failed: {source}")]
    Http {
        /// URL that failed.
        url: String,
        /// Underlying error.
        #[source]
        source: Box<ureq::Error>,
    },
    /// Local filesystem failure.
    #[error("filesystem error at {path}: {source}")]
    Io {
        /// Affected path.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// Downloaded bytes do not match the pinned digest.
    #[error("checksum mismatch for {file}: expected {expected}, got {got}")]
    Checksum {
        /// File name.
        file: String,
        /// Pinned digest.
        expected: String,
        /// Actual digest.
        got: String,
    },
}

/// Returns the directory holding `model`'s files under `models_dir`.
#[must_use]
pub fn model_dir(models_dir: &Path, model: &ModelSpec) -> PathBuf {
    models_dir.join(model.id)
}

/// True when every file of `model` — and of every entry it
/// [depends on](ModelSpec::depends_on) — is already present under
/// `models_dir`.
#[must_use]
pub fn is_fetched(models_dir: &Path, model: &ModelSpec) -> bool {
    let dir = model_dir(models_dir, model);
    model.files.iter().all(|f| dir.join(f.name).is_file())
        && model.dependencies().all(|dep| is_fetched(models_dir, dep))
}

/// Downloads any missing files of `model` into `models_dir`, verifying
/// pinned SHA-256 digests.
///
/// Entries the model [depends on](ModelSpec::depends_on) are fetched
/// first, into their own directories. Calls `progress` with a
/// human-readable line per file event.
///
/// # Errors
///
/// Returns [`FetchError`] on network, filesystem, or checksum failure.
pub fn fetch_model(
    models_dir: &Path,
    model: &ModelSpec,
    mut progress: impl FnMut(&str),
) -> Result<(), FetchError> {
    fetch_model_dyn(models_dir, model, &mut progress)
}

/// Type-erased body of [`fetch_model`] (recursion over dependencies must
/// not re-instantiate the generic closure parameter).
fn fetch_model_dyn(
    models_dir: &Path,
    model: &ModelSpec,
    progress: &mut dyn FnMut(&str),
) -> Result<(), FetchError> {
    for dep in model.dependencies() {
        fetch_model_dyn(models_dir, dep, progress)?;
    }
    if model.files.is_empty() {
        return Ok(());
    }
    let dir = model_dir(models_dir, model);
    std::fs::create_dir_all(&dir).map_err(|source| FetchError::Io {
        path: dir.clone(),
        source,
    })?;
    for file in model.files {
        let dest = dir.join(file.name);
        if dest.is_file() {
            progress(&format!("{}/{}: already present", model.id, file.name));
            continue;
        }
        progress(&format!("{}/{}: downloading…", model.id, file.name));
        let bytes = download(file.url)?;
        if let Some(expected) = file.sha256 {
            let got = hex_sha256(&bytes);
            if got != expected {
                return Err(FetchError::Checksum {
                    file: file.name.to_owned(),
                    expected: expected.to_owned(),
                    got,
                });
            }
        }
        // Write via a temp file so an interrupted download never looks
        // complete to `is_fetched`.
        let tmp = dest.with_extension("part");
        std::fs::write(&tmp, &bytes).map_err(|source| FetchError::Io {
            path: tmp.clone(),
            source,
        })?;
        std::fs::rename(&tmp, &dest).map_err(|source| FetchError::Io {
            path: dest.clone(),
            source,
        })?;
        progress(&format!(
            "{}/{}: done ({} bytes)",
            model.id,
            file.name,
            bytes.len()
        ));
    }
    Ok(())
}

/// Returns a Hugging Face access token from the environment
/// (`NOICAN_HF_TOKEN` or `HF_TOKEN`), if any. Needed only for gated/private
/// repos (see docs/models.md).
fn hf_token() -> Option<String> {
    std::env::var("NOICAN_HF_TOKEN")
        .or_else(|_| std::env::var("HF_TOKEN"))
        .ok()
        .filter(|t| !t.is_empty())
}

fn download(url: &str) -> Result<Vec<u8>, FetchError> {
    let wrap = |source: ureq::Error| FetchError::Http {
        url: url.to_owned(),
        source: Box::new(source),
    };
    let mut request = ureq::get(url);
    if url.starts_with("https://huggingface.co/")
        && let Some(token) = hf_token()
    {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let mut response = request.call().map_err(wrap)?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .read_to_end(&mut bytes)
        .map_err(|source| wrap(ureq::Error::Io(source)))?;
    Ok(bytes)
}

fn hex_sha256(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_matches_known_vector() {
        assert_eq!(
            hex_sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn fetch_status_follows_dependencies() {
        let models_dir = std::env::temp_dir().join(format!(
            "noican-fetch-test-{}-{}",
            std::process::id(),
            line!()
        ));
        let hush = ModelSpec::find("hush").expect("registered");
        let wideband = ModelSpec::find("hush-48k").expect("registered");
        // No files of its own, but not fetched until its dependency is.
        assert!(!is_fetched(&models_dir, wideband));
        let dir = model_dir(&models_dir, hush);
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join(hush.files[0].name), b"stub").expect("stub file");
        assert!(is_fetched(&models_dir, hush));
        assert!(is_fetched(&models_dir, wideband));
        // Fetching the composite touches nothing when the dependency is
        // present (no network access needed for this assertion).
        let mut lines = Vec::new();
        fetch_model(&models_dir, wideband, |line| lines.push(line.to_owned()))
            .expect("nothing to download");
        assert_eq!(
            lines,
            [format!("hush/{}: already present", hush.files[0].name)]
        );
        assert!(!model_dir(&models_dir, wideband).exists());
        std::fs::remove_dir_all(&models_dir).expect("cleanup");
    }
}
