//! The model catalog: five whisper.cpp models at one pinned Hugging Face commit
//! (FR-08, spec 002 data-model "LocalModelSpec", research R-6).
//!
//! [`MODELS`] is the one production table (P-010). `ModelStore` and `Downloader`
//! take a catalog slice, so tests inject an entry served by a mock server; the
//! production table is checked only by the shape test below.

/// A catalog model. Its stable string form ([`ModelId::as_str`]) is what settings
/// (`builtin_local.model_id`), IPC and logs carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelId {
    Tiny,
    Base,
    Small,
    MediumQ5_0,
    LargeV3TurboQ5_0,
}

impl ModelId {
    /// Every model, in catalog order.
    pub const ALL: [ModelId; 5] = [
        ModelId::Tiny,
        ModelId::Base,
        ModelId::Small,
        ModelId::MediumQ5_0,
        ModelId::LargeV3TurboQ5_0,
    ];

    /// `tiny` | `base` | `small` | `medium-q5_0` | `large-v3-turbo-q5_0`.
    pub fn as_str(self) -> &'static str {
        // Skeleton (T-016 red tests): not implemented yet.
        todo!("T-016: ModelId::as_str")
    }

    /// The inverse of [`as_str`](Self::as_str); any other string is `None`.
    pub fn parse(s: &str) -> Option<ModelId> {
        // Skeleton (T-016 red tests): not implemented yet.
        let _ = s;
        todo!("T-016: ModelId::parse")
    }
}

/// One catalog entry. `&'static` strings so the production table is a const; a
/// test builds its entry with leaked strings (the mock server's URL).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogEntry {
    pub id: ModelId,
    /// `ggml-<id>.bin`; never an `.en` variant.
    pub file_name: &'static str,
    /// `https://huggingface.co/ggerganov/whisper.cpp/resolve/<commit>/<file_name>`.
    pub url: &'static str,
    /// Exact size in bytes: progress total, disk check, "downloaded" check.
    pub size_bytes: u64,
    /// Pinned SHA-256, 64 lowercase hex digits; never computed from upstream at
    /// runtime.
    pub sha256: &'static str,
    /// True only for `small`.
    pub recommended: bool,
}

/// The production catalog, in [`ModelId::ALL`] order.
// Skeleton (T-016 red tests): the pinned commit, sizes and SHA-256 values are read
// from the Hugging Face LFS metadata by the implementing task (R-6, decision #49).
pub const MODELS: &[CatalogEntry] = &[];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    const STABLE: [(ModelId, &str); 5] = [
        (ModelId::Tiny, "tiny"),
        (ModelId::Base, "base"),
        (ModelId::Small, "small"),
        (ModelId::MediumQ5_0, "medium-q5_0"),
        (ModelId::LargeV3TurboQ5_0, "large-v3-turbo-q5_0"),
    ];

    const URL_PREFIX: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/";

    fn is_lower_hex(s: &str, len: usize) -> bool {
        s.len() == len
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }

    #[test]
    fn model_ids_have_stable_strings_and_round_trip() {
        // data-model "LocalModelSpec": the stable strings are stored in settings and
        // sent over IPC. Bite: any string changed, two swapped, parse not the inverse,
        // parse accepting an `.en` variant, a display name or another case.
        for (id, s) in STABLE {
            assert_eq!(id.as_str(), s, "{id:?}");
            assert_eq!(ModelId::parse(s), Some(id), "parse({s:?})");
        }
        for other in [
            "",
            "Tiny",
            "small.en",
            "medium",
            "large-v3-turbo",
            "ggml-base.bin",
        ] {
            assert_eq!(ModelId::parse(other), None, "parse({other:?})");
        }
        assert_eq!(
            ModelId::ALL.to_vec(),
            STABLE.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            "ALL is not in catalog order"
        );
    }

    #[test]
    fn production_catalog_has_the_pinned_shape() {
        // FR-08, R-6, data-model validation. Bite: a missing or extra entry, catalog
        // order changed, a duplicate id or file, an `.en` file, a file name not
        // `ggml-<id>.bin`, a hash that is not 64 lowercase hex, a zero size, zero or
        // two recommended or one that is not `small`, a URL off the pinned commit,
        // two commits, a query in a URL.
        assert_eq!(MODELS.len(), 5, "catalog entries");
        assert_eq!(
            MODELS.iter().map(|m| m.id).collect::<Vec<_>>(),
            ModelId::ALL.to_vec(),
            "catalog order"
        );
        let ids: BTreeSet<&str> = MODELS.iter().map(|m| m.id.as_str()).collect();
        let files: BTreeSet<&str> = MODELS.iter().map(|m| m.file_name).collect();
        assert_eq!(ids.len(), 5, "unique ids");
        assert_eq!(files.len(), 5, "unique file names");

        let mut commits = BTreeSet::new();
        for m in MODELS {
            assert_eq!(
                m.file_name,
                format!("ggml-{}.bin", m.id.as_str()),
                "{:?} file name",
                m.id
            );
            assert!(!m.file_name.contains(".en"), "{:?}: .en variant", m.id);
            assert!(
                is_lower_hex(m.sha256, 64),
                "{:?}: sha256 {:?}",
                m.id,
                m.sha256
            );
            assert!(
                m.size_bytes > 1_000_000,
                "{:?}: size {}",
                m.id,
                m.size_bytes
            );
            assert!(!m.url.contains('?'), "{:?}: query in URL", m.id);

            let rest = m
                .url
                .strip_prefix(URL_PREFIX)
                .unwrap_or_else(|| panic!("{:?}: URL {:?} not under {URL_PREFIX}", m.id, m.url));
            let (commit, file) = rest
                .split_once('/')
                .unwrap_or_else(|| panic!("{:?}: URL {:?} has no commit", m.id, m.url));
            assert!(is_lower_hex(commit, 40), "{:?}: commit {commit:?}", m.id);
            assert_eq!(file, m.file_name, "{:?}: URL file", m.id);
            commits.insert(commit);
        }
        assert_eq!(
            commits.len(),
            1,
            "one pinned commit in every URL: {commits:?}"
        );

        let recommended: Vec<ModelId> = MODELS
            .iter()
            .filter(|m| m.recommended)
            .map(|m| m.id)
            .collect();
        assert_eq!(recommended, vec![ModelId::Small], "recommended");
    }
}
