//! The one base-URL rule for the API, the local server and post-processing
//! (research R-8; spec 004 FR-004).

/// A base URL that passed [`check_base_url`]: trimmed, one trailing `/` removed,
/// scheme `http`/`https`, non-empty host, no userinfo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedUrl(String);

impl NormalizedUrl {
    /// The normalized text, as stored in the settings.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlError {
    /// Nothing but whitespace (field code `required`).
    Empty,
    /// Not an absolute `http`/`https` URL with a host (field code `url.malformed`).
    Malformed,
    /// A well-formed URL with userinfo: a non-empty username or password
    /// (`user:pass@`), field code `url.credentials` (decision #27(2)). Key bytes
    /// must not reach the settings file through a URL. A query string is allowed.
    Credentials,
}

/// The one storage form of a base URL, valid or not: whitespace trimmed, then one
/// trailing `/` stripped. The settings service stores every base URL in this form
/// (spec 004 data-model "normalize before store"); [`check_base_url`] checks it.
pub fn normalize_base_url(raw: &str) -> &str {
    let trimmed = raw.trim();
    trimmed.strip_suffix('/').unwrap_or(trimmed)
}

/// Normalizes with [`normalize_base_url`], then requires the result to parse
/// (with the `url` crate, as the HTTP client does) as an absolute `http`/`https`
/// URL with a non-empty host and no userinfo (neither a username nor a password).
/// The text itself is kept as entered otherwise.
pub fn check_base_url(raw: &str) -> Result<NormalizedUrl, UrlError> {
    if raw.trim().is_empty() {
        return Err(UrlError::Empty);
    }
    let text = normalize_base_url(raw);
    let parsed = url::Url::parse(text).map_err(|_| UrlError::Malformed)?;
    let scheme_ok = matches!(parsed.scheme(), "http" | "https");
    let host_ok = parsed.host_str().is_some_and(|host| !host.is_empty());
    if !(scheme_ok && host_ok) {
        return Err(UrlError::Malformed);
    }
    let has_userinfo =
        !parsed.username().is_empty() || parsed.password().is_some_and(|p| !p.is_empty());
    if has_userinfo {
        return Err(UrlError::Credentials);
    }
    Ok(NormalizedUrl(text.to_string()))
}

/// True exactly when `url` would send its traffic (and the API key) unencrypted
/// to another machine: scheme `http` and a `url`-crate host that is not loopback
/// (loopback = `Domain("localhost")`, `Ipv4` in 127.0.0.0/8, `Ipv6` == `::1`;
/// spec 004 FR-020, FR-29, decision #52). `https` is never insecure. Decided on
/// the host the HTTP client connects to, never on the text.
pub fn is_insecure_remote(url: &NormalizedUrl) -> bool {
    // A `NormalizedUrl` is only built by `check_base_url`, which parsed this very
    // text with the same parser; a parse failure cannot happen, and there is then
    // no scheme to call `http`.
    let Ok(parsed) = url::Url::parse(url.as_str()) else {
        return false;
    };
    // The `url` crate lower-cases the scheme and a special URL's domain, and
    // decodes IPv4 shorthand (`127.1`, `0x7f.1`, `2130706433`) into `Ipv4`.
    let loopback = parsed.host().is_some_and(|host| is_loopback_host(&host));
    parsed.scheme() == "http" && !loopback
}

/// The one loopback test (T-120, invariant I4): `Domain("localhost")`, `Ipv4` in
/// 127.0.0.0/8, `Ipv6` == `::1`, over a host parsed by the `url` crate (which
/// lower-cases a special URL's domain and decodes IPv4 shorthand). Used by
/// [`is_insecure_remote`] and [`provider_host`]; `localhost.`, `foo.localhost`,
/// `::ffff:127.0.0.1` and the unspecified addresses are not loopback.
pub fn is_loopback_host(host: &url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(domain) => *domain == "localhost",
        url::Host::Ipv4(ip) => ip.is_loopback(),
        url::Host::Ipv6(ip) => *ip == std::net::Ipv6Addr::LOCALHOST,
    }
}

/// A non-loopback host a base URL sends audio or text to (spec 007 FR-031): the
/// `url` crate's host text, lower case, without port or path. It has no `Display`
/// and its `Debug` does not print the value, so a host never reaches a log line by
/// accident; [`ProviderHost::as_str`] is the one way to read it (the notice and
/// `calls.warned_hosts`).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ProviderHost(String);

impl ProviderHost {
    /// The host text (`api.openai.com`, `192.168.1.5`).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ProviderHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProviderHost(..)")
    }
}

/// The provider-host rule (spec 007 FR-031): `None` for a loopback host
/// ([`is_loopback_host`]), the host otherwise, whatever the scheme.
pub fn provider_host(url: &NormalizedUrl) -> Option<ProviderHost> {
    // As in `is_insecure_remote`: a `NormalizedUrl` always parses.
    let parsed = url::Url::parse(url.as_str()).ok()?;
    let host = parsed.host()?;
    if is_loopback_host(&host) {
        return None;
    }
    parsed.host_str().map(|text| ProviderHost(text.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_base_url_table() {
        // Accepted, with the normalized text (spec T029). Bite: no trim, no
        // trailing-slash strip, or stripping more than one `/` (R-8 strips exactly
        // one: `strip_suffix('/')` -> `trim_end_matches('/')` turns the `//` rows red).
        let accepted = [
            ("http://x//", "http://x/"),
            (
                "https://api.example.com/v1//",
                "https://api.example.com/v1/",
            ),
            // A query string is kept (decision #27(2)): Azure-style endpoints.
            (
                "https://api.example.com/v1?api-version=2024-06-01",
                "https://api.example.com/v1?api-version=2024-06-01",
            ),
            ("http://localhost:8000/v1", "http://localhost:8000/v1"),
            ("https://api.openai.com/v1", "https://api.openai.com/v1"),
            ("  https://api.openai.com/v1  ", "https://api.openai.com/v1"),
            ("https://api.openai.com/v1/", "https://api.openai.com/v1"),
            ("\thttp://localhost:8000/v1/\n", "http://localhost:8000/v1"),
            ("https://api.example.com/", "https://api.example.com"),
            ("http://127.0.0.1:8080/v1", "http://127.0.0.1:8080/v1"),
            ("http://[::1]:8000/v1", "http://[::1]:8000/v1"),
            ("http://192.0.2.10:8000/v1", "http://192.0.2.10:8000/v1"),
        ];
        for (raw, normalized) in accepted {
            match check_base_url(raw) {
                Ok(url) => assert_eq!(url.as_str(), normalized, "{raw:?}"),
                Err(e) => panic!("{raw:?} must be accepted, got {e:?}"),
            }
        }

        // Refused as empty. Bite: whitespace-only treated as a URL.
        for raw in ["", " ", "  \t\n "] {
            assert_eq!(check_base_url(raw), Err(UrlError::Empty), "{raw:?}");
        }

        // Refused as malformed (spec T029 list + no host / wrong scheme / bad host).
        // Bite: no scheme check (htp, ftp, mailto, file) or no host check (https://).
        for raw in [
            "api.openai.com",
            "htp://x",
            "https://",
            "ftp://host",
            "mailto:user@example.com",
            "file:///etc/hosts",
            "https://exa mple.com/v1",
            "//api.example.com/v1",
            "http//api.example.com",
        ] {
            assert_eq!(check_base_url(raw), Err(UrlError::Malformed), "{raw:?}");
        }
    }

    #[test]
    fn normalized_url_is_stable() {
        // Property, for inputs without a double trailing slash: normalizing a
        // normalized URL changes nothing (save then reload yields the same text).
        // Not true for `…//`: R-8 strips one `/` per pass, so `https://x/v1//` ->
        // `https://x/v1/` -> `https://x/v1` (pinned in `check_base_url_table`).
        for raw in [
            " https://api.openai.com/v1/ ",
            "http://localhost:8000/v1",
            "https://api.example.com/",
            "https://api.example.com/v1?api-version=2024-06-01",
        ] {
            let once = check_base_url(raw).expect("accepted");
            let twice = check_base_url(once.as_str()).expect("still accepted");
            assert_eq!(once, twice, "{raw:?}");
        }
    }

    /// A [`NormalizedUrl`] built the one way the service builds it.
    #[track_caller]
    fn normalized(raw: &str) -> NormalizedUrl {
        check_base_url(raw).unwrap_or_else(|e| panic!("{raw:?} must pass check_base_url: {e:?}"))
    }

    /// `http` to a non-loopback host: `is_insecure_remote` is true and the provider
    /// rule names the host. One case list for both rules (T-120, invariant I4), so
    /// they cannot disagree about a host.
    const INSECURE: [&str; 10] = [
        "http://example.com/v1",
        "HTTP://EXAMPLE.COM/v1",
        "http://192.168.1.5:8000/v1",
        "http://192.0.2.10:8000/v1",
        "http://0.0.0.0:8000",
        "http://[::]:8000",
        "http://[::ffff:127.0.0.1]",
        "http://localhost./v1",
        "http://foo.localhost:8000",
        "http://127.0.0.1.nip.io/v1",
    ];
    /// A loopback host under either scheme: `is_insecure_remote` is false and the
    /// provider rule says none.
    const LOOPBACK: [&str; 11] = [
        "http://LOCALHOST:8000/v1",
        "http://localhost",
        "http://LocalHost:8000/v1",
        "http://127.0.0.1",
        "http://127.1.2.3",
        "http://127.255.255.254:8080/v1",
        "http://0x7f.1/v1",
        "http://2130706433:8000",
        "http://[::1]:8000/v1",
        "http://[0:0:0:0:0:0:0:1]",
        "https://localhost:8443/v1",
    ];
    /// `https` to a non-loopback host: not insecure, but a provider host.
    const SECURE_REMOTE: [&str; 6] = [
        "https://example.com",
        "https://example.com/v1",
        "https://192.168.1.5:8000/v1",
        "https://0.0.0.0:8000",
        "https://[::ffff:127.0.0.1]",
        "https://localhost.:8443",
    ];

    #[test]
    fn is_insecure_remote_table() {
        // T-015 / decision #52: http + a non-loopback host of the `url` crate warns;
        // loopback is the spec's closed list (localhost any case, 127.0.0.0/8, ::1)
        // over the parsed host, never over the text.
        // Bite: `scheme == "http"` alone (every loopback row turns red); a text
        // prefix/contains check on "localhost"/"127." (`localhost.`, `foo.localhost`,
        // `127.0.0.1.nip.io` red, and the shorthand IPv4 rows `0x7f.1`/`2130706433`
        // red); `Ipv4Addr::is_loopback` replaced by `== 127.0.0.1` (`127.1.2.3` red);
        // `to_ipv4()`/`is_unspecified` counted as loopback (`[::ffff:127.0.0.1]`,
        // `0.0.0.0`, `[::]` red); https treated like http (`https://example.com` red).
        let mut wrong = Vec::new();
        for (raw, expected) in INSECURE
            .iter()
            .map(|r| (*r, true))
            .chain(LOOPBACK.iter().map(|r| (*r, false)))
            .chain(SECURE_REMOTE.iter().map(|r| (*r, false)))
        {
            let got = is_insecure_remote(&normalized(raw));
            if got != expected {
                wrong.push(format!("{raw:?}: expected {expected}, got {got}"));
            }
        }
        assert!(wrong.is_empty(), "is_insecure_remote wrong for: {wrong:#?}");
    }

    // ---- T-120: the provider-host rule (spec 007 FR-031) -----------------------------

    #[test]
    fn is_loopback_host_is_the_one_loopback_test() {
        // Invariant I4: `localhost` (any case), 127.0.0.0/8 and `::1` only, over the
        // parsed host. Bite: any of the near misses of `is_insecure_remote_table`
        // accepted, a loopback shape refused.
        let mut wrong = Vec::new();
        for (raw, expected) in LOOPBACK
            .iter()
            .map(|r| (*r, true))
            .chain(INSECURE.iter().map(|r| (*r, false)))
            .chain(SECURE_REMOTE.iter().map(|r| (*r, false)))
        {
            let parsed = ::url::Url::parse(raw).unwrap_or_else(|e| panic!("{raw}: {e}"));
            let host = parsed.host().unwrap_or_else(|| panic!("{raw}: no host"));
            let got = is_loopback_host(&host);
            if got != expected {
                wrong.push(format!("{raw:?}: expected {expected}, got {got}"));
            }
        }
        assert!(wrong.is_empty(), "is_loopback_host wrong for: {wrong:#?}");
    }

    #[test]
    fn provider_host_is_none_for_loopback_and_the_host_otherwise() {
        // data-model "warned_hosts", tasks.md G06: `localhost`, `127.0.0.0/8`, `::1` ->
        // none; `api.openai.com`, `192.168.1.5` -> host. The scheme does not matter
        // (the notice is about where the audio text goes, not about encryption), and
        // the same cases drive `is_insecure_remote`. Bite: the rule duplicated with a
        // narrower loopback test (`127.1.2.3`, `LOCALHOST`, `[::1]` come back as hosts),
        // `scheme == https` deciding, the port kept in the host.
        let mut wrong = Vec::new();
        for raw in LOOPBACK {
            if let Some(host) = provider_host(&normalized(raw)) {
                wrong.push(format!(
                    "{raw:?}: expected none, got a host ({})",
                    host.as_str()
                ));
            }
        }
        for raw in INSECURE.iter().chain(SECURE_REMOTE.iter()) {
            if provider_host(&normalized(raw)).is_none() {
                wrong.push(format!("{raw:?}: expected a host, got none"));
            }
        }
        assert!(wrong.is_empty(), "provider_host wrong for: {wrong:#?}");

        // The host text: lower case (the `url` crate's), no port, no path.
        for (raw, host) in [
            ("https://api.openai.com/v1", "api.openai.com"),
            ("https://API.OpenAI.com/v1", "api.openai.com"),
            ("http://192.168.1.5:8000/v1", "192.168.1.5"),
            ("https://LOCALHOST.example/v1", "localhost.example"),
            ("https://api.example.com:8443/v1/", "api.example.com"),
        ] {
            let got = provider_host(&normalized(raw)).unwrap_or_else(|| panic!("{raw}: none"));
            assert_eq!(got.as_str(), host, "{raw}");
        }
        // Two spellings of one host are one host (the notice shows once per host).
        assert_eq!(
            provider_host(&normalized("https://api.example.com/v1")),
            provider_host(&normalized("https://API.EXAMPLE.COM:443/other")),
        );
        assert_ne!(
            provider_host(&normalized("https://api.example.com/v1")),
            provider_host(&normalized("https://api.example.org/v1")),
        );
    }

    #[test]
    fn provider_host_never_prints_the_host() {
        // "A host is never logged": the value has no Display and its Debug (what a
        // `{:?}` in a log or an assertion would print) names no host. Bite: `derive(Debug)`
        // on a tuple struct over the String, an `impl Display`.
        use std::fmt::{Debug, Display};
        use std::marker::PhantomData;
        trait NotDisplay {
            const IS: bool = false;
        }
        impl<T: ?Sized> NotDisplay for T {}
        struct Probe<T: ?Sized>(PhantomData<T>);
        #[allow(dead_code)]
        impl<T: ?Sized + Display> Probe<T> {
            const IS: bool = true;
        }
        // black_box: the probe is a constant by design (it asks the type system);
        // the assertion still runs at test time.
        assert!(
            !std::hint::black_box(Probe::<ProviderHost>::IS),
            "ProviderHost implements Display"
        );
        fn debug_of<T: Debug>(value: &T) -> String {
            format!("{value:?} {value:#?}")
        }
        for raw in [
            "https://secret-host.example.com/v1",
            "http://198.51.100.77:8000/v1",
        ] {
            let host = provider_host(&normalized(raw)).expect("a host");
            let printed = debug_of(&host);
            assert!(!printed.contains("secret-host"), "{printed}");
            assert!(!printed.contains("example.com"), "{printed}");
            assert!(!printed.contains("198.51.100"), "{printed}");
            let printed = debug_of(&Some(host));
            assert!(!printed.contains("secret-host"), "{printed}");
            assert!(!printed.contains("198.51.100"), "{printed}");
        }
    }
}
