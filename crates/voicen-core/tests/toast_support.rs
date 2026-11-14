//! T-210 core helpers the shell's toast notifier (and T-213's activator) stand on:
//! the one `retry:<id>` codec of `PendingId` and the one `io::Error` -> logs reason map.
//!
//! Pinned API (docs/tasks/T-210.md ## Tests): `PendingId::to_toast_arg(self) -> String`,
//! `PendingId::from_toast_arg(&str) -> Option<PendingId>` (strict decimal `u64` after
//! the literal prefix `retry:`), `LogsUnwritableReason::of(&Path, &io::Error)`.

use std::io;

use voicen_core::diag::LogsUnwritableReason;
use voicen_core::platform::PendingId;
use voicen_core::test_support::TempDir;

fn parse(arg: &str) -> Option<PendingId> {
    PendingId::from_toast_arg(arg)
}

#[test]
fn a_pending_id_round_trips_through_its_toast_argument() {
    // The argument is `retry:<u64>` and nothing else (FR-20, NFR-04). Bite: another
    // prefix or separator, the number in hex or padded, a lossy cast, `to_` and `from_`
    // that disagree.
    for n in [0_u64, 1, 7, 4_294_967_296, u64::MAX] {
        let id = parse(&format!("retry:{n}")).unwrap_or_else(|| panic!("retry:{n} must parse"));
        assert_eq!(id.to_toast_arg(), format!("retry:{n}"), "write of {n}");
        assert_eq!(parse(&id.to_toast_arg()), Some(id), "round trip of {n}");
    }
    assert_ne!(
        parse("retry:1"),
        parse("retry:2"),
        "different numbers are different ids"
    );
}

#[test]
fn every_other_string_parses_to_none() {
    // The invariant is "any other string is None": strict decimal u64, the exact
    // prefix. `+1` is the one `u64::from_str` would accept. Bite: `strip_prefix` then
    // `parse` unguarded (accepts `+1`), a `trim`, a case-insensitive prefix, a prefix
    // match only (`retry:1x` or `retry:1:2`), an empty number read as 0, a wrapped
    // overflow.
    for bad in [
        "retry:",
        "retry:-1",
        "retry:+1",
        "retry:1x",
        "retry:18446744073709551616",
        "retry:99999999999999999999999999",
        "other:1",
        "",
        "retry",
        "retry:1:2",
        "retry: 1",
        "retry:1 ",
        " retry:1",
        "Retry:1",
        "retry:0x1",
        "retry:1.0",
        "retry:١",
        "retry\u{0}:1",
    ] {
        assert_eq!(parse(bad), None, "{bad:?} must not parse");
    }
}

fn dir() -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new();
    let dir = tmp.path().join("logs");
    (tmp, dir)
}

#[test]
fn the_logs_reason_follows_the_error_kind() {
    // The map the log's private `unwritable` had (diag/log.rs), now public and the only
    // one (P-010): PermissionDenied, StorageFull (and QuotaExceeded) -> DiskFull,
    // NotADirectory, anything else -> Other. Bite: a kind mapped to another reason,
    // StorageFull falling into Other, a second map in the shell that drifts.
    let (_tmp, dir) = dir();
    let of = |kind: io::ErrorKind| LogsUnwritableReason::of(&dir, &io::Error::new(kind, "x"));
    assert_eq!(
        of(io::ErrorKind::PermissionDenied),
        LogsUnwritableReason::PermissionDenied
    );
    assert_eq!(
        of(io::ErrorKind::StorageFull),
        LogsUnwritableReason::DiskFull
    );
    assert_eq!(
        of(io::ErrorKind::QuotaExceeded),
        LogsUnwritableReason::DiskFull
    );
    assert_eq!(
        of(io::ErrorKind::NotADirectory),
        LogsUnwritableReason::NotADirectory
    );
    assert_eq!(
        of(io::ErrorKind::TimedOut),
        LogsUnwritableReason::Other(None)
    );
}

#[cfg(unix)]
#[test]
fn an_unmapped_error_keeps_its_raw_os_code_in_other() {
    // EIO (5) maps to no kind of the map: `Other(Some(5))`; the shell never shows the
    // code (every Other is `logs_reason.other`). Bite: the code dropped (`Other(None)`).
    let (_tmp, dir) = dir();
    let e = io::Error::from_raw_os_error(5);
    assert_eq!(
        LogsUnwritableReason::of(&dir, &e),
        LogsUnwritableReason::Other(Some(5))
    );
}

#[test]
fn a_path_through_a_regular_file_is_not_a_directory_whatever_the_error_says() {
    // The rule in the existing map: the nearest existing part of the path is a file ->
    // NotADirectory, even for an error that says PermissionDenied. Bite: the check of
    // the path dropped (only the kind is read).
    let tmp = TempDir::new();
    let file = tmp.path().join("plain");
    std::fs::write(&file, b"x").expect("plant a file");
    let under = file.join("logs");
    let denied = io::Error::new(io::ErrorKind::PermissionDenied, "x");
    assert_eq!(
        LogsUnwritableReason::of(&under, &denied),
        LogsUnwritableReason::NotADirectory
    );
}
