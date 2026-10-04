#[cfg(feature = "test")]
pub fn only_with_feature() {}

#[cfg(not(feature = "test"))]
pub fn without_feature() {}

#[cfg_attr(feature = "test", derive(Debug))]
pub struct Probe;
