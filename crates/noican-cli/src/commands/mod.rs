//! Subcommand implementations.

pub(crate) mod fetch;
pub(crate) mod latency;
pub(crate) mod list;
pub(crate) mod probe;
pub(crate) mod process;

/// Resolves a model-selection argument to catalog entries.
///
/// An empty selection means every model, which is the useful default for both
/// fetching and comparison.
///
/// # Errors
///
/// Returns an error naming the unknown identifier, and listing the valid ones,
/// rather than silently skipping it.
pub(crate) fn select(
    ids: &[String],
) -> anyhow::Result<Vec<&'static noican_models::ModelDescriptor>> {
    if ids.is_empty() {
        return Ok(noican_models::CATALOG.iter().collect());
    }
    ids.iter()
        .map(|id| {
            noican_models::catalog::find(id).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown model `{id}`. Available: {}",
                    noican_models::catalog::ids().collect::<Vec<_>>().join(", ")
                )
            })
        })
        .collect()
}

/// Units used when rendering a byte count.
const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];

/// Formats a byte count for humans.
#[must_use]
pub(crate) fn human_bytes(bytes: u64) -> String {
    #[expect(
        clippy::cast_precision_loss,
        reason = "the value is only used to render a rounded size for display"
    )]
    let mut scaled = bytes as f64;
    let mut unit = 0;
    while scaled >= 1024.0 && unit + 1 < UNITS.len() {
        scaled /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{scaled:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::{human_bytes, select};

    #[test]
    fn empty_selection_means_everything() {
        assert_eq!(select(&[]).unwrap().len(), noican_models::CATALOG.len());
    }

    #[test]
    fn named_selection_preserves_order() {
        let selected = select(&["gtcrn".to_owned(), "fastenhancer-t".to_owned()]).unwrap();
        assert_eq!(selected[0].id, "gtcrn");
        assert_eq!(selected[1].id, "fastenhancer-t");
    }

    #[test]
    fn unknown_selection_lists_the_alternatives() {
        let error = select(&["nope".to_owned()]).unwrap_err().to_string();
        assert!(error.contains("unknown model `nope`"), "{error}");
        assert!(error.contains("fastenhancer-t"), "{error}");
    }

    #[test]
    fn byte_counts_are_readable() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1_536), "1.5 KiB");
        assert_eq!(human_bytes(10_485_760), "10.0 MiB");
    }
}
