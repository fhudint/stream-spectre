use url::Url;

use super::video_id::VideoId;

/// Longest token we are willing to inspect. Chat messages are far shorter;
/// this only bounds the work done on hostile input.
const MAX_TOKEN_LEN: usize = 512;

const SHORT_HOST: &str = "youtu.be";
const LONG_HOSTS: [&str; 3] = ["youtube.com", "www.youtube.com", "m.youtube.com"];

/// Extracts a video ID from one whitespace-free token typed in chat.
///
/// Accepted:
/// - a bare 11-character ID
/// - `youtu.be/ID`
/// - `youtube.com/watch?v=ID` (also `www.` and `m.`)
/// - `youtube.com/shorts/ID` (also `www.` and `m.`)
///
/// The scheme is optional (`http://` and `https://` only). Anything else
/// yields `None`: other hosts, other schemes, ports, credentials in the URL,
/// extra path segments, and IDs that fail validation. The result is always a
/// validated `VideoId`; the input text is never reused.
pub fn extract_video_id(token: &str) -> Option<VideoId> {
    // Control characters and backslashes are silently dropped or rewritten by
    // URL parsers, so refuse them instead of guessing what was meant.
    if token.len() > MAX_TOKEN_LEN || token.chars().any(|c| c.is_control() || c == '\\') {
        return None;
    }
    if let Ok(id) = VideoId::parse(token) {
        return Some(id);
    }

    let url = parse_url(token)?;
    if url.port().is_some() || !url.username().is_empty() || url.password().is_some() {
        return None;
    }

    let host = url.host_str()?;
    let segments: Vec<&str> = url.path_segments()?.collect();
    // Tolerate one trailing slash.
    let segments = match segments.split_last() {
        Some((&"", rest)) => rest,
        _ => segments.as_slice(),
    };

    if host == SHORT_HOST {
        return match segments {
            [id] => VideoId::parse(id).ok(),
            _ => None,
        };
    }

    if LONG_HOSTS.contains(&host) {
        return match segments {
            ["watch"] => url
                .query_pairs()
                .find(|(key, _)| key == "v")
                .and_then(|(_, value)| VideoId::parse(&value).ok()),
            ["shorts", id] => VideoId::parse(id).ok(),
            _ => None,
        };
    }

    None
}

/// Parses the token as an `https` URL, adding the scheme when it is missing.
fn parse_url(token: &str) -> Option<Url> {
    let rest = strip_web_scheme(token)?;
    // A host starts with a letter or digit; this also rejects "//host/..."
    // and "@host/...", which URL parsers would otherwise normalise.
    if !rest.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        return None;
    }
    Url::parse(&format!("https://{rest}")).ok()
}

/// Removes a leading `http://` or `https://` (any case). A token with any
/// other `scheme://` is rejected; a token without one is returned unchanged.
fn strip_web_scheme(token: &str) -> Option<&str> {
    for scheme in ["https://", "http://"] {
        let matches = token
            .get(..scheme.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(scheme));
        if matches {
            return token.get(scheme.len()..);
        }
    }
    if token.contains("://") {
        None
    } else {
        Some(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "dQw4w9WgXcQ";

    fn id(token: &str) -> Option<String> {
        extract_video_id(token).map(|v| v.as_str().to_owned())
    }

    fn accepted(token: &str) {
        assert_eq!(id(token).as_deref(), Some(ID), "should accept {token:?}");
    }

    fn rejected(token: &str) {
        assert_eq!(id(token), None, "should reject {token:?}");
    }

    #[test]
    fn accepts_bare_id() {
        accepted(ID);
        assert_eq!(id("a_b-C1d2E3f").as_deref(), Some("a_b-C1d2E3f"));
    }

    #[test]
    fn accepts_the_three_documented_link_forms() {
        accepted("https://youtu.be/dQw4w9WgXcQ");
        accepted("https://www.youtube.com/watch?v=dQw4w9WgXcQ");
        accepted("https://youtube.com/watch?v=dQw4w9WgXcQ");
        accepted("https://www.youtube.com/shorts/dQw4w9WgXcQ");
        accepted("https://youtube.com/shorts/dQw4w9WgXcQ");
    }

    #[test]
    fn accepts_mobile_host_and_missing_or_plain_scheme() {
        accepted("https://m.youtube.com/watch?v=dQw4w9WgXcQ");
        accepted("http://youtu.be/dQw4w9WgXcQ");
        accepted("youtu.be/dQw4w9WgXcQ");
        accepted("youtube.com/watch?v=dQw4w9WgXcQ");
        accepted("www.youtube.com/shorts/dQw4w9WgXcQ");
    }

    #[test]
    fn scheme_and_host_are_case_insensitive_but_the_id_is_not() {
        accepted("HTTPS://YOUTU.BE/dQw4w9WgXcQ");
        accepted("Https://WWW.YouTube.com/watch?v=dQw4w9WgXcQ");
        assert_eq!(
            id("https://youtu.be/DQW4W9WGXCQ").as_deref(),
            Some("DQW4W9WGXCQ")
        );
    }

    #[test]
    fn tolerates_extra_query_fragment_and_trailing_slash() {
        accepted("https://youtu.be/dQw4w9WgXcQ?t=42");
        accepted("https://youtu.be/dQw4w9WgXcQ?si=abcdef&t=1");
        accepted("https://youtu.be/dQw4w9WgXcQ/");
        accepted("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42s&list=PLabc");
        accepted("https://www.youtube.com/watch?feature=share&v=dQw4w9WgXcQ");
        accepted("https://www.youtube.com/watch/?v=dQw4w9WgXcQ");
        accepted("https://www.youtube.com/watch?v=dQw4w9WgXcQ#frag");
        accepted("https://www.youtube.com/shorts/dQw4w9WgXcQ?feature=share");
        accepted("https://www.youtube.com/shorts/dQw4w9WgXcQ/");
    }

    #[test]
    fn percent_encoded_query_value_is_decoded_then_validated() {
        accepted("https://www.youtube.com/watch?v=%64Qw4w9WgXcQ");
        rejected("https://www.youtube.com/watch?v=dQw4w9WgXc%2F");
    }

    #[test]
    fn rejects_other_hosts_and_lookalikes() {
        rejected("https://evil.com/watch?v=dQw4w9WgXcQ");
        rejected("https://youtube.com.evil.com/watch?v=dQw4w9WgXcQ");
        rejected("https://evil.com/youtube.com/watch?v=dQw4w9WgXcQ");
        rejected("https://notyoutube.com/watch?v=dQw4w9WgXcQ");
        rejected("https://youtu.be.evil.com/dQw4w9WgXcQ");
        rejected("https://evilyoutu.be/dQw4w9WgXcQ");
        rejected("https://youtube.co/watch?v=dQw4w9WgXcQ");
        rejected("https://youtube.com./watch?v=dQw4w9WgXcQ");
        rejected("https://yоutube.com/watch?v=dQw4w9WgXcQ"); // Cyrillic 'о'
        rejected("https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ");
    }

    #[test]
    fn rejects_credentials_and_ports() {
        rejected("https://youtube.com@evil.com/watch?v=dQw4w9WgXcQ");
        rejected("https://user:pass@youtube.com/watch?v=dQw4w9WgXcQ");
        rejected("https://youtube.com:8080/watch?v=dQw4w9WgXcQ");
        rejected("https://youtu.be:444/dQw4w9WgXcQ");
    }

    #[test]
    fn rejects_other_schemes() {
        rejected("javascript:alert(1)");
        rejected("ftp://youtu.be/dQw4w9WgXcQ");
        rejected("data:text/html,<script>alert(1)</script>");
        rejected("file:///etc/passwd");
        rejected("//youtu.be/dQw4w9WgXcQ");
        rejected("vbscript://youtu.be/dQw4w9WgXcQ");
    }

    #[test]
    fn rejects_backslashes_and_control_characters() {
        rejected("youtu.be\\dQw4w9WgXcQ");
        rejected("https://youtu.be\\dQw4w9WgXcQ");
        rejected("\\\\youtu.be/dQw4w9WgXcQ");
        rejected("you\ttu.be/dQw4w9WgXcQ");
        rejected("https://youtu.be/dQw4w9\nWgXcQ");
        rejected("youtu.be/dQw4w9WgXcQ\n");
        rejected("@youtu.be/dQw4w9WgXcQ");
        rejected("/youtu.be/dQw4w9WgXcQ");
    }

    #[test]
    fn rejects_wrong_paths_and_missing_or_bad_ids() {
        rejected("https://www.youtube.com/watch");
        rejected("https://www.youtube.com/watch?v=");
        rejected("https://www.youtube.com/watch?v=short");
        rejected("https://www.youtube.com/watch?x=dQw4w9WgXcQ");
        rejected("https://www.youtube.com/watch/extra?v=dQw4w9WgXcQ");
        rejected("https://www.youtube.com/");
        rejected("https://www.youtube.com/shorts/");
        rejected("https://www.youtube.com/shorts/dQw4w9WgXcQ/extra");
        rejected("https://www.youtube.com/embed/dQw4w9WgXcQ");
        rejected("https://www.youtube.com/playlist?list=PLabc");
        rejected("https://www.youtube.com/@somechannel");
        rejected("https://youtu.be/");
        rejected("https://youtu.be/short");
        rejected("https://youtu.be/dQw4w9WgXcQ/extra");
        rejected("https://youtu.be/%64Qw4w9WgXcQ"); // path is not percent-decoded
    }

    #[test]
    fn the_first_v_parameter_decides() {
        accepted("https://www.youtube.com/watch?v=dQw4w9WgXcQ&v=aaaaaaaaaaa");
        rejected("https://www.youtube.com/watch?v=bad&v=dQw4w9WgXcQ");
    }

    #[test]
    fn garbage_and_oversized_input_are_rejected_without_panicking() {
        for token in [
            "",
            "hello",
            "http://",
            "https://",
            "://",
            "youtu.be",
            "youtu.be/",
            "🎵🎵🎵",
            "é",
            "https://é/é",
            "\u{0}",
            "%",
            "?v=dQw4w9WgXcQ",
        ] {
            rejected(token);
        }
        let long = format!("https://youtu.be/{}", "a".repeat(10_000));
        rejected(&long);
        let long_valid_prefix =
            format!("https://www.youtube.com/watch?v={ID}&x={}", "a".repeat(600));
        rejected(&long_valid_prefix);
    }
}
