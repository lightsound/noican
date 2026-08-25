//! The enrolled speaker profile: one embedding, on disk, in a readable format.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Dimension of the embedding the catalogued model produces.
pub const EMBEDDING_DIMENSION: usize = 192;

/// File name a profile takes inside the model store.
pub const PROFILE_FILE_NAME: &str = "speaker-profile.txt";

/// An enrolled speaker: a unit-length mean embedding plus how it was made.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerProfile {
    /// Unit-length mean of the enrolment embeddings.
    pub embedding: Vec<f32>,
    /// Model the embedding came from. An embedding from one model is
    /// meaningless to another, so comparing across models has to be refused
    /// rather than silently produce noise.
    pub model_id: String,
    /// How many windows were averaged.
    pub windows: usize,
}

impl SpeakerProfile {
    /// Builds a profile by averaging `embeddings` and normalising the result.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Enrolment`] if there are no embeddings, if they
    /// disagree about dimension, or if they cancel out to nothing.
    pub fn from_embeddings(model_id: &str, embeddings: &[Vec<f32>]) -> Result<Self> {
        let Some(first) = embeddings.first() else {
            return Err(Error::Enrolment {
                detail: "no speech windows were long enough to embed".to_owned(),
            });
        };
        if embeddings.iter().any(|one| one.len() != first.len()) {
            return Err(Error::Enrolment {
                detail: "the embeddings disagree about their dimension".to_owned(),
            });
        }

        let mut mean = vec![0.0f32; first.len()];
        for embedding in embeddings {
            for (slot, value) in mean.iter_mut().zip(embedding) {
                *slot += *value;
            }
        }
        normalise(&mut mean).ok_or_else(|| Error::Enrolment {
            detail: "the enrolment embeddings averaged to zero".to_owned(),
        })?;

        Ok(Self {
            embedding: mean,
            model_id: model_id.to_owned(),
            windows: embeddings.len(),
        })
    }

    /// Cosine similarity against a candidate embedding.
    ///
    /// The profile is already unit length, so this is a dot product once the
    /// candidate is normalised. Returns 0 for a zero-length candidate, which is
    /// what silence produces.
    #[must_use]
    pub fn similarity(&self, candidate: &[f32]) -> f32 {
        let norm = candidate
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt();
        if norm <= 0.0 || candidate.len() != self.embedding.len() {
            return 0.0;
        }
        let dot: f32 = self
            .embedding
            .iter()
            .zip(candidate)
            .map(|(a, b)| a * b)
            .sum();
        dot / norm
    }

    /// Where a profile lives inside a model store rooted at `root`.
    #[must_use]
    pub fn default_path(root: &Path) -> PathBuf {
        root.join(PROFILE_FILE_NAME)
    }

    /// Writes the profile as text.
    ///
    /// Plain text rather than a binary blob or a serialisation dependency: it is
    /// one header line and one number per line, which stays inspectable and
    /// diffable, and this is the only thing in the tree that needs a format.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the file cannot be written.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| Error::Io {
                operation: "create directory",
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let mut text = format!(
            "noican-speaker-profile 1\nmodel {}\nwindows {}\n",
            self.model_id, self.windows
        );
        for value in &self.embedding {
            let _ = writeln!(text, "{value:.9}");
        }
        std::fs::write(path, text).map_err(|source| Error::Io {
            operation: "write",
            path: path.to_path_buf(),
            source,
        })
    }

    /// Reads a profile written by [`Self::save`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the file cannot be read and
    /// [`Error::Enrolment`] if its contents are not a profile.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
            operation: "read",
            path: path.to_path_buf(),
            source,
        })?;
        let malformed = |detail: &str| Error::Enrolment {
            detail: format!("{}: {detail}", path.display()),
        };

        let mut lines = text.lines();
        if lines.next() != Some("noican-speaker-profile 1") {
            return Err(malformed("not a noican speaker profile"));
        }
        let model_id = lines
            .next()
            .and_then(|line| line.strip_prefix("model "))
            .ok_or_else(|| malformed("no model line"))?
            .to_owned();
        let windows = lines
            .next()
            .and_then(|line| line.strip_prefix("windows "))
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| malformed("no window count"))?;

        let embedding = lines
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.trim().parse::<f32>())
            .collect::<std::result::Result<Vec<f32>, _>>()
            .map_err(|_| malformed("an embedding value is not a number"))?;
        if embedding.is_empty() {
            return Err(malformed("the embedding is empty"));
        }

        Ok(Self {
            embedding,
            model_id,
            windows,
        })
    }
}

/// Scales `values` to unit length, returning `None` if they are all zero.
pub fn normalise(values: &mut [f32]) -> Option<()> {
    let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm <= 0.0 || !norm.is_finite() {
        return None;
    }
    for value in values.iter_mut() {
        *value /= norm;
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::{SpeakerProfile, normalise};

    #[test]
    fn averaging_produces_a_unit_length_profile() {
        let profile =
            SpeakerProfile::from_embeddings("m", &[vec![3.0, 0.0], vec![0.0, 4.0]]).unwrap();
        let norm = profile
            .embedding
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt();
        assert!((norm - 1.0).abs() < 1e-6, "profile has norm {norm}");
        assert_eq!(profile.windows, 2);
    }

    #[test]
    fn an_identical_embedding_scores_one() {
        let profile = SpeakerProfile::from_embeddings("m", &[vec![1.0, 2.0, 3.0]]).unwrap();
        let similarity = profile.similarity(&[2.0, 4.0, 6.0]);
        assert!((similarity - 1.0).abs() < 1e-5, "got {similarity}");
    }

    #[test]
    fn an_orthogonal_embedding_scores_zero() {
        let profile = SpeakerProfile::from_embeddings("m", &[vec![1.0, 0.0]]).unwrap();
        assert!(profile.similarity(&[0.0, 1.0]).abs() < 1e-6);
    }

    /// Silence embeds to nothing, and the gate must read that as "not a match"
    /// rather than dividing by zero.
    #[test]
    fn silence_and_mismatched_dimensions_score_zero() {
        let profile = SpeakerProfile::from_embeddings("m", &[vec![1.0, 0.0]]).unwrap();
        assert!(profile.similarity(&[0.0, 0.0]).abs() < f32::EPSILON);
        assert!(profile.similarity(&[1.0, 0.0, 0.0]).abs() < f32::EPSILON);
    }

    #[test]
    fn enrolment_refuses_degenerate_input() {
        assert!(SpeakerProfile::from_embeddings("m", &[]).is_err());
        assert!(SpeakerProfile::from_embeddings("m", &[vec![0.0, 0.0]]).is_err());
        assert!(
            SpeakerProfile::from_embeddings("m", &[vec![1.0], vec![1.0, 2.0]]).is_err(),
            "mismatched dimensions were accepted"
        );
    }

    #[test]
    fn a_profile_survives_a_round_trip() {
        let directory = std::env::temp_dir().join(format!("noican-profile-{}", std::process::id()));
        let path = directory.join("profile.txt");
        let profile = SpeakerProfile::from_embeddings("ecapa", &[vec![0.5, -0.25, 0.125]]).unwrap();
        profile.save(&path).unwrap();

        let loaded = SpeakerProfile::load(&path).unwrap();
        assert_eq!(loaded.model_id, "ecapa");
        assert_eq!(loaded.windows, 1);
        for (after, before) in loaded.embedding.iter().zip(&profile.embedding) {
            assert!((after - before).abs() < 1e-6);
        }
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn loading_rejects_a_file_that_is_not_a_profile() {
        let directory = std::env::temp_dir().join(format!("noican-bad-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("nope.txt");
        std::fs::write(&path, "hello\n").unwrap();
        assert!(SpeakerProfile::load(&path).is_err());
        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn normalising_zero_fails_rather_than_producing_nan() {
        let mut values = vec![0.0, 0.0];
        assert!(normalise(&mut values).is_none());
        let mut values = vec![3.0, 4.0];
        assert!(normalise(&mut values).is_some());
        assert!((values[0] - 0.6).abs() < 1e-6);
    }
}
