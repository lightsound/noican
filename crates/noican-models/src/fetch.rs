//! Downloading and verifying model weights.

use std::io::Read as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

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
///
/// Presence is the only thing checked: this runs on hot paths (the
/// picker's "fetched" status, `prepare_stage`'s decision to fetch), so
/// re-hashing every file would be too expensive. Pinned digests are
/// verified by [`fetch_model`] whenever it touches a file, which is
/// where corrupt or wrongly-placed files get repaired.
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
/// first, into their own directories. A present file with a pinned
/// digest is re-verified and re-downloaded when it does not match, so
/// corrupt or wrongly-placed files (including manually placed ones)
/// are repaired here instead of surfacing as a load error later.
/// Calls `progress` with a human-readable line per file event.
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
            // A file without a pinned digest can only be checked for
            // presence; a pinned one is re-verified so corrupt or
            // wrongly-placed files are repaired instead of reported as
            // fetched (is_fetched stays presence-only for hot paths).
            let verified = match file.sha256 {
                Some(expected) => sha256_file(&dest)? == expected,
                None => true,
            };
            if verified {
                progress(&format!("{}/{}: already present", model.id, file.name));
                continue;
            }
            progress(&format!(
                "{}/{}: checksum mismatch, re-downloading",
                model.id, file.name
            ));
        }
        progress(&format!("{}/{}: downloading…", model.id, file.name));
        // Write via a temp file so an interrupted download never looks
        // complete to `is_fetched`; the download streams straight into
        // it, hashing as it goes.
        let tmp = dest.with_extension("part");
        let (got, len) = match download_to(file.url, &tmp) {
            Ok(result) => result,
            Err(error) => {
                std::fs::remove_file(&tmp).ok();
                return Err(error);
            }
        };
        if let Some(expected) = file.sha256
            && got != expected
        {
            std::fs::remove_file(&tmp).ok();
            return Err(FetchError::Checksum {
                file: file.name.to_owned(),
                expected: expected.to_owned(),
                got,
            });
        }
        std::fs::rename(&tmp, &dest).map_err(|source| FetchError::Io {
            path: dest.clone(),
            source,
        })?;
        progress(&format!("{}/{}: done ({len} bytes)", model.id, file.name));
    }
    Ok(())
}

/// Returns a Hugging Face access token from the environment
/// (`NOICAN_HF_TOKEN` or `HF_TOKEN`), if any. Optional: every registry
/// entry hosted there is public, but authenticated requests get higher
/// rate limits and can reach gated repos.
fn hf_token() -> Option<String> {
    std::env::var("NOICAN_HF_TOKEN")
        .or_else(|_| std::env::var("HF_TOKEN"))
        .ok()
        .filter(|t| !t.is_empty())
}

/// The fetcher agent, configured once: hard bounds on every phase of a
/// download so a stalled or trickling server can never hang a fetch (or
/// the engine start it blocks) forever. Connect and header waits stay
/// short; the body deadline is generous — the largest weights are tens
/// of MB, so 15 minutes covers very slow links — and resets per call,
/// so a redirect chain gets its own allowance per leg.
fn download_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .timeout_recv_body(Some(Duration::from_mins(15)))
            .build()
            .into()
    })
}

/// Streams `url` into `tmp`, hashing as it goes, and returns the digest
/// and byte count. The body never lives in memory: a stopped or corrupt
/// transfer leaves a partial `tmp` for the caller to remove, never a
/// file that looks complete.
fn download_to(url: &str, tmp: &Path) -> Result<(String, u64), FetchError> {
    let wrap = |source: ureq::Error| FetchError::Http {
        url: url.to_owned(),
        source: Box::new(source),
    };
    let mut request = download_agent().get(url);
    if url.starts_with("https://huggingface.co/")
        && let Some(token) = hf_token()
    {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    let mut response = request.call().map_err(wrap)?;
    let mut reader = response.body_mut().as_reader();
    let mut writer = std::fs::File::create(tmp).map_err(|source| FetchError::Io {
        path: tmp.to_owned(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut len = 0_u64;
    let mut chunk = vec![0_u8; 1 << 16].into_boxed_slice();
    loop {
        let read = reader
            .read(&mut chunk)
            .map_err(|source| wrap(ureq::Error::Io(source)))?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
        writer
            .write_all(&chunk[..read])
            .map_err(|source| FetchError::Io {
                path: tmp.to_owned(),
                source,
            })?;
        len += read as u64;
    }
    writer.flush().map_err(|source| FetchError::Io {
        path: tmp.to_owned(),
        source,
    })?;
    Ok((hex_digest(hasher), len))
}

/// SHA-256 of a file already on disk, read in chunks so the whole file
/// never lives in memory.
fn sha256_file(path: &Path) -> Result<String, FetchError> {
    let mut reader = std::fs::File::open(path).map_err(|source| FetchError::Io {
        path: path.to_owned(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut chunk = vec![0_u8; 1 << 16].into_boxed_slice();
    loop {
        let read = reader.read(&mut chunk).map_err(|source| FetchError::Io {
            path: path.to_owned(),
            source,
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
    }
    Ok(hex_digest(hasher))
}

fn hex_digest(hasher: Sha256) -> String {
    use std::fmt::Write as _;
    let digest = hasher.finalize();
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
        let mut hasher = Sha256::new();
        hasher.update(b"abc");
        assert_eq!(
            hex_digest(hasher),
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
        std::fs::remove_dir_all(&models_dir).expect("cleanup");
    }

    /// A spec whose file needs no network: the pinned digest is the
    /// SHA-256 of `b"correct weights"`, and the URL is unroutable, so
    /// any real download attempt fails instantly and offline.
    const VERIFY_SPEC: ModelSpec = ModelSpec {
        id: "verify-test",
        display_name: "verify-test",
        family: crate::manifest::ModelFamily::Denoise,
        sample_rate: 48_000,
        license: "test",
        files: &[crate::manifest::FileSpec {
            name: "weights.bin",
            url: "http://127.0.0.1:1/unreachable",
            sha256: Some("7afc240a360b1f66b2da6dbe941071513fd89c0f4d5e2961231c10c3c4b054ea"),
        }],
        depends_on: &[],
    };

    /// A present file whose bytes match the pinned digest is reported
    /// as present with no download attempt; one whose bytes do not is
    /// detected and re-downloaded (surfacing as a fetch error here
    /// because the URL is unroutable — the progress lines are what
    /// prove the mismatch was caught).
    #[test]
    fn fetch_verifies_present_files_against_the_pinned_digest() {
        let models_dir = std::env::temp_dir().join(format!(
            "noican-fetch-test-{}-{}",
            std::process::id(),
            line!()
        ));
        let dir = model_dir(&models_dir, &VERIFY_SPEC);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let dest = dir.join("weights.bin");

        std::fs::write(&dest, b"correct weights").expect("write");
        let mut lines = Vec::new();
        fetch_model(&models_dir, &VERIFY_SPEC, |line| {
            lines.push(line.to_owned());
        })
        .expect("a matching digest needs no download");
        assert_eq!(lines, ["verify-test/weights.bin: already present"]);

        std::fs::write(&dest, b"wrong bytes").expect("write");
        lines.clear();
        let error = fetch_model(&models_dir, &VERIFY_SPEC, |line| {
            lines.push(line.to_owned());
        })
        .expect_err("the unroutable URL must fail after detection");
        assert!(matches!(error, FetchError::Http { .. }));
        assert_eq!(
            lines,
            [
                "verify-test/weights.bin: checksum mismatch, re-downloading",
                "verify-test/weights.bin: downloading…",
            ]
        );
        // The stale destination was left untouched and no temp remains.
        assert_eq!(std::fs::read(&dest).expect("read"), b"wrong bytes");
        assert!(!dest.with_extension("part").exists());
        std::fs::remove_dir_all(&models_dir).expect("cleanup");
    }
}
