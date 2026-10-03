//! The one base-URL rule for the API, the local server and post-processing
//! (research R-8; spec 004 FR-004).

/// A base URL that passed [`check_base_url`]: trimmed, one trailing `/` removed,
/// scheme `http`/`https`, non-empty host.
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
}

/// Trims whitespace and strips one trailing `/`, then requires the result to parse
/// (with the `url` crate, as the HTTP client does) as an absolute `http`/`https`
/// URL with a non-empty host. The text itself is kept as entered otherwise.
pub fn check_base_url(raw: &str) -> Result<NormalizedUrl, UrlError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(UrlError::Empty);
    }
    let text = trimmed.strip_suffix('/').unwrap_or(trimmed);
    let parsed = url::Url::parse(text).map_err(|_| UrlError::Malformed)?;
    let scheme_ok = matches!(parsed.scheme(), "http" | "https");
    let host_ok = parsed.host_str().is_some_and(|host| !host.is_empty());
    if !(scheme_ok && host_ok) {
        return Err(UrlError::Malformed);
    }
    Ok(NormalizedUrl(text.to_string()))
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
}
