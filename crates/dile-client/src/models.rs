//! The two models this product ships against, and where they live on disk.
//!
//! **Two entries, one per tier** (`docs/PROJECT.md` §3, Engine tiers and defaults). The
//! Vulkan tier runs the v1 model the M0 decision layer chose; the CPU tier runs the model
//! the maintainer picked for the rare machine whose GPU probe fails. Both are stock Whisper
//! weights from the same Hugging Face repository, and **no fine-tune ships in v1**.
//!
//! | Tier | File | Size |
//! |---|---|---|
//! | Vulkan (default) | `ggml-large-v3-q5_0.bin` | 1,031 MB |
//! | CPU (fallback) | `ggml-large-v3-turbo-q5_0.bin` | 547 MB |
//!
//! **Why turbo on the CPU tier and not something smaller.** The fallback path is rare, and
//! the maintainer's decision of 2026-09-13 is accuracy over speed on it: turbo is the one
//! quantized whisper that can be waited for on a CPU at all, and — unlike the small models
//! that *are* faster than real time — it still takes an initial prompt, which is how the
//! dictionary reaches the decoder. A tier that cannot take the prompt would be a tier
//! without the product's accuracy feature (§3, and the rejected "CPU default" row).
//!
//! **Pinned by commit sha, never by branch.** `docs/PROJECT.md` §3, Distribution: Hugging
//! Face only, `resolve/<commit-sha>`, sha256 embedded in the app, no mirror in v1. A
//! `resolve/main` URL is a promise about a file somebody else can change; the sha256 below
//! is the promise this application actually checks. Both hashes were read from the Hugging
//! Face API rather than by downloading the files, and the Vulkan-tier one was confirmed
//! against a local copy that predates this package.

use std::path::{Path, PathBuf};

use dile_engine_proto::Device;

/// The commit of `ggerganov/whisper.cpp` every model URL below is pinned to.
///
/// Quoted in the URLs through [`pinned`] rather than concatenated at run time, so that the
/// binary carries the exact strings it will ask for and a reader can grep for them.
pub const REVISION: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";

/// A download URL on the pinned commit.
macro_rules! pinned {
    ($file:literal) => {
        concat!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/",
            "5359861c739e955e79d9a303bcbc70fb988958b1/",
            $file
        )
    };
}

/// One model: what it is called, where it comes from, and what it must hash to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelSpec {
    /// The file name on disk, which is also the file name in the repository.
    pub file: &'static str,
    /// The pinned download URL.
    pub url: &'static str,
    /// The sha256 of the whole file, lower-case hexadecimal.
    pub sha256: &'static str,
    /// The exact size in bytes, as the repository reports it.
    pub size_bytes: u64,
}

impl ModelSpec {
    /// The size in whole megabytes, which is the number the consent dialog shows.
    ///
    /// Megabytes rather than mebibytes: the dialog is read by a person deciding whether to
    /// spend their connection on it, and every download manager they have ever seen counts
    /// the same way Hugging Face's own file listing does.
    #[must_use]
    pub const fn size_mb(&self) -> u64 {
        self.size_bytes / 1_000_000
    }
}

/// The default tier's model: the v1 model of `docs/PROJECT.md` §3.
///
/// Stock Whisper large-v3 `q5_0`, chosen on the 20-sentence command set with the dictionary
/// fed in as the initial prompt: WER 0.261 against 0.379 without it, 30 of 31 technical
/// terms against 17, p95 2.38 s on Vulkan.
pub const VULKAN_TIER: ModelSpec = ModelSpec {
    file: "ggml-large-v3-q5_0.bin",
    url: pinned!("ggml-large-v3-q5_0.bin"),
    sha256: "d75795ecff3f83b5faa89d1900604ad8c780abd5739fae406de19f23ecd98ad1",
    size_bytes: 1_081_140_203,
};

/// The fallback tier's model: Whisper large-v3-turbo `q5_0`.
///
/// Maintainer decision 2026-09-13 (W3-A): accuracy over speed, because a machine that ends
/// up here is a machine whose GPU could not be trusted, not a machine in a hurry. Half the
/// size of the Vulkan tier's model and the fastest whisper row M0 measured — 7× faster than
/// `q5_0` on long audio for +0.008 WER — while still accepting the dictionary prompt.
pub const CPU_TIER: ModelSpec = ModelSpec {
    file: "ggml-large-v3-turbo-q5_0.bin",
    url: pinned!("ggml-large-v3-turbo-q5_0.bin"),
    sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
    size_bytes: 574_041_195,
};

/// The model a tier runs.
#[must_use]
pub const fn for_tier(tier: Device) -> &'static ModelSpec {
    match tier {
        Device::Vulkan => &VULKAN_TIER,
        Device::Cpu => &CPU_TIER,
    }
}

/// The name of the directory models are kept in, under the application's local data.
pub const DIRECTORY: &str = "models";

/// Where models are kept: `<app local data>/models/`.
///
/// Local data rather than roaming: a gigabyte of weights has no business following a user
/// between machines, and the directory is the same one WP2's debug `last.wav` lands beside.
/// The caller names `<app local data>` — the tray gets it from Tauri and the command line
/// from [`crate::paths`], and they are the same directory.
///
/// **`DILE_MODEL_DIR` overrides it in debug builds only.** It exists so that a machine which
/// already holds these weights — a benchmark workspace, say — can be pointed at instead of
/// downloading a second copy of them. A release build does not read it: a released product
/// that took its model path from the environment would be one an attacker could aim.
#[must_use]
pub fn directory_in(local_data: &Path) -> PathBuf {
    #[cfg(debug_assertions)]
    if let Ok(override_dir) = std::env::var("DILE_MODEL_DIR")
        && !override_dir.trim().is_empty()
    {
        return PathBuf::from(override_dir);
    }

    local_data.join(DIRECTORY)
}

#[cfg(test)]
mod tests {
    use super::{CPU_TIER, REVISION, VULKAN_TIER, for_tier};
    use dile_engine_proto::Device;

    #[test]
    fn both_tiers_are_pinned_to_a_commit_and_never_to_a_branch() {
        for spec in [VULKAN_TIER, CPU_TIER] {
            assert!(
                spec.url.contains(REVISION),
                "{} is not pinned to the recorded commit",
                spec.file
            );
            assert!(
                !spec.url.contains("/resolve/main/"),
                "{} points at a branch somebody else can move",
                spec.file
            );
            assert!(
                spec.url.starts_with("https://huggingface.co/"),
                "{} is not on the one host this product downloads from",
                spec.file
            );
            assert!(spec.url.ends_with(spec.file));
        }
    }

    #[test]
    fn every_checksum_is_a_lower_case_sha256() {
        for spec in [VULKAN_TIER, CPU_TIER] {
            assert_eq!(spec.sha256.len(), 64, "{} has no sha256", spec.file);
            assert!(
                spec.sha256
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "{} has a checksum the comparison would miss",
                spec.file
            );
        }
        assert_ne!(VULKAN_TIER.sha256, CPU_TIER.sha256);
    }

    #[test]
    fn the_tiers_do_not_share_a_file_and_the_sizes_are_the_ones_quoted() {
        assert_ne!(VULKAN_TIER.file, CPU_TIER.file);
        assert_eq!(for_tier(Device::Vulkan), &VULKAN_TIER);
        assert_eq!(for_tier(Device::Cpu), &CPU_TIER);

        // The numbers the consent dialog shows a person before they spend their connection.
        assert_eq!(VULKAN_TIER.size_mb(), 1_081);
        assert_eq!(CPU_TIER.size_mb(), 574);
        const {
            assert!(
                CPU_TIER.size_bytes < VULKAN_TIER.size_bytes,
                "the fallback tier must not be the larger download"
            );
        }
    }
}
