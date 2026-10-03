//! Which built-in whisper.cpp models are on disk (spec 004 T067; contracts/core-traits.md).
//!
//! Defined here, implemented by 002's `ModelStore` (T-016). Model ids are the stable
//! strings stored in `builtin_local.model_id` (decision #23 N2); 002 maps its enum.
//!
//! STUB (T-003 red tests): every body is `todo!()`; the developer implements them.

pub trait DownloadedModels: Send + Sync {
    fn is_downloaded(&self, id: &str) -> bool;
    fn list(&self) -> Vec<String>;
}

/// A fixed set of downloaded model ids.
#[cfg(any(test, feature = "test-fakes"))]
#[derive(Default)]
pub struct FakeDownloadedModels {
    // STUB: the developer chooses the state.
    _state: (),
}

#[cfg(any(test, feature = "test-fakes"))]
#[allow(unused_variables)] // STUB: bodies are todo!()
impl FakeDownloadedModels {
    /// The models in `ids` are downloaded, no other.
    pub fn new(ids: &[&str]) -> FakeDownloadedModels {
        todo!("T-003: FakeDownloadedModels::new")
    }
}

#[cfg(any(test, feature = "test-fakes"))]
#[allow(unused_variables)] // STUB: bodies are todo!()
impl DownloadedModels for FakeDownloadedModels {
    fn is_downloaded(&self, id: &str) -> bool {
        todo!("T-003: FakeDownloadedModels::is_downloaded")
    }

    fn list(&self) -> Vec<String> {
        todo!("T-003: FakeDownloadedModels::list")
    }
}
