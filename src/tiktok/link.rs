use url::Url;

use super::ids::{ShortCode, TiktokVideoId};
use crate::youtube::{is_safe_token, parse_url};

/// Hosts of a link to one video.
const VIDEO_HOSTS: [&str; 3] = ["tiktok.com", "www.tiktok.com", "m.tiktok.com"];
/// Hosts of a short link (what the app's share button gives).
const SHORT_HOSTS: [&str; 2] = ["vt.tiktok.com", "vm.tiktok.com"];

const MAX_ACCOUNT_LEN: usize = 64;

/// What a viewer's `!vt` argument points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TiktokRef {
    /// The video itself: its ID is known without asking TikTok. The account
    /// is known when the viewer sent a web link (letters, digits, `_` and
    /// `.` only, so it is safe to show).
    Video {
        id: TiktokVideoId,
        account: Option<String>,
    },
    /// A short link's code: TikTok has to be asked which video it leads to.
    Short(ShortCode),
}

/// Reads one whitespace-free token typed in chat after a TikTok command.
///
/// Accepted:
/// - a bare video ID (`7700124541145599252`)
/// - a bare short link code (`ZSpGELffb`)
/// - `tiktok.com/@name/video/<id>` (also `www.` and `m.`; anything after
///   `?` is ignored)
/// - `vt.tiktok.com/<code>` and `vm.tiktok.com/<code>`
///
/// The scheme is optional (`http://` and `https://` only). Anything else gives
/// `None`: other hosts, other schemes, ports, credentials in the URL, extra
/// path segments. The result holds only validated values; the input text is
/// never reused.
pub fn extract_tiktok_ref(token: &str) -> Option<TiktokRef> {
    if !is_safe_token(token) {
        return None;
    }
    // Digits only, 15 to 20 of them, is an ID. Anything else made of letters
    // and digits is tried as a code. The two cannot clash: a code is at most
    // 12 characters and an ID at least 15.
    if let Ok(id) = TiktokVideoId::parse(token) {
        return Some(TiktokRef::Video { id, account: None });
    }
    if let Ok(code) = ShortCode::parse(token) {
        return Some(TiktokRef::Short(code));
    }

    let url = parse_url(token)?;
    if url.port().is_some() || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    let host = url.host_str()?;
    let segments = path_segments(&url)?;

    if SHORT_HOSTS.contains(&host) {
        return match segments.as_slice() {
            [code] => ShortCode::parse(code).ok().map(TiktokRef::Short),
            _ => None,
        };
    }
    if VIDEO_HOSTS.contains(&host) {
        return video_from_segments(&segments).map(|(account, id)| TiktokRef::Video {
            id,
            account: Some(account),
        });
    }
    None
}

/// The video a `https://www.tiktok.com/@name/video/<id>` address points at,
/// with the account's name. For the address a short link redirects to, which
/// is not typed by anyone and so must be held to the same rules.
pub(super) fn video_from_url(url: &Url) -> Option<(String, TiktokVideoId)> {
    if url.scheme() != "https"
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || !VIDEO_HOSTS.contains(&url.host_str()?)
    {
        return None;
    }
    video_from_segments(&path_segments(url)?)
}

fn video_from_segments(segments: &[&str]) -> Option<(String, TiktokVideoId)> {
    match segments {
        [account, "video", id] => {
            let name = account.strip_prefix('@')?;
            let id = TiktokVideoId::parse(id).ok()?;
            is_account(name).then(|| (name.to_owned(), id))
        }
        _ => None,
    }
}

/// Whether `name` can be an account name (without the `@`): letters, digits,
/// `_` and `.`, so it is safe to show and to put in a list.
pub(super) fn is_account(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_ACCOUNT_LEN
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
}

/// The path split in segments, tolerating one trailing slash.
fn path_segments(url: &Url) -> Option<Vec<&str>> {
    let segments: Vec<&str> = url.path_segments()?.collect();
    Some(match segments.split_last() {
        Some((&"", rest)) => rest.to_vec(),
        _ => segments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "7700124541145599252";

    fn video(token: &str) -> Option<String> {
        match extract_tiktok_ref(token)? {
            TiktokRef::Video { id, .. } => Some(id.as_str().to_owned()),
            TiktokRef::Short(_) => None,
        }
    }

    fn short(token: &str) -> Option<String> {
        match extract_tiktok_ref(token)? {
            TiktokRef::Short(code) => Some(code.as_str().to_owned()),
            TiktokRef::Video { .. } => None,
        }
    }

    #[test]
    fn a_bare_id_and_a_bare_code_are_told_apart() {
        assert_eq!(video(ID).as_deref(), Some(ID));
        assert_eq!(short("ZSpGELffb").as_deref(), Some("ZSpGELffb"));
        // Capitals are kept.
        assert_eq!(short("zSPgelFFB").as_deref(), Some("zSPgelFFB"));
        // Digits only but too short for an ID: a code.
        assert_eq!(short("123456789").as_deref(), Some("123456789"));
        // A YouTube ID made only of letters and digits looks like a code. It
        // is asked of TikTok and leads nowhere; that costs one cached request.
        assert_eq!(short("dQw4w9WgXcQ").as_deref(), Some("dQw4w9WgXcQ"));
        // 13 and 14 digits are neither (a code stops at 12, an ID starts at 15).
        assert_eq!(extract_tiktok_ref("1234567890123"), None);
        assert_eq!(extract_tiktok_ref("12345678901234"), None);
    }

    #[test]
    fn the_link_from_the_web_gives_the_id() {
        for link in [
            "https://www.tiktok.com/@cherlynn052/video/7700124541145599252?is_from_webapp=1&sender_device=pc",
            "https://www.tiktok.com/@cherlynn052/video/7700124541145599252",
            "https://www.tiktok.com/@cherlynn052/video/7700124541145599252/",
            "http://tiktok.com/@cherlynn052/video/7700124541145599252",
            "https://m.tiktok.com/@cher.lynn_05/video/7700124541145599252?x=1#frag",
            "www.tiktok.com/@cherlynn052/video/7700124541145599252",
            "HTTPS://WWW.TIKTOK.COM/@cherlynn052/video/7700124541145599252",
            "tiktok.com/@a/video/7700124541145599252",
        ] {
            assert_eq!(video(link).as_deref(), Some(ID), "{link}");
        }
    }

    #[test]
    fn the_account_comes_with_a_web_link_but_not_with_a_bare_id() {
        let account = |token: &str| match extract_tiktok_ref(token) {
            Some(TiktokRef::Video { account, .. }) => account,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            account("https://www.tiktok.com/@cher.lynn_05/video/7700124541145599252").as_deref(),
            Some("cher.lynn_05")
        );
        assert_eq!(account(ID), None);
    }

    #[test]
    fn the_link_from_the_app_gives_the_code() {
        for link in [
            "https://vt.tiktok.com/ZSpGELffb/",
            "https://vt.tiktok.com/ZSpGELffb",
            "http://vm.tiktok.com/ZSpGELffb/",
            "vt.tiktok.com/ZSpGELffb/",
            "https://vt.tiktok.com/ZSpGELffb/?utm=1",
        ] {
            assert_eq!(short(link).as_deref(), Some("ZSpGELffb"), "{link}");
        }
    }

    #[test]
    fn other_hosts_schemes_and_shapes_are_refused() {
        for bad in [
            // other hosts, including lookalikes and subdomain tricks
            "https://evil.com/@a/video/7700124541145599252",
            "https://tiktok.com.evil.com/@a/video/7700124541145599252",
            "https://evil.com/tiktok.com/@a/video/7700124541145599252",
            "https://nottiktok.com/@a/video/7700124541145599252",
            "https://www.tiktok.com.evil.com/@a/video/7700124541145599252",
            "https://vt.tiktok.com.evil.com/ZSpGELffb/",
            "https://evil.com/ZSpGELffb/",
            "https://www.tiktokv.com/@a/video/7700124541145599252",
            "https://tiktok.evil/@a/video/7700124541145599252",
            "https://youtu.be/dQw4w9WgXcQ",
            // other schemes
            "ftp://www.tiktok.com/@a/video/7700124541145599252",
            "javascript:alert(1)",
            "data:text/html,x",
            "//www.tiktok.com/@a/video/7700124541145599252",
            "@www.tiktok.com/@a/video/7700124541145599252",
            // ports and credentials
            "https://www.tiktok.com:8080/@a/video/7700124541145599252",
            "https://user@www.tiktok.com/@a/video/7700124541145599252",
            "https://user:pw@vt.tiktok.com/ZSpGELffb/",
            "https://vt.tiktok.com:444/ZSpGELffb/",
            // wrong shapes
            "https://www.tiktok.com/@a/video/",
            "https://www.tiktok.com/@a/video/123",
            "https://www.tiktok.com/@a/video/7700124541145599252/extra",
            "https://www.tiktok.com/@a/photo/7700124541145599252",
            "https://www.tiktok.com/a/video/7700124541145599252",
            "https://www.tiktok.com/@/video/7700124541145599252",
            "https://www.tiktok.com/@a%20b/video/7700124541145599252",
            "https://www.tiktok.com/@a<b>/video/7700124541145599252",
            "https://www.tiktok.com/video/7700124541145599252",
            "https://www.tiktok.com/",
            "https://www.tiktok.com/@a",
            "https://vt.tiktok.com/",
            "https://vt.tiktok.com/ZSpGELffb/extra",
            "https://vt.tiktok.com/ZSp-GELffb/",
            "https://vt.tiktok.com/ZSp/",
            "https://vt.tiktok.com/@a/video/7700124541145599252",
            // not a link at all
            "",
            "hello",
            "ZSp GELffb",
            "ZSpGELffb\n",
            "ZSpGE",
            "7700124541145599252a",
            "dQw4w9_WgXc", // a YouTube ID with an underscore is not a code
            "https://www.tiktok.com/@a/video/7700124541145599252\\evil",
            "https://www.tiktok.com/@a/video/7700124541145599252\u{0}",
            "😀",
            "!vt",
        ] {
            assert_eq!(extract_tiktok_ref(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_result_never_carries_the_typed_text() {
        let token = "https://www.tiktok.com/@a/video/7700124541145599252?x=%3Cscript%3E";
        match extract_tiktok_ref(token).unwrap() {
            TiktokRef::Video { id, account } => {
                assert_eq!(id.as_str(), ID);
                assert_eq!(account.as_deref(), Some("a"));
            }
            TiktokRef::Short(_) => panic!("not a code"),
        }
    }

    #[test]
    fn oversized_and_hostile_tokens_do_not_panic() {
        assert_eq!(extract_tiktok_ref(&"a".repeat(100_000)), None);
        assert_eq!(
            extract_tiktok_ref(&format!("https://vt.tiktok.com/{}", "a".repeat(100_000))),
            None
        );
        assert_eq!(
            extract_tiktok_ref(&format!("{}://x", "a".repeat(600))),
            None
        );
        for odd in [
            "https://",
            "http:///",
            "https://[::1",
            "https://:::",
            "https://%00",
            "https://.com/",
        ] {
            assert_eq!(extract_tiktok_ref(odd), None, "{odd:?}");
        }
    }

    #[test]
    fn a_redirect_target_is_held_to_the_same_rules() {
        let parse = |s: &str| video_from_url(&Url::parse(s).unwrap());
        assert_eq!(
            parse(
                "https://www.tiktok.com/@feriumar758/video/7219950637247728923?_r=1&_t=ZS-9BTRBCxhj9O"
            ),
            Some((
                "feriumar758".to_owned(),
                TiktokVideoId::parse("7219950637247728923").unwrap()
            ))
        );
        for bad in [
            "http://www.tiktok.com/@a/video/7219950637247728923", // not https
            "https://evil.com/@a/video/7219950637247728923",
            "https://www.tiktok.com.evil.com/@a/video/7219950637247728923",
            "https://www.tiktok.com:444/@a/video/7219950637247728923",
            "https://u@www.tiktok.com/@a/video/7219950637247728923",
            "https://www.tiktok.com/?_r=1",
            "https://www.tiktok.com/@a/video/12",
            "https://vt.tiktok.com/ZSpGELffb/",
        ] {
            assert_eq!(parse(bad), None, "{bad}");
        }
    }
}
