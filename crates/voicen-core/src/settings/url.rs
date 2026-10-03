//! The one base-URL rule for the API, the local server and post-processing
//! (research R-8; spec 004 FR-004).
//!
//! STUB (T-003 red tests): every body is `todo!()`; the developer implements them.

/// A base URL that passed [`check_base_url`]: trimmed, one trailing `/` removed,
/// scheme `http`/`https`, non-empty host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedUrl(
    // STUB: the developer chooses the representation.
    #[allow(dead_code)] String,
);

impl NormalizedUrl {
    /// The normalized text, as stored in the settings.
    pub fn as_str(&self) -> &str {
        todo!("T-003: NormalizedUrl::as_str")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlError {
    /// Nothing but whitespace (field code `required`).
    Empty,
    /// Not an absolute `http`/`https` URL with a host (field code `url.malformed`).
    Malformed,
}

#[allow(unused_variables)] // STUB: body is todo!()
pub fn check_base_url(raw: &str) -> Result<NormalizedUrl, UrlError> {
    todo!("T-003: check_base_url")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_base_url_table() {
        // Accepted, with the normalized text (spec T029). Bite: no trim, no
        // trailing-slash strip, or stripping more than one `/`.
        let accepted = [
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
        // Property: normalizing a normalized URL changes nothing (save then reload
        // yields the same text).
        for raw in [
            " https://api.openai.com/v1/ ",
            "http://localhost:8000/v1",
            "https://api.example.com/",
        ] {
            let once = check_base_url(raw).expect("accepted");
            let twice = check_base_url(once.as_str()).expect("still accepted");
            assert_eq!(once, twice, "{raw:?}");
        }
    }
}
