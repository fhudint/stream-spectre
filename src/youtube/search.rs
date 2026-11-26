//! Looking for a video by words, for the search command.
//!
//! YouTube's own search page is asked first: it costs no quota, but it is not
//! an API and its answer can change. The Data API's `search.list` is the
//! second choice, because each call takes 100 quota units; the pipeline counts
//! those and stops at the limit the streamer set.

use serde_json::Value;

use super::error::YoutubeError;
use super::response::classify_error;
use super::video_id::VideoId;

/// Most candidates taken from one search; the first one that passes the rules
/// is used, and a video check costs 1 quota unit (less with the cache).
pub const MAX_RESULTS: usize = 5;

/// What a search found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchResult {
    /// The videos found, best match first, at most `MAX_RESULTS`.
    pub ids: Vec<VideoId>,
    /// The Data API was used (100 quota units), not YouTube's own page.
    pub official: bool,
}

/// The video IDs in an answer of YouTube's own search, best first. `None`
/// when the answer is not what is expected (not a JSON object, or without the
/// `contents` of a search): the page has changed, or something is in the way.
/// An answer with no videos in it is `Some` and empty: nothing was found.
pub(crate) fn ids_from_page(body: &[u8]) -> Option<Vec<VideoId>> {
    let root: Value = serde_json::from_slice(body).ok()?;
    root.get("contents")?;
    let mut ids = Vec::new();
    // Depth first, in the order of the page: the first results come first.
    let mut stack = vec![&root];
    while let Some(node) = stack.pop() {
        match node {
            Value::Object(map) => {
                if let Some(video) = map.get("videoRenderer") {
                    if let Some(id) = video
                        .get("videoId")
                        .and_then(Value::as_str)
                        .and_then(|id| VideoId::parse(id).ok())
                    {
                        if !ids.contains(&id) {
                            ids.push(id);
                        }
                        if ids.len() == MAX_RESULTS {
                            break;
                        }
                    }
                    continue;
                }
                // Reversed, so that the first entry is the next one taken.
                stack.extend(map.values().rev());
            }
            Value::Array(list) => stack.extend(list.iter().rev()),
            _ => {}
        }
    }
    Some(ids)
}

/// The video IDs in the answer of the Data API's `search.list`.
pub(crate) fn ids_from_api(status: u16, body: &[u8]) -> Result<Vec<VideoId>, YoutubeError> {
    if !(200..300).contains(&status) {
        // "Not found" is an empty search, anything else an error.
        classify_error(status, body)?;
        return Ok(Vec::new());
    }
    let root: Value = serde_json::from_slice(body)
        .map_err(|_| YoutubeError::BadResponse("response is not the expected JSON"))?;
    let items = root
        .get("items")
        .and_then(Value::as_array)
        .ok_or(YoutubeError::BadResponse("response has no items"))?;
    Ok(items
        .iter()
        .filter_map(|item| item.pointer("/id/videoId").and_then(Value::as_str))
        .filter_map(|id| VideoId::parse(id).ok())
        .take(MAX_RESULTS)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> String {
        format!("{n:0>11}")
    }

    fn page(videos: &[String]) -> Vec<u8> {
        let items: Vec<Value> = videos
            .iter()
            .map(|id| serde_json::json!({"videoRenderer": {"videoId": id, "title": {}}}))
            .collect();
        serde_json::json!({
            "contents": {"twoColumn": {"primary": {"sections": [
                {"ads": {"adRenderer": {}}},
                {"itemSection": {"contents": items}},
            ]}}}
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn the_page_gives_the_videos_in_its_order_up_to_the_limit() {
        let all: Vec<String> = (1..=9).map(id).collect();
        let found = ids_from_page(&page(&all)).unwrap();
        assert_eq!(found.len(), MAX_RESULTS);
        let names: Vec<&str> = found.iter().map(VideoId::as_str).collect();
        assert_eq!(names, all[..MAX_RESULTS]);
    }

    #[test]
    fn a_video_listed_twice_counts_once_and_a_bad_id_is_left_out() {
        let found = ids_from_page(&page(&[id(1), id(1), "not an id".to_owned(), id(2)])).unwrap();
        let names: Vec<&str> = found.iter().map(VideoId::as_str).collect();
        assert_eq!(names, [id(1), id(2)]);
    }

    #[test]
    fn nothing_found_is_not_the_same_as_a_page_that_has_changed() {
        assert_eq!(ids_from_page(&page(&[])), Some(Vec::new()));
        for odd in [
            &b"[]"[..],
            b"not json",
            b"{}",
            br#"{"error":{"code":400}}"#,
            b"",
        ] {
            assert_eq!(ids_from_page(odd), None, "{odd:?}");
        }
    }

    #[test]
    fn the_api_answer_gives_ids_and_names_its_errors() {
        let ok = format!(
            r#"{{"items":[{{"id":{{"kind":"youtube#video","videoId":"{}"}}}},{{"id":{{"kind":"youtube#channel","channelId":"x"}}}}]}}"#,
            id(7)
        );
        let found = ids_from_api(200, ok.as_bytes()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].as_str(), id(7));
        assert_eq!(ids_from_api(200, br#"{"items":[]}"#).unwrap(), Vec::new());
        assert!(matches!(
            ids_from_api(200, b"{}"),
            Err(YoutubeError::BadResponse(_))
        ));
        assert_eq!(
            ids_from_api(403, br#"{"error":{"errors":[{"reason":"quotaExceeded"}]}}"#),
            Err(YoutubeError::QuotaExceeded)
        );
        assert_eq!(
            ids_from_api(
                400,
                br#"{"error":{"errors":[{"reason":"API_KEY_INVALID"}]}}"#
            ),
            Err(YoutubeError::InvalidApiKey)
        );
    }
}
