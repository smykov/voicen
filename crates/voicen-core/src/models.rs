//! Which built-in whisper.cpp models are on disk (spec 004 T067; contracts/core-traits.md).
//!
//! Defined here, implemented by 002's `ModelStore` (T-016). Model ids are the stable
//! strings stored in `builtin_local.model_id` (decision #23 N2); 002 maps its enum.

pub trait DownloadedModels: Send + Sync {
    fn is_downloaded(&self, id: &str) -> bool;
    fn list(&self) -> Vec<String>;
}

/// A fixed set of downloaded model ids.
#[cfg(any(test, feature = "test-fakes"))]
#[derive(Default)]
pub struct FakeDownloadedModels {
    ids: Vec<String>,
}

#[cfg(any(test, feature = "test-fakes"))]
impl FakeDownloadedModels {
    /// The models in `ids` are downloaded, no other.
    pub fn new(ids: &[&str]) -> FakeDownloadedModels {
        FakeDownloadedModels {
            ids: ids.iter().map(|id| id.to_string()).collect(),
        }
    }
}

#[cfg(any(test, feature = "test-fakes"))]
impl DownloadedModels for FakeDownloadedModels {
    fn is_downloaded(&self, id: &str) -> bool {
        self.ids.iter().any(|known| known == id)
    }

    fn list(&self) -> Vec<String> {
        self.ids.clone()
    }
}
