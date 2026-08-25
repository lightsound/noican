//! Parsing the `config.ini` that ships inside a `DeepFilterNet` model bundle.
//!
//! Only the handful of keys the inference path needs are read. Everything else
//! in the file describes how the model was trained and is irrelevant here.

use crate::error::Error;

/// The parameters of one `DeepFilterNet`-family model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DfnConfig {
    /// Sample rate the model runs at, in hertz.
    pub sample_rate: u32,
    /// Transform size.
    pub n_fft: usize,
    /// Hop between frames.
    pub hop: usize,
    /// Number of ERB bands the encoder consumes.
    pub erb_bands: usize,
    /// Number of low bins the deep filter operates on.
    pub df_bins: usize,
    /// Minimum FFT bins per ERB band.
    pub min_bins_per_band: usize,
    /// Time constant of the running feature normalisation, in seconds.
    pub norm_tau: f32,
    /// Length of the deep filter, in frames.
    pub df_order: usize,
    /// Frames of lookahead the convolutional encoder was trained with.
    pub conv_lookahead: usize,
    /// Frames of lookahead the deep filter was trained with.
    pub df_lookahead: usize,
}

impl DfnConfig {
    /// Frames of lookahead the model as a whole uses.
    ///
    /// `DeepFilterNet3` takes the larger of the two declared values; the
    /// reference implementation does the same.
    #[must_use]
    pub const fn lookahead(&self) -> usize {
        if self.conv_lookahead > self.df_lookahead {
            self.conv_lookahead
        } else {
            self.df_lookahead
        }
    }

    /// Scale the reference applies to the spectrum during analysis.
    ///
    /// `2 * hop / n_fft^2`. The models were trained on spectra at this scale, so
    /// it has to be applied going in and undone coming out. At the 50 % overlap
    /// every variant uses it reduces to `1 / n_fft`.
    #[must_use]
    pub fn analysis_scale(&self) -> f32 {
        #[expect(
            clippy::cast_precision_loss,
            reason = "transform sizes are small integers, exact in f32"
        )]
        let (hop, n_fft) = (self.hop as f32, self.n_fft as f32);
        2.0 * hop / (n_fft * n_fft)
    }

    /// Parses the subset of `config.ini` that inference needs.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Metadata`] naming the first key that is absent or
    /// unparseable.
    pub fn parse(model_id: &str, text: &str) -> crate::Result<Self> {
        let entries = flatten(text);
        let get = |key: &str| -> crate::Result<&str> {
            entries
                .iter()
                .find_map(|(name, value)| (*name == key).then_some(*value))
                .ok_or_else(|| Error::Metadata {
                    model: model_id.to_owned(),
                    key: key.to_owned(),
                })
        };
        let number = |key: &str| -> crate::Result<usize> {
            get(key)?.parse().map_err(|_| Error::Metadata {
                model: model_id.to_owned(),
                key: key.to_owned(),
            })
        };
        // Hush omits the lookahead keys that DeepFilterNet3 declares, and a
        // missing lookahead means none.
        let optional = |key: &str| -> usize { number(key).unwrap_or(0) };

        let sample_rate = u32::try_from(number("sr")?).map_err(|_| Error::Metadata {
            model: model_id.to_owned(),
            key: "sr".to_owned(),
        })?;

        Ok(Self {
            sample_rate,
            n_fft: number("fft_size")?,
            hop: number("hop_size")?,
            erb_bands: number("nb_erb")?,
            df_bins: number("nb_df")?,
            min_bins_per_band: optional("min_nb_erb_freqs").max(1),
            norm_tau: get("norm_tau")?.parse().map_err(|_| Error::Metadata {
                model: model_id.to_owned(),
                key: "norm_tau".to_owned(),
            })?,
            df_order: number("df_order")?,
            conv_lookahead: optional("conv_lookahead"),
            df_lookahead: optional("df_lookahead"),
        })
    }
}

/// Collects `key = value` pairs from an INI file, ignoring section headers.
///
/// The keys inference needs are unique across sections in every bundle, so
/// tracking sections would only add a way to get them wrong.
fn flatten(text: &str) -> Vec<(&str, &str)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('[') || line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            Some((key.trim(), value.trim()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::DfnConfig;

    /// The relevant excerpt of `DeepFilterNet3_onnx.tar.gz`'s `config.ini`.
    const DFN3: &str = "
[train]
model = deepfilternet3

[df]
sr = 48000
fft_size = 960
hop_size = 480
nb_erb = 32
nb_df = 96
norm_tau = 1
min_nb_erb_freqs = 2
df_order = 5
df_lookahead = 2

[deepfilternet]
conv_lookahead = 2
conv_ch = 64
";

    /// The relevant excerpt of Hush's `config.ini`.
    const HUSH: &str = "
[df]
sr = 16000
hop_size = 160
fft_size = 320
nb_erb = 32
nb_df = 64
min_nb_erb_freqs = 2
norm_tau = 1.0

[deepfilternet]
conv_ch = 16
conv_lookahead = 0
df_lookahead = 0
df_order = 5
";

    #[test]
    fn parses_deep_filter_net3() {
        let config = DfnConfig::parse("dfn3", DFN3).unwrap();
        assert_eq!(config.sample_rate, 48_000);
        assert_eq!(config.n_fft, 960);
        assert_eq!(config.hop, 480);
        assert_eq!(config.erb_bands, 32);
        assert_eq!(config.df_bins, 96);
        assert_eq!(config.df_order, 5);
        // Both lookaheads are 2, so the model's lookahead is 2.
        assert_eq!(config.lookahead(), 2);
        // 2 * 480 / 960^2 == 1 / 960 at 50 % overlap.
        assert!((config.analysis_scale() - 1.0 / 960.0).abs() < 1e-12);
    }

    #[test]
    fn parses_hush_which_declares_no_lookahead() {
        let config = DfnConfig::parse("hush", HUSH).unwrap();
        assert_eq!(config.sample_rate, 16_000);
        assert_eq!(config.n_fft, 320);
        assert_eq!(config.df_bins, 64);
        assert_eq!(config.lookahead(), 0);
        assert!((config.norm_tau - 1.0).abs() < 1e-6);
    }

    /// The keys are read across sections, and the two bundles put `df_order` in
    /// different ones — `[df]` for `DeepFilterNet3`, `[deepfilternet]` for Hush.
    #[test]
    fn keys_are_found_regardless_of_section() {
        assert_eq!(DfnConfig::parse("dfn3", DFN3).unwrap().df_order, 5);
        assert_eq!(DfnConfig::parse("hush", HUSH).unwrap().df_order, 5);
    }

    #[test]
    fn a_missing_required_key_names_itself() {
        let error = DfnConfig::parse("broken", "[df]\nsr = 48000\n").unwrap_err();
        assert!(error.to_string().contains("fft_size"), "{error}");
    }

    #[test]
    fn a_malformed_value_names_its_key() {
        let text = DFN3.replace("nb_erb = 32", "nb_erb = lots");
        let error = DfnConfig::parse("broken", &text).unwrap_err();
        assert!(error.to_string().contains("nb_erb"), "{error}");
    }
}
