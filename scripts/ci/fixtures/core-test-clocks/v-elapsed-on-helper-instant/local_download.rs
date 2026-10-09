mod common;

use std::time::Duration;

#[test]
fn cancel_during_steady_transfer_ends_cancelled_quickly() {
    let started = common::timing::now();
    let got = download_and_cancel();
    let quick = started.elapsed() < Duration::from_secs(1);
    assert_eq!(got, Cancelled);
    assert!(quick);
}
