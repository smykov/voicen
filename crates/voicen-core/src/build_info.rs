//! Version and commit of the running build (FR-18).

use std::fmt;

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildInfo {
    pub version: &'static str,
    pub commit: &'static str,
}

impl fmt::Display for BuildInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.version, self.commit)
    }
}

pub fn build_info() -> BuildInfo {
    BuildInfo {
        version: env!("CARGO_PKG_VERSION"),
        commit: env!("VOICEN_COMMIT"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_package_version_and_a_commit() {
        let info = build_info();
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert!(!info.commit.is_empty());
    }

    #[test]
    fn displays_as_version_and_commit_in_parentheses() {
        let info = BuildInfo {
            version: "1.2.3",
            commit: "abc1234",
        };
        assert_eq!(info.to_string(), "1.2.3 (abc1234)");
    }
}
