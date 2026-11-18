//! T-171 (T-023 analysis (ii), spec 006 T010/T011): the commit that `build.rs`
//! embeds as `VOICEN_COMMIT`, chosen by one pure function.
//!
//! Contract: the trim of the first non-blank of (the `VOICEN_COMMIT` env value, the
//! stdout of a successful `git rev-parse`), else `"unknown"`: never empty, never
//! padded.
//!
//! RED STAGE: `pick_commit` below is a characterization stub of what `build.rs`
//! does today (the env value is filtered on its trim but returned untrimmed; git
//! stdout is trimmed but may stay empty). It is the behaviour to be fixed in the
//! implement stage, which also moves this file into `build.rs`'s use.

/// CURRENT BEHAVIOUR of `build.rs` (to be fixed): see the module doc.
pub fn pick_commit(env: Option<&str>, git: Option<&str>) -> String {
    env.filter(|c| !c.trim().is_empty())
        .map(str::to_string)
        .or_else(|| git.map(|s| s.trim().to_string()))
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::pick_commit;

    #[test]
    fn pick_commit_follows_the_selection_table() {
        let table: [(Option<&str>, Option<&str>, &str); 6] = [
            (Some("abc1234"), None, "abc1234"),
            (Some("  abc1234\n"), None, "abc1234"),
            (Some("  "), Some("def5678\n"), "def5678"),
            (None, Some(""), "unknown"),
            (None, Some(" \n"), "unknown"),
            (None, None, "unknown"),
        ];
        for (env, git, want) in table {
            assert_eq!(pick_commit(env, git), want, "env {env:?}, git {git:?}");
        }
    }

    #[test]
    fn a_set_env_value_wins_over_git() {
        assert_eq!(pick_commit(Some("abc1234"), Some("def5678")), "abc1234");
    }

    #[test]
    fn the_result_is_never_empty_and_never_padded() {
        let inputs = [None, Some(""), Some(" "), Some("\n"), Some(" a1b2c3d \n")];
        for env in inputs {
            for git in inputs {
                let got = pick_commit(env, git);
                assert!(!got.is_empty(), "env {env:?}, git {git:?}");
                assert_eq!(got, got.trim(), "env {env:?}, git {git:?}");
            }
        }
    }
}
