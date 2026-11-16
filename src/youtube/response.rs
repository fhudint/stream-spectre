//! Turning a `videos.list` HTTP response into `VideoDetails` or an error.
//!
//! Pure functions only, so every branch can be tested without a network.

use serde::Deserialize;

use super::duration::parse_iso8601_duration;
use super::error::YoutubeError;
use super::model::{LiveStatus, VideoDetails, sanitize_text};
use super::video_id::VideoId;

/// `part` parameter: the three resource sections we read. Per the API docs a
/// call costs 1 quota unit regardless of the parts requested.
pub(crate) const PARTS: &str = "snippet,contentDetails,status";

const AGE_RESTRICTED_RATING: &str = "ytAgeRestricted";
const MAX_REASON_LEN: usize = 64;

#[derive(Debug, Deserialize)]
struct ListResponse {
    #[serde(default)]
    items: Vec<Item>,
}

#[derive(Debug, Deserialize)]
struct Item {
    id: Option<String>,
    snippet: Option<Snippet>,
    #[serde(rename = "contentDetails")]
    content_details: Option<ContentDetails>,
    status: Option<Status>,
}

#[derive(Debug, Deserialize)]
struct Snippet {
    title: Option<String>,
    #[serde(rename = "channelTitle")]
    channel_title: Option<String>,
    #[serde(rename = "liveBroadcastContent")]
    live_broadcast_content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContentDetails {
    duration: Option<String>,
    #[serde(rename = "contentRating")]
    content_rating: Option<ContentRating>,
}

#[derive(Debug, Deserialize)]
struct ContentRating {
    #[serde(rename = "ytRating")]
    yt_rating: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Status {
    #[serde(rename = "privacyStatus")]
    privacy_status: Option<String>,
    embeddable: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: Option<ErrorBody>,
}

#[derive(Debug, Default, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    errors: Vec<ErrorReason>,
    #[serde(default)]
    details: Vec<ErrorReason>,
}

#[derive(Debug, Deserialize)]
struct ErrorReason {
    reason: Option<String>,
}

/// Interprets the response to a `videos.list` request for `requested`.
///
/// `Ok(None)` means YouTube has no such (visible) video.
pub(crate) fn interpret_response(
    requested: &VideoId,
    status: u16,
    body: &[u8],
) -> Result<Option<VideoDetails>, YoutubeError> {
    if (200..300).contains(&status) {
        parse_success(requested, body)
    } else {
        classify_error(status, body)
    }
}

fn parse_success(requested: &VideoId, body: &[u8]) -> Result<Option<VideoDetails>, YoutubeError> {
    // serde would also accept a JSON array as an (empty) struct; a real
    // response is always an object.
    if body.iter().find(|b| !b.is_ascii_whitespace()) != Some(&b'{') {
        return Err(YoutubeError::BadResponse("response is not a JSON object"));
    }
    let response: ListResponse = serde_json::from_slice(body)
        .map_err(|_| YoutubeError::BadResponse("response is not the expected JSON"))?;

    let Some(item) = response.items.into_iter().next() else {
        return Ok(None);
    };
    if item.id.as_deref() != Some(requested.as_str()) {
        return Err(YoutubeError::BadResponse("video ID does not match request"));
    }

    let snippet = item
        .snippet
        .ok_or(YoutubeError::BadResponse("missing snippet"))?;
    let content_details = item
        .content_details
        .ok_or(YoutubeError::BadResponse("missing contentDetails"))?;
    let status = item
        .status
        .ok_or(YoutubeError::BadResponse("missing status"))?;

    let duration_secs = content_details
        .duration
        .as_deref()
        .and_then(parse_iso8601_duration)
        .ok_or(YoutubeError::BadResponse("missing or unreadable duration"))?;

    let live = match snippet.live_broadcast_content.as_deref() {
        None | Some("none") => LiveStatus::None,
        Some("upcoming") => LiveStatus::Upcoming,
        // "live", or any value we do not know: treat as not a normal video.
        Some(_) => LiveStatus::Live,
    };

    Ok(Some(VideoDetails {
        id: requested.clone(),
        title: sanitize_text(snippet.title.as_deref().unwrap_or_default()),
        channel: sanitize_text(snippet.channel_title.as_deref().unwrap_or_default()),
        duration_secs,
        // Missing means we cannot prove it is embeddable.
        embeddable: status.embeddable.unwrap_or(false),
        age_restricted: content_details
            .content_rating
            .and_then(|rating| rating.yt_rating)
            .as_deref()
            == Some(AGE_RESTRICTED_RATING),
        live,
        private: status.privacy_status.as_deref() == Some("private"),
    }))
}

/// Maps an error response to an error, or to `Ok(None)` for "no such video".
fn classify_error(status: u16, body: &[u8]) -> Result<Option<VideoDetails>, YoutubeError> {
    let reasons = error_reasons(body);
    let has = |wanted: &str| reasons.iter().any(|r| r == wanted);

    if has("quotaExceeded") {
        return Err(YoutubeError::QuotaExceeded);
    }
    if has("rateLimitExceeded") || status == 429 {
        return Err(YoutubeError::RateLimited);
    }
    if status == 400 && has("API_KEY_INVALID") {
        return Err(YoutubeError::InvalidApiKey);
    }
    if status == 404 {
        return Ok(None);
    }

    let reason = reasons
        .into_iter()
        .next()
        .unwrap_or_else(|| "unknown".to_owned());
    match status {
        401 | 403 => Err(YoutubeError::AccessDenied { reason }),
        500..=599 => Err(YoutubeError::Unavailable { status }),
        _ => Err(YoutubeError::Rejected { status, reason }),
    }
}

/// Reason codes from a Google error body, sanitized. Empty when the body is
/// not the usual JSON error.
fn error_reasons(body: &[u8]) -> Vec<String> {
    let Ok(envelope) = serde_json::from_slice::<ErrorEnvelope>(body) else {
        return Vec::new();
    };
    let error = envelope.error.unwrap_or_default();
    error
        .errors
        .into_iter()
        .chain(error.details)
        .filter_map(|entry| entry.reason)
        .map(|reason| sanitize_reason(&reason))
        .collect()
}

fn sanitize_reason(reason: &str) -> String {
    let cleaned: String = reason
        .chars()
        .take(MAX_REASON_LEN)
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .collect();
    if cleaned.is_empty() {
        "unknown".to_owned()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "dQw4w9WgXcQ";

    fn id() -> VideoId {
        VideoId::parse(ID).unwrap()
    }

    fn ok_body(snippet_extra: &str, details_extra: &str, status_extra: &str) -> String {
        format!(
            r#"{{"items":[{{"id":"{ID}",
              "snippet":{{"title":"Never Gonna Give You Up","channelTitle":"Rick Astley"{snippet_extra}}},
              "contentDetails":{{"duration":"PT3M33S"{details_extra}}},
              "status":{{"privacyStatus":"public","embeddable":true{status_extra}}}}}]}}"#
        )
    }

    fn details(body: &str) -> VideoDetails {
        interpret_response(&id(), 200, body.as_bytes())
            .unwrap()
            .unwrap()
    }

    #[test]
    fn maps_a_normal_video() {
        let d = details(&ok_body("", "", ""));
        assert_eq!(d.id, id());
        assert_eq!(d.title, "Never Gonna Give You Up");
        assert_eq!(d.channel, "Rick Astley");
        assert_eq!(d.duration_secs, 213);
        assert!(d.embeddable);
        assert!(!d.age_restricted);
        assert_eq!(d.live, LiveStatus::None);
        assert!(!d.private);
    }

    #[test]
    fn detects_age_restriction() {
        let d = details(&ok_body(
            "",
            r#","contentRating":{"ytRating":"ytAgeRestricted"}"#,
            "",
        ));
        assert!(d.age_restricted);

        let d = details(&ok_body("", r#","contentRating":{}"#, ""));
        assert!(!d.age_restricted);
    }

    #[test]
    fn detects_live_and_upcoming() {
        let d = details(&ok_body(r#","liveBroadcastContent":"live""#, "", ""));
        assert_eq!(d.live, LiveStatus::Live);
        let d = details(&ok_body(r#","liveBroadcastContent":"upcoming""#, "", ""));
        assert_eq!(d.live, LiveStatus::Upcoming);
        let d = details(&ok_body(r#","liveBroadcastContent":"none""#, "", ""));
        assert_eq!(d.live, LiveStatus::None);
        let d = details(&ok_body(
            r#","liveBroadcastContent":"somethingnew""#,
            "",
            "",
        ));
        assert_eq!(d.live, LiveStatus::Live);
    }

    #[test]
    fn missing_embeddable_counts_as_not_embeddable() {
        let body = format!(
            r#"{{"items":[{{"id":"{ID}","snippet":{{"title":"t","channelTitle":"c"}},
               "contentDetails":{{"duration":"PT1M"}},"status":{{"privacyStatus":"public"}}}}]}}"#
        );
        assert!(!details(&body).embeddable);
    }

    #[test]
    fn embeddable_false_and_private_are_reported() {
        let body = ok_body("", "", "").replace("\"embeddable\":true", "\"embeddable\":false");
        assert!(!details(&body).embeddable);
        let body = ok_body("", "", "").replace("\"public\"", "\"private\"");
        assert!(details(&body).private);
        let body = ok_body("", "", "").replace("\"public\"", "\"unlisted\"");
        assert!(!details(&body).private);
    }

    #[test]
    fn title_and_channel_are_sanitized() {
        let body = ok_body("", "", "")
            .replace("Never Gonna Give You Up", "Line1\\nLine2\\u0000 <b>x</b>")
            .replace("Rick Astley", &"é".repeat(500));
        let d = details(&body);
        assert_eq!(d.title, "Line1 Line2  <b>x</b>");
        assert_eq!(d.channel.chars().count(), 200);
    }

    #[test]
    fn empty_items_means_not_found() {
        assert_eq!(interpret_response(&id(), 200, br#"{"items":[]}"#), Ok(None));
        assert_eq!(interpret_response(&id(), 200, b"{}"), Ok(None));
    }

    #[test]
    fn unexpected_success_bodies_are_bad_responses() {
        let bad_id = ok_body("", "", "").replace(ID, "aaaaaaaaaaa");
        let no_duration = ok_body("", "", "").replace("\"duration\":\"PT3M33S\"", "\"x\":1");
        let junk_duration = ok_body("", "", "").replace("PT3M33S", "3:33");
        let no_snippet = format!(r#"{{"items":[{{"id":"{ID}"}}]}}"#);
        for body in [
            bad_id.as_str(),
            no_duration.as_str(),
            junk_duration.as_str(),
            no_snippet.as_str(),
            "not json",
            "",
            "<html>proxy login</html>",
            "[]",
            r#"{"items":"nope"}"#,
        ] {
            assert!(
                matches!(
                    interpret_response(&id(), 200, body.as_bytes()),
                    Err(YoutubeError::BadResponse(_))
                ),
                "{body}"
            );
        }
    }

    // Captured from the real API with an invalid key.
    const INVALID_KEY_BODY: &str = r#"{
      "error": {
        "code": 400,
        "message": "API key not valid. Please pass a valid API key.",
        "errors": [{"message": "API key not valid. Please pass a valid API key.", "domain": "global", "reason": "badRequest"}],
        "status": "INVALID_ARGUMENT",
        "details": [
          {"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "API_KEY_INVALID", "domain": "googleapis.com", "metadata": {"service": "youtube.googleapis.com"}},
          {"@type": "type.googleapis.com/google.rpc.LocalizedMessage", "locale": "en-US", "message": "API key not valid. Please pass a valid API key."}
        ]
      }
    }"#;

    #[test]
    fn invalid_api_key_is_recognised() {
        assert_eq!(
            interpret_response(&id(), 400, INVALID_KEY_BODY.as_bytes()),
            Err(YoutubeError::InvalidApiKey)
        );
        assert!(YoutubeError::InvalidApiKey.is_config_problem());
    }

    fn error_body(reason: &str) -> String {
        format!(r#"{{"error":{{"code":0,"errors":[{{"reason":"{reason}"}}]}}}}"#)
    }

    #[test]
    fn quota_and_rate_limit_errors_are_recognised() {
        assert_eq!(
            interpret_response(&id(), 403, error_body("quotaExceeded").as_bytes()),
            Err(YoutubeError::QuotaExceeded)
        );
        assert_eq!(
            interpret_response(&id(), 403, error_body("rateLimitExceeded").as_bytes()),
            Err(YoutubeError::RateLimited)
        );
        assert_eq!(
            interpret_response(&id(), 429, b"slow down"),
            Err(YoutubeError::RateLimited)
        );
    }

    #[test]
    fn other_client_errors_keep_a_sanitized_reason() {
        assert_eq!(
            interpret_response(&id(), 403, error_body("accessNotConfigured").as_bytes()),
            Err(YoutubeError::AccessDenied {
                reason: "accessNotConfigured".to_owned()
            })
        );
        assert_eq!(
            interpret_response(
                &id(),
                403,
                br#"{"error":{"errors":[{"reason":"forbidden"}]}}"#
            ),
            Err(YoutubeError::AccessDenied {
                reason: "forbidden".to_owned()
            })
        );
        assert_eq!(
            interpret_response(&id(), 400, error_body("badRequest").as_bytes()),
            Err(YoutubeError::Rejected {
                status: 400,
                reason: "badRequest".to_owned()
            })
        );
        // A hostile reason cannot smuggle markup or secrets into messages.
        let nasty = error_body("<script>alert('k3y')</script>");
        let err = interpret_response(&id(), 400, nasty.as_bytes()).unwrap_err();
        let message = err.to_string();
        assert!(
            !message.contains('<') && !message.contains('\''),
            "{message}"
        );
    }

    #[test]
    fn not_found_and_server_errors() {
        assert_eq!(
            interpret_response(&id(), 404, error_body("videoNotFound").as_bytes()),
            Ok(None)
        );
        for status in [500, 502, 503, 504] {
            assert_eq!(
                interpret_response(&id(), status, b"oops"),
                Err(YoutubeError::Unavailable { status })
            );
        }
        assert_eq!(
            interpret_response(&id(), 302, b""),
            Err(YoutubeError::Rejected {
                status: 302,
                reason: "unknown".to_owned()
            })
        );
    }

    #[test]
    fn garbage_error_bodies_do_not_panic() {
        for body in [
            "",
            "null",
            "[]",
            "{",
            "\u{0}",
            r#"{"error":null}"#,
            r#"{"error":{"errors":[{}]}}"#,
        ] {
            let result = interpret_response(&id(), 403, body.as_bytes());
            assert!(
                matches!(result, Err(YoutubeError::AccessDenied { .. })),
                "{body:?}"
            );
        }
    }

    #[test]
    fn notice_keys_exist_in_both_locales() {
        use crate::config::Language;
        use crate::i18n::I18n;
        for language in [Language::Id, Language::En] {
            let i18n = I18n::load(language).unwrap();
            for error in [
                YoutubeError::QuotaExceeded,
                YoutubeError::RateLimited,
                YoutubeError::Network("timeout"),
            ] {
                let key = error.notice_key();
                assert_ne!(i18n.t(key), key, "missing {key} in {language:?}");
            }
        }
    }
}
