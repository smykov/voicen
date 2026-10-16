#[test]
fn message_ids_and_methods_are_not_hosts() {
    // A message id or a method whose name only starts with "invalid" after a dot is no host.
    let id = "failure.invalid_api_key";
    let other = "settings.invalidValue";
    cache.invalidate();
    let n = report.invalid2;
    assert_ne!(id, other);
    assert_eq!(n, 0);
}
