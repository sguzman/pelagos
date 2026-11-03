//! Minimal private HLS conditional-GET validator matcher shared by the
//! reload-driven playlist and immutable retained previews.
use axum::http::{HeaderMap, header::IF_NONE_MATCH};

/// For GET, If-None-Match uses weak comparison. Our validators contain
/// ASCII letters, digits, hyphens and quotes, never a comma. A matching
/// value is NOT authorization; all callers must recheck permissions first.
pub(super) fn matches_if_none_match(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get(IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|values| {
            values
                .split(',')
                .map(str::trim)
                .any(|value| value == "*" || value.strip_prefix("W/").unwrap_or(value) == expected)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn conditional_etags_match_exact_weak_list_or_wildcard_only() {
        let expected = "\"retained-abc123\"";
        let mut headers = HeaderMap::new();
        assert!(!matches_if_none_match(&headers, expected));
        headers.insert(IF_NONE_MATCH, HeaderValue::from_static("\"different\""));
        assert!(!matches_if_none_match(&headers, expected));
        headers.insert(IF_NONE_MATCH, HeaderValue::from_static(expected));
        assert!(matches_if_none_match(&headers, expected));
        headers.insert(
            IF_NONE_MATCH,
            HeaderValue::from_static("\"different\", W/\"retained-abc123\""),
        );
        assert!(matches_if_none_match(&headers, expected));
        headers.insert(IF_NONE_MATCH, HeaderValue::from_static("*"));
        assert!(matches_if_none_match(&headers, expected));
    }
}
