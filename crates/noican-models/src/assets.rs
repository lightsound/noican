//! Immutable model manifest and checksum-verifying local cache.

use std::{
    env,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use thiserror::Error;

const MAX_ASSET_BYTES: u64 = 512 * 1024 * 1024;

/// One file required by a model backend.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ModelAsset {
    /// FastEnhancer Tiny 48 kHz waveform graph.
    FastEnhancerTiny,
    /// FastEnhancer Base 48 kHz waveform graph.
    FastEnhancerBase,
    /// FastEnhancer Small 48 kHz waveform graph.
    FastEnhancerSmall,
    /// DPDFNet2 high-resolution 48 kHz graph.
    DpdfNet2HighResolution,
    /// DPDFNet8 high-resolution 48 kHz graph.
    DpdfNet8HighResolution,
    /// UL-UNAS 16 kHz simplified streaming graph.
    UlUnas,
    /// Hush 16 kHz DeepFilterNet model bundle.
    Hush,
    /// Target-speaker extraction graph.
    TseGraph,
    /// External data referenced by the TSE graph.
    TseWeights,
    /// SpeechBrain ECAPA-TDNN embedding graph.
    Ecapa,
    /// SpeechBrain-compatible 80-band filterbank matrix.
    EcapaFilterbank,
}

impl ModelAsset {
    /// Every file that noican can fetch or validate.
    pub const ALL: [Self; 11] = [
        Self::FastEnhancerTiny,
        Self::FastEnhancerBase,
        Self::FastEnhancerSmall,
        Self::DpdfNet2HighResolution,
        Self::DpdfNet8HighResolution,
        Self::UlUnas,
        Self::Hush,
        Self::TseGraph,
        Self::TseWeights,
        Self::Ecapa,
        Self::EcapaFilterbank,
    ];

    /// Immutable download and integrity metadata for this file.
    #[must_use]
    pub const fn specification(self) -> AssetSpecification {
        match self {
            Self::FastEnhancerTiny => AssetSpecification::verified(
                "fastenhancer/t.onnx",
                "https://github.com/aask1357/fastenhancer/releases/download/onnx-48khz-v1/fastenhancer_t.onnx",
                "1993a3f58ae95d959123b3d28779c2800e0f016d6f4e1177f1213144f301b89c",
            ),
            Self::FastEnhancerBase => AssetSpecification::verified(
                "fastenhancer/b.onnx",
                "https://github.com/aask1357/fastenhancer/releases/download/onnx-48khz-v1/fastenhancer_b.onnx",
                "70e23bba3d41e80d30ebc5eba39d9df64f0e0315f31c772022bb17576c4d96bf",
            ),
            Self::FastEnhancerSmall => AssetSpecification::verified(
                "fastenhancer/s.onnx",
                "https://github.com/aask1357/fastenhancer/releases/download/onnx-48khz-v1/fastenhancer_s.onnx",
                "f04ece2beed330da367264c54cedded62f65a117fbde5c005d3a88fc796d0ba3",
            ),
            Self::DpdfNet2HighResolution => AssetSpecification::verified(
                "dpdfnet/dpdfnet2_48khz_hr.onnx",
                "https://huggingface.co/Ceva-IP/DPDFNet/resolve/dd6818d00f50c836fed43a6243ebe49116de5964/onnx/dpdfnet2_48khz_hr.onnx",
                "7f0575a5cec0ba4ffd8f8bd657e06d007e4ccdd955d76faab922b9d3291dc14b",
            ),
            Self::DpdfNet8HighResolution => AssetSpecification::verified(
                "dpdfnet/dpdfnet8_48khz_hr.onnx",
                "https://huggingface.co/Ceva-IP/DPDFNet/resolve/dd6818d00f50c836fed43a6243ebe49116de5964/onnx/dpdfnet8_48khz_hr.onnx",
                "7b3afbb260a08fe9af3d16e3bda992971be1e7e951d1dee7c2d235f5c43f5631",
            ),
            Self::UlUnas => AssetSpecification::verified(
                "ul-unas/ulunas_stream_simple.onnx",
                "https://raw.githubusercontent.com/Xiaobin-Rong/ul-unas/00f7c700da43d38347f30a6ccebd86fcbc798e07/ulunas_onnx/onnx_models/ulunas_stream_simple.onnx",
                "f2e804d54d6a88f4f82f44d86c9f1cf646db2509bfca935cfbfc5fcd8cbfac3b",
            ),
            Self::Hush => AssetSpecification::verified(
                "hush/advanced_dfnet16k_model_best_onnx.tar.gz",
                "https://huggingface.co/weya-ai/hush/resolve/40812c28145510d8a4b14641bb58c879a7a7b4fe/onnx/advanced_dfnet16k_model_best_onnx.tar.gz",
                "45632ccaa82b71bb743d6caa7c78e983fe2f2790a3af7f6ec48e6ed7ba085df6",
            ),
            Self::TseGraph => AssetSpecification::authenticated(
                "tse/tse_prod_48k.onnx",
                "https://huggingface.co/penta2himajin/tse-conv-tasnet-48k/resolve/main/tse_prod_48k.onnx",
                "NOICAN_TSE_ONNX_SHA256",
            ),
            Self::TseWeights => AssetSpecification::authenticated(
                "tse/tse_prod_48k.onnx.data",
                "https://huggingface.co/penta2himajin/tse-conv-tasnet-48k/resolve/main/tse_prod_48k.onnx.data",
                "NOICAN_TSE_DATA_SHA256",
            ),
            Self::Ecapa => AssetSpecification::verified(
                "ecapa/ecapa-speaker-v1.onnx",
                "https://huggingface.co/vedk00/ecapa-voxceleb-speaker-embedding-onnx/resolve/a9cb9321b07b4ee5b0ea47fdd25242d9cacd824a/model/ecapa-speaker-v1.onnx",
                "f46380bbaeddb929fb3a10ab63a4b1877a50e3d1e5fdd55a1b618d5651d3f64e",
            ),
            Self::EcapaFilterbank => AssetSpecification::verified(
                "ecapa/fbank-80x201-f32.bin",
                "https://huggingface.co/vedk00/ecapa-voxceleb-speaker-embedding-onnx/resolve/a9cb9321b07b4ee5b0ea47fdd25242d9cacd824a/model/fbank-80x201-f32.bin",
                "024e5073b7cfedee84408dc68dd6bafa02808fc786e67f1314e9c918297f5a63",
            ),
        }
    }
}

/// Download metadata for a model file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssetSpecification {
    /// Path under the noican model cache.
    pub relative_path: &'static str,
    /// Immutable upstream URL when the publisher exposes revisions.
    pub url: &'static str,
    expected_hash: ExpectedHash,
}

impl AssetSpecification {
    const fn verified(
        relative_path: &'static str,
        url: &'static str,
        sha256: &'static str,
    ) -> Self {
        Self {
            relative_path,
            url,
            expected_hash: ExpectedHash::Fixed(sha256),
        }
    }

    const fn authenticated(
        relative_path: &'static str,
        url: &'static str,
        hash_environment_variable: &'static str,
    ) -> Self {
        Self {
            relative_path,
            url,
            expected_hash: ExpectedHash::Environment(hash_environment_variable),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExpectedHash {
    Fixed(&'static str),
    Environment(&'static str),
}

/// Options controlling an asset fetch.
#[derive(Clone, Copy, Debug, Default)]
pub struct FetchOptions<'a> {
    /// Optional Hugging Face bearer token for a gated repository.
    pub hugging_face_token: Option<&'a str>,
}

/// Verified on-disk model cache.
#[derive(Clone, Debug)]
pub struct ModelStore {
    root: PathBuf,
}

impl ModelStore {
    /// Use an explicit model cache root.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Use the platform cache directory (`$XDG_CACHE_HOME/noican/models` on Linux).
    ///
    /// # Errors
    ///
    /// Returns [`AssetError::CacheDirectoryUnavailable`] if the platform has no
    /// configured cache directory.
    pub fn platform_default() -> Result<Self, AssetError> {
        let root = dirs::cache_dir()
            .ok_or(AssetError::CacheDirectoryUnavailable)?
            .join("noican")
            .join("models");
        Ok(Self::new(root))
    }

    /// Root directory of this cache.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Expected local path for an asset.
    #[must_use]
    pub fn path(&self, asset: ModelAsset) -> PathBuf {
        self.root.join(asset.specification().relative_path)
    }

    /// Verify a cached file or download it atomically.
    ///
    /// # Errors
    ///
    /// Returns an I/O, HTTP, size, configuration, or checksum error. A partial
    /// download is never promoted into the cache.
    pub fn ensure(
        &self,
        asset: ModelAsset,
        options: FetchOptions<'_>,
    ) -> Result<PathBuf, AssetError> {
        let specification = asset.specification();
        let expected = expected_hash(specification.expected_hash)?;
        let destination = self.path(asset);
        if destination.is_file() {
            if sha256_file(&destination)? == expected {
                return Ok(destination);
            }
            fs::remove_file(&destination).map_err(|source| AssetError::Io {
                path: destination.clone(),
                source,
            })?;
        }
        let parent = destination
            .parent()
            .ok_or_else(|| AssetError::InvalidPath(destination.clone()))?;
        fs::create_dir_all(parent).map_err(|source| AssetError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
        let partial = destination.with_extension("partial");
        let result = self.download(asset, specification, &partial, options.hugging_face_token);
        if let Err(error) = result {
            let _ignored = fs::remove_file(&partial);
            return Err(error);
        }
        let actual = sha256_file(&partial)?;
        if actual != expected {
            let _ignored = fs::remove_file(&partial);
            return Err(AssetError::Checksum {
                asset,
                expected,
                actual,
            });
        }
        fs::rename(&partial, &destination).map_err(|source| AssetError::Io {
            path: destination.clone(),
            source,
        })?;
        Ok(destination)
    }

    fn download(
        &self,
        asset: ModelAsset,
        specification: AssetSpecification,
        partial: &Path,
        token: Option<&str>,
    ) -> Result<(), AssetError> {
        let mut request = ureq::get(specification.url).header("User-Agent", "noican/0.1");
        if let Some(token) = token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        let response = request.call().map_err(|source| AssetError::Http {
            asset,
            message: source.to_string(),
        })?;
        let (_, body) = response.into_parts();
        let mut reader = body.into_reader().take(MAX_ASSET_BYTES + 1);
        let mut file = File::create(partial).map_err(|source| AssetError::Io {
            path: partial.to_path_buf(),
            source,
        })?;
        let copied = io::copy(&mut reader, &mut file).map_err(|source| AssetError::Io {
            path: partial.to_path_buf(),
            source,
        })?;
        if copied > MAX_ASSET_BYTES {
            return Err(AssetError::TooLarge {
                asset,
                limit: MAX_ASSET_BYTES,
            });
        }
        file.sync_all().map_err(|source| AssetError::Io {
            path: partial.to_path_buf(),
            source,
        })
    }
}

/// Asset cache and download failures.
#[derive(Debug, Error)]
pub enum AssetError {
    /// The operating system did not expose a user cache location.
    #[error("platform cache directory is unavailable")]
    CacheDirectoryUnavailable,
    /// An expected hash must be supplied for a gated mutable URL.
    #[error("set {variable} to the publisher-confirmed SHA-256 before fetching this gated asset")]
    MissingExpectedHash {
        /// Environment variable accepted by the downloader.
        variable: &'static str,
    },
    /// A generated destination has no parent.
    #[error("asset destination has no parent: {}", .0.display())]
    InvalidPath(PathBuf),
    /// Filesystem operation failed.
    #[error("I/O error at {}: {source}", path.display())]
    Io {
        /// Affected path.
        path: PathBuf,
        /// Underlying failure.
        #[source]
        source: io::Error,
    },
    /// Upstream request failed.
    #[error("failed to download {asset:?}: {message}")]
    Http {
        /// Requested asset.
        asset: ModelAsset,
        /// Sanitized transport or status detail.
        message: String,
    },
    /// Upstream sent more data than the configured safety bound.
    #[error("download for {asset:?} exceeded {limit} bytes")]
    TooLarge {
        /// Requested asset.
        asset: ModelAsset,
        /// Maximum accepted byte count.
        limit: u64,
    },
    /// Content did not match the pinned digest.
    #[error("checksum mismatch for {asset:?}: expected {expected}, got {actual}")]
    Checksum {
        /// Affected asset.
        asset: ModelAsset,
        /// Manifest digest.
        expected: String,
        /// Downloaded digest.
        actual: String,
    },
}

fn expected_hash(expected: ExpectedHash) -> Result<String, AssetError> {
    match expected {
        ExpectedHash::Fixed(hash) => Ok(hash.to_owned()),
        ExpectedHash::Environment(variable) => env::var(variable)
            .map(|hash| hash.to_ascii_lowercase())
            .map_err(|_error| AssetError::MissingExpectedHash { variable }),
    }
}

fn sha256_file(path: &Path) -> Result<String, AssetError> {
    let file = File::open(path).map_err(|source| AssetError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    sha256_reader(file).map_err(|source| AssetError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn sha256_reader(mut reader: impl Read) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_paths_are_relative_and_unique() {
        let mut paths = std::collections::HashSet::new();
        for asset in ModelAsset::ALL {
            let specification = asset.specification();
            assert!(!Path::new(specification.relative_path).is_absolute());
            assert!(paths.insert(specification.relative_path));
        }
    }

    #[test]
    fn sha256_matches_known_vector() -> Result<(), io::Error> {
        let actual = sha256_reader("abc".as_bytes())?;
        assert_eq!(
            actual,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        Ok(())
    }

    #[test]
    fn store_preserves_manifest_layout() -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        let store = ModelStore::new(temporary.path());
        assert_eq!(
            store.path(ModelAsset::FastEnhancerTiny),
            temporary.path().join("fastenhancer/t.onnx")
        );
        Ok(())
    }
}
