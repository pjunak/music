use crate::cleanup::MusicBrainzLookupError;
use music_application::cleanup_enrichment::catalog::{CatalogError, CatalogFailure};

pub(super) fn http_failure(error: &reqwest::Error) -> CatalogFailure {
    if let Some(status) = error.status() {
        CatalogFailure::HttpStatus(status.as_u16())
    } else if error.is_timeout() {
        CatalogFailure::Timeout
    } else {
        CatalogFailure::Transport
    }
}

pub(super) fn musicbrainz_failure(error: MusicBrainzLookupError) -> CatalogError {
    CatalogError::MusicBrainzFailure(match error {
        MusicBrainzLookupError::CoolingDown => CatalogFailure::CoolingDown,
        MusicBrainzLookupError::Http(error) => http_failure(&error),
        MusicBrainzLookupError::Json(_) => CatalogFailure::InvalidJson,
        MusicBrainzLookupError::ResponseTooLarge => CatalogFailure::ResponseTooLarge,
        MusicBrainzLookupError::InvalidScore(_) | MusicBrainzLookupError::InvalidPayload(_) => {
            CatalogFailure::InvalidPayload
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_http_diagnostics_never_include_request_urls()
    -> Result<(), Box<dyn std::error::Error>> {
        let response = reqwest::Response::from(
            axum::http::Response::builder()
                .status(503)
                .body("private provider response")?,
        );
        let error = response
            .error_for_status()
            .err()
            .ok_or("fixture must fail")?
            .with_url(reqwest::Url::parse(
                "https://example.invalid/?api_key=private-key&track=private-title",
            )?);
        let error = musicbrainz_failure(MusicBrainzLookupError::Http(error));
        assert_eq!(error.code(), "musicbrainz_unavailable");
        assert_eq!(
            error.annotate("Lookup unavailable."),
            "Lookup unavailable. Details: MusicBrainz: HTTP 503."
        );
        assert!(!format!("{error:?}").contains("private"));
        Ok(())
    }

    #[test]
    fn lastfm_diagnostics_keep_numeric_codes_without_provider_messages()
    -> Result<(), Box<dyn std::error::Error>> {
        let error = super::super::parse_community_tags(&serde_json::json!({
            "error": 26, "message": "api_key=private-key; private provider message"
        }))
        .err()
        .ok_or("fixture must fail")?;
        assert_eq!(error.code(), "lastfm_unavailable");
        assert_eq!(
            error.annotate("Tags unavailable."),
            "Tags unavailable. Details: Last.fm: provider error code 26."
        );
        let invalid = super::super::parse_community_tags(&serde_json::json!({
            "error": "private-error-code", "message": "private provider message"
        }))
        .err()
        .ok_or("invalid code must fail")?;
        assert_eq!(
            invalid.annotate("Tags unavailable."),
            "Tags unavailable. Details: Last.fm: unexpected response structure."
        );
        Ok(())
    }
}
