//! YouTube live chat, read the way the YouTube website itself reads it.
//!
//! The official chat API needs OAuth or costs quota (5 units a call), so this
//! uses no key and no login instead: the watch page of a live video holds a
//! token for its chat, and the same endpoint the page polls (`get_live_chat`)
//! hands over the messages and the token for the next poll. None of that is
//! documented or promised, so it can break whenever YouTube changes it; as
//! with TikTok, everything that depends on it is in this file.
//!
//! Checked by hand against a live stream: the page layout and the chat
//! response as described in the parsers below, the moderator badge (an icon
//! of type `MODERATOR`) and the member badge (a custom picture, no icon).
//! Not seen yet: the streamer's own badge (assumed to be icon type `OWNER`),
//! custom emoji (assumed `isCustomEmoji`) and Super Chat messages, which are
//! not read.

use std::collections::{HashSet, VecDeque};
use std::time::Duration;

use serde_json::{Value, json};
use url::Url;

use super::connection::{
    ChatConnector, ChatFeed, ConnectError, ConnectFailure, FeedItem, ReconnectingSource,
};
use super::role::Role;
use super::sender::Sender;
use super::source::ChatMessage;
use crate::text::sanitize_text;
use crate::youtube::VideoId;

const SITE: &str = "https://www.youtube.com/";

/// How long to wait for YouTube's answer.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// A page or answer larger than this is not read any further. The watch page
/// is about 1.3 MB.
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Longest chat message kept, in characters; only bounds unexpected data.
const MAX_COMMENT_CHARS: usize = 500;

/// The wait between two polls is what YouTube asks for (10 seconds when this
/// was checked), kept within these limits.
const MIN_POLL: Duration = Duration::from_secs(1);
const MAX_POLL: Duration = Duration::from_secs(30);
const DEFAULT_POLL: Duration = Duration::from_secs(5);

/// How many message IDs are remembered to drop repeats.
const SEEN_LIMIT: usize = 2000;

/// Chat source for one live video, reconnecting as needed.
pub type YoutubeSource = ReconnectingSource<YoutubeConnector>;

/// Creates the chat source for the live video `video` (an 11-character ID).
pub fn youtube_source(video: &str) -> YoutubeSource {
    ReconnectingSource::new(YoutubeConnector::new(video))
}

pub struct YoutubeConnector {
    video: String,
    client: Option<reqwest::Client>,
    /// `None` only if the constant address could not be read, which makes
    /// every try fail at once.
    base: Option<Url>,
    min_poll: Duration,
}

impl YoutubeConnector {
    pub fn new(video: &str) -> Self {
        match Url::parse(SITE) {
            Ok(base) => Self::with_base_url(video, base, MIN_POLL),
            Err(_) => Self {
                video: video.to_owned(),
                client: None,
                base: None,
                min_poll: MIN_POLL,
            },
        }
    }

    /// Like `new`, with another address to ask and another shortest wait
    /// between polls (for tests).
    pub fn with_base_url(video: &str, base: Url, min_poll: Duration) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION")
            ))
            .https_only(base.scheme() == "https")
            .redirect(reqwest::redirect::Policy::none());
        let client = if is_loopback(&base) {
            // Never send local traffic through a configured proxy.
            client.no_proxy()
        } else {
            client
        };
        Self {
            video: video.to_owned(),
            client: client.build().ok(),
            base: Some(base),
            min_poll,
        }
    }
}

impl ChatConnector for YoutubeConnector {
    type Feed = YoutubeFeed;

    fn channel(&self) -> &str {
        &self.video
    }

    async fn connect(&mut self) -> Result<YoutubeFeed, ConnectError> {
        let id = VideoId::parse(&self.video).map_err(|_| other("not a video ID"))?;
        let client = self
            .client
            .clone()
            .ok_or_else(|| other("cannot build the HTTP client"))?;
        let base = self.base.clone().ok_or_else(|| other("bad address"))?;
        let url = base
            .join(&format!("watch?v={id}"))
            .map_err(|_| other("bad address"))?;
        let response = client
            .get(url)
            .header("Accept-Language", "en")
            .send()
            .await
            .map_err(request_failed)?;
        check_status(response.status())?;
        let page = read_capped(response).await?;
        let start = parse_watch_page(&String::from_utf8_lossy(&page))?;
        Ok(YoutubeFeed {
            client,
            base,
            start,
            first: true,
            finished: false,
            wait: None,
            pending: VecDeque::new(),
            seen: Seen::default(),
            min_poll: self.min_poll,
        })
    }
}

/// What the watch page gives for polling the chat.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WatchStart {
    /// The key and version the site's own scripts send; public, and not a
    /// key of the user's.
    key: String,
    version: String,
    continuation: String,
}

pub struct YoutubeFeed {
    client: reqwest::Client,
    base: Url,
    start: WatchStart,
    /// The first answer holds the chat as it was when we came: not read, or
    /// old commands would run again.
    first: bool,
    /// The last answer has come: once its messages are read, the feed ends.
    finished: bool,
    wait: Option<Duration>,
    pending: VecDeque<ChatMessage>,
    seen: Seen,
    min_poll: Duration,
}

impl ChatFeed for YoutubeFeed {
    async fn next(&mut self) -> FeedItem {
        loop {
            if let Some(message) = self.pending.pop_front() {
                return FeedItem::Message(message);
            }
            if self.finished {
                return FeedItem::Ended;
            }
            if let Some(wait) = self.wait.take() {
                tokio::time::sleep(wait).await;
            }
            match self.poll().await {
                Ok(true) => {}
                Ok(false) => return FeedItem::Ended,
                Err(error) => {
                    tracing::warn!(detail = %error.detail, "YouTube chat poll failed");
                    return FeedItem::Ended;
                }
            }
        }
    }
}

impl YoutubeFeed {
    /// One poll. `Ok(false)`: the chat has ended (the answer holds no chat).
    async fn poll(&mut self) -> Result<bool, ConnectError> {
        let mut url = self
            .base
            .join("youtubei/v1/live_chat/get_live_chat")
            .map_err(|_| other("bad address"))?;
        url.query_pairs_mut().append_pair("key", &self.start.key);
        let body = json!({
            "context": {"client": {
                "clientName": "WEB",
                "clientVersion": self.start.version,
                "hl": "en",
            }},
            "continuation": self.start.continuation,
        });
        let response = self
            .client
            .post(url)
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(request_failed)?;
        check_status(response.status())?;
        let bytes = read_capped(response).await?;
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| other("unexpected answer"))?;
        let Some(poll) = parse_poll(&value, self.min_poll) else {
            return Ok(false);
        };
        let first = std::mem::replace(&mut self.first, false);
        for (id, message) in poll.messages {
            if self.seen.insert(id) && !first {
                self.pending.push_back(message);
            }
        }
        // Without a token this was the last answer; what it held is still read.
        let Some((continuation, wait)) = poll.next else {
            self.finished = true;
            return Ok(true);
        };
        self.start.continuation = continuation;
        self.wait = Some(wait);
        Ok(true)
    }
}

/// The IDs of the messages seen lately.
#[derive(Default)]
struct Seen {
    ids: HashSet<String>,
    order: VecDeque<String>,
}

impl Seen {
    /// True when `id` is new.
    fn insert(&mut self, id: String) -> bool {
        if !self.ids.insert(id.clone()) {
            return false;
        }
        self.order.push_back(id);
        while self.order.len() > SEEN_LIMIT {
            if let Some(old) = self.order.pop_front() {
                self.ids.remove(&old);
            }
        }
        true
    }
}

fn other(detail: &str) -> ConnectError {
    ConnectError {
        kind: ConnectFailure::Other,
        detail: detail.to_owned(),
    }
}

fn request_failed(error: reqwest::Error) -> ConnectError {
    ConnectError {
        kind: ConnectFailure::Network,
        detail: sanitize_text(&error.to_string()),
    }
}

fn check_status(status: reqwest::StatusCode) -> Result<(), ConnectError> {
    if status.is_success() {
        return Ok(());
    }
    let kind = match status.as_u16() {
        429 | 403 => ConnectFailure::Blocked,
        500..=599 => ConnectFailure::Network,
        _ => ConnectFailure::Other,
    };
    Err(ConnectError {
        kind,
        detail: format!("HTTP {}", status.as_u16()),
    })
}

/// The body, up to `MAX_BODY_BYTES`; a larger one is an error.
async fn read_capped(mut response: reqwest::Response) -> Result<Vec<u8>, ConnectError> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(request_failed)? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BODY_BYTES {
            return Err(other("answer too large"));
        }
    }
    Ok(body)
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Reads what is needed to poll the chat from the watch page: the chat's
/// first token from the page's own data (`var ytInitialData = {...}`), and the
/// key and version of the site's scripts. A page without a live chat, or with
/// the chat of a finished stream, means the video is not live.
fn parse_watch_page(page: &str) -> Result<WatchStart, ConnectError> {
    const DATA: &str = "var ytInitialData = ";
    let start = page
        .find(DATA)
        .ok_or_else(|| other("the page has no data"))?
        + DATA.len();
    let data: Value = serde_json::Deserializer::from_str(&page[start..])
        .into_iter::<Value>()
        .next()
        .and_then(Result::ok)
        .ok_or_else(|| other("the page data cannot be read"))?;

    let not_live = |detail: &str| ConnectError {
        kind: ConnectFailure::NotLive,
        detail: detail.to_owned(),
    };
    let chat = data
        .pointer("/contents/twoColumnWatchNextResults/conversationBar/liveChatRenderer")
        .ok_or_else(|| not_live("the video has no live chat"))?;
    if chat.get("isReplay").and_then(Value::as_bool) == Some(true) {
        return Err(not_live("the stream is over"));
    }
    let continuation = chat
        .pointer("/continuations/0/reloadContinuationData/continuation")
        .and_then(Value::as_str)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| other("the chat has no token"))?
        .to_owned();
    let key = quoted_after(page, "\"INNERTUBE_API_KEY\":\"")
        .filter(|key| {
            is_token(key, 20..=80, |c| {
                c.is_ascii_alphanumeric() || matches!(c, '_' | '-')
            })
        })
        .ok_or_else(|| other("the page has no usable key"))?;
    let version = quoted_after(page, "\"INNERTUBE_CLIENT_VERSION\":\"")
        .or_else(|| quoted_after(page, "\"clientVersion\":\""))
        .filter(|version| is_token(version, 3..=32, |c| c.is_ascii_digit() || c == '.'))
        .ok_or_else(|| other("the page has no usable version"))?;
    Ok(WatchStart {
        key,
        version,
        continuation,
    })
}

/// The text between `marker` and the next double quote.
fn quoted_after(page: &str, marker: &str) -> Option<String> {
    let start = page.find(marker)? + marker.len();
    let end = start + page[start..].find('"')?;
    Some(page[start..end].to_owned())
}

fn is_token(
    text: &str,
    len: std::ops::RangeInclusive<usize>,
    allowed: impl Fn(char) -> bool,
) -> bool {
    len.contains(&text.len()) && text.chars().all(allowed)
}

/// What one poll brought.
#[derive(Debug, PartialEq)]
struct Poll {
    /// `(message ID, message)`, oldest first.
    messages: Vec<(String, ChatMessage)>,
    /// The token for the next poll and how long to wait first; `None` when
    /// the chat is over.
    next: Option<(String, Duration)>,
}

/// `None` when the answer holds no chat at all (the stream ended). The wait
/// before the next poll is at least `min_poll`.
fn parse_poll(value: &Value, min_poll: Duration) -> Option<Poll> {
    let chat = value.pointer("/continuationContents/liveChatContinuation")?;
    let messages = chat
        .get("actions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|action| action.pointer("/addChatItemAction/item/liveChatTextMessageRenderer"))
        .filter_map(map_message)
        .collect();
    let next = chat
        .get("continuations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|continuation| {
            let data = continuation
                .get("invalidationContinuationData")
                .or_else(|| continuation.get("timedContinuationData"))?;
            let token = data.get("continuation")?.as_str()?;
            let wait = data
                .get("timeoutMs")
                .and_then(Value::as_u64)
                .map_or(DEFAULT_POLL, Duration::from_millis)
                .clamp(min_poll, MAX_POLL);
            Some((token.to_owned(), wait))
        });
    Some(Poll { messages, next })
}

/// Builds a message, or `None` when the sender cannot be identified (which
/// would let them dodge per-user limits).
fn map_message(renderer: &Value) -> Option<(String, ChatMessage)> {
    let id = renderer.get("id")?.as_str()?.to_owned();
    let channel = renderer.get("authorExternalChannelId")?.as_str()?;
    let channel_ok = !channel.is_empty()
        && channel.len() <= 64
        && channel
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
    if id.is_empty() || !channel_ok {
        return None;
    }
    let name = renderer
        .pointer("/authorName/simpleText")
        .and_then(Value::as_str)
        .map(sanitize_text)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "YouTube viewer".to_owned());
    let text: String = renderer
        .pointer("/message/runs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(run_text)
        .collect::<String>()
        .chars()
        .take(MAX_COMMENT_CHARS)
        .collect();
    let badges = renderer
        .get("authorBadges")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    Some((
        id,
        ChatMessage {
            // The prefix keeps a YouTube channel apart from any other platform.
            sender: Sender::new(format!("youtube:{channel}"), name, role_of(badges)),
            text,
        },
    ))
}

/// The text of one piece of a message: plain text, a link's text, an emoji
/// (the character itself, or `:shortcut:` for a custom one).
fn run_text(run: &Value) -> String {
    if let Some(text) = run.get("text").and_then(Value::as_str) {
        return text.to_owned();
    }
    let Some(emoji) = run.get("emoji") else {
        return String::new();
    };
    let custom = emoji.get("isCustomEmoji").and_then(Value::as_bool) == Some(true);
    let text = if custom {
        emoji.pointer("/shortcuts/0")
    } else {
        emoji.get("emojiId")
    };
    text.and_then(Value::as_str).unwrap_or_default().to_owned()
}

/// The streamer and moderators are moderators; members of the channel are
/// subscribers; everyone else counts as a follower, because YouTube shows no
/// follower status and the default minimum role is follower.
fn role_of(badges: &[Value]) -> Role {
    let mut member = false;
    for badge in badges {
        let Some(badge) = badge.get("liveChatAuthorBadgeRenderer") else {
            continue;
        };
        match badge.pointer("/icon/iconType").and_then(Value::as_str) {
            Some("MODERATOR" | "OWNER") => return Role::Moderator,
            Some(_) => {}
            // Membership badges are a custom picture and carry no icon.
            None => member |= badge.get("customThumbnail").is_some(),
        }
    }
    if member {
        Role::Subscriber
    } else {
        Role::Follower
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    const VIDEO: &str = "HFSuEKR1b0w";

    fn text_message(id: &str, channel: &str, name: &str, text: &str) -> Value {
        json!({"addChatItemAction": {"item": {"liveChatTextMessageRenderer": {
            "id": id,
            "authorExternalChannelId": channel,
            "authorName": {"simpleText": name},
            "message": {"runs": [{"text": text}]},
        }}}})
    }

    fn answer(actions: &[Value], continuation: Option<(&str, u64)>) -> Value {
        let continuations = continuation.map_or_else(Vec::new, |(token, ms)| {
            vec![json!({"invalidationContinuationData": {"continuation": token, "timeoutMs": ms}})]
        });
        json!({"continuationContents": {"liveChatContinuation": {
            "actions": actions,
            "continuations": continuations,
        }}})
    }

    fn watch_page(chat: Option<Value>) -> String {
        let mut data = json!({"contents": {"twoColumnWatchNextResults": {"conversationBar": {}}}});
        if let Some(chat) = chat {
            data["contents"]["twoColumnWatchNextResults"]["conversationBar"]["liveChatRenderer"] =
                chat;
        }
        format!(
            r#"<html><script>ytcfg.set({{"INNERTUBE_API_KEY":"AIzaSyFakeFakeFakeFakeFakeFakeFake12345","INNERTUBE_CLIENT_VERSION":"2.20261116.00.00"}});</script><script>var ytInitialData = {data};</script></html>"#
        )
    }

    fn live_chat() -> Value {
        json!({"continuations": [{"reloadContinuationData": {"continuation": "FIRST"}}]})
    }

    // --- the page ---

    #[test]
    fn the_watch_page_of_a_live_video_gives_what_polling_needs() {
        let start = parse_watch_page(&watch_page(Some(live_chat()))).unwrap();
        assert_eq!(start.continuation, "FIRST");
        assert_eq!(start.version, "2.20261116.00.00");
        assert_eq!(start.key, "AIzaSyFakeFakeFakeFakeFakeFakeFake12345");
    }

    #[test]
    fn a_page_without_a_live_chat_means_not_live_and_other_problems_are_not() {
        let kind = |page: &str| parse_watch_page(page).unwrap_err().kind;
        assert_eq!(kind(&watch_page(None)), ConnectFailure::NotLive);
        let replay = json!({"isReplay": true, "continuations": [{"reloadContinuationData": {"continuation": "X"}}]});
        assert_eq!(kind(&watch_page(Some(replay))), ConnectFailure::NotLive);

        assert_eq!(kind("<html>nothing</html>"), ConnectFailure::Other);
        assert_eq!(kind("var ytInitialData = {broken"), ConnectFailure::Other);
        let no_token = json!({"continuations": []});
        assert_eq!(kind(&watch_page(Some(no_token))), ConnectFailure::Other);
        // A key or version that is not made of the usual characters is never used.
        let page = watch_page(Some(live_chat()));
        assert_eq!(
            kind(&page.replace("AIzaSyFakeFake", "AIza&evil=1&Fake")),
            ConnectFailure::Other
        );
        assert_eq!(
            kind(&page.replace("2.20261116.00.00", "2.0;alert(1)")),
            ConnectFailure::Other
        );
    }

    // --- one answer ---

    #[test]
    fn an_answer_gives_the_messages_and_the_next_poll() {
        let value = answer(
            &[
                text_message("m1", "UCaaa", "Sari", "!yt dQw4w9WgXcQ"),
                json!({"addChatItemAction": {"item": {"liveChatViewerEngagementMessageRenderer": {}}}}),
                json!({"removeChatItemAction": {"targetItemId": "m0"}}),
                text_message("m2", "UCbbb", "Budi", "hello"),
            ],
            Some(("NEXT", 10_000)),
        );
        let poll = parse_poll(&value, MIN_POLL).unwrap();
        let texts: Vec<_> = poll
            .messages
            .iter()
            .map(|(id, m)| (id.as_str(), m.text.as_str()))
            .collect();
        assert_eq!(texts, [("m1", "!yt dQw4w9WgXcQ"), ("m2", "hello")]);
        assert_eq!(
            poll.next,
            Some(("NEXT".to_owned(), Duration::from_secs(10)))
        );
        assert_eq!(poll.messages[0].1.sender.id, "youtube:UCaaa");
        assert_eq!(poll.messages[0].1.sender.name, "Sari");
    }

    #[test]
    fn the_wait_between_polls_is_what_youtube_asks_within_limits() {
        let wait = |ms: Option<u64>| {
            let mut data = json!({"continuation": "N"});
            if let Some(ms) = ms {
                data["timeoutMs"] = json!(ms);
            }
            let value = json!({"continuationContents": {"liveChatContinuation": {
                "continuations": [{"timedContinuationData": data}]}}});
            parse_poll(&value, MIN_POLL).unwrap().next.unwrap().1
        };
        assert_eq!(wait(Some(7_000)), Duration::from_secs(7));
        assert_eq!(wait(Some(0)), MIN_POLL);
        assert_eq!(wait(Some(3_600_000)), MAX_POLL);
        assert_eq!(wait(None), DEFAULT_POLL);
    }

    #[test]
    fn an_answer_with_no_chat_or_no_next_token_means_the_chat_is_over() {
        assert_eq!(parse_poll(&json!({}), MIN_POLL), None);
        assert_eq!(parse_poll(&json!({"error": {"code": 400}}), MIN_POLL), None);
        let last = answer(&[text_message("m1", "UCaaa", "Sari", "bye")], None);
        let poll = parse_poll(&last, MIN_POLL).unwrap();
        assert_eq!(poll.messages.len(), 1);
        assert_eq!(poll.next, None);
    }

    // --- one message ---

    fn mapped(renderer: Value) -> Option<ChatMessage> {
        map_message(&renderer).map(|(_, message)| message)
    }

    #[test]
    fn a_message_is_text_links_and_emoji_as_text() {
        let message = mapped(json!({
            "id": "m", "authorExternalChannelId": "UCaaa", "authorName": {"simpleText": "Sari"},
            "message": {"runs": [
                {"text": "!yt "},
                {"text": "youtu.be/dQw4w9WgXcQ", "navigationEndpoint": {}},
                {"emoji": {"emojiId": "😀", "shortcuts": [":grinning:"]}},
                {"emoji": {"emojiId": "UCx/abc", "shortcuts": [":custom:"], "isCustomEmoji": true}},
                {"emoji": {}},
                {"somethingNew": 1},
            ]},
        }))
        .unwrap();
        assert_eq!(message.text, "!yt youtu.be/dQw4w9WgXcQ😀:custom:");
    }

    #[test]
    fn a_sender_that_cannot_be_identified_is_dropped() {
        let base = |channel: Value, id: Value| json!({"id": id, "authorExternalChannelId": channel, "authorName": {"simpleText": "X"}, "message": {"runs": []}});
        assert!(mapped(base(json!("UCaaa"), json!("m"))).is_some());
        for (channel, id) in [
            (json!(""), json!("m")),
            (json!(null), json!("m")),
            (json!("UC/../x"), json!("m")),
            (json!("U".repeat(65)), json!("m")),
            (json!("UCaaa"), json!("")),
            (json!("UCaaa"), json!(null)),
        ] {
            assert!(
                mapped(base(channel.clone(), id.clone())).is_none(),
                "{channel} {id}"
            );
        }
    }

    #[test]
    fn names_and_texts_are_cleaned_and_bounded() {
        let message = mapped(json!({
            "id": "m", "authorExternalChannelId": "UCaaa",
            "authorName": {"simpleText": "  Sa\nri \u{7} "},
            "message": {"runs": [{"text": "x".repeat(MAX_COMMENT_CHARS + 50)}]},
        }))
        .unwrap();
        assert_eq!(message.sender.name, "Sa ri");
        assert_eq!(message.text.chars().count(), MAX_COMMENT_CHARS);
        let unnamed =
            mapped(json!({"id": "m", "authorExternalChannelId": "UCaaa", "message": {"runs": []}}))
                .unwrap();
        assert_eq!(unnamed.sender.name, "YouTube viewer");
    }

    #[test]
    fn badges_decide_the_role() {
        let badge = |icon: Option<&str>, picture: bool| {
            let mut renderer = json!({"tooltip": "whatever"});
            if let Some(icon) = icon {
                renderer["icon"] = json!({"iconType": icon});
            }
            if picture {
                renderer["customThumbnail"] = json!({"thumbnails": []});
            }
            json!({"liveChatAuthorBadgeRenderer": renderer})
        };
        assert_eq!(role_of(&[]), Role::Follower);
        assert_eq!(role_of(&[badge(Some("VERIFIED"), false)]), Role::Follower);
        assert_eq!(role_of(&[badge(None, true)]), Role::Subscriber);
        assert_eq!(role_of(&[badge(Some("MODERATOR"), false)]), Role::Moderator);
        assert_eq!(role_of(&[badge(Some("OWNER"), false)]), Role::Moderator);
        // The highest standing wins, in any order; strange badges are ignored.
        assert_eq!(
            role_of(&[badge(None, true), badge(Some("MODERATOR"), false)]),
            Role::Moderator
        );
        assert_eq!(
            role_of(&[json!({"other": 1}), badge(None, false)]),
            Role::Follower
        );
    }

    #[test]
    fn remembered_ids_drop_repeats_and_are_forgotten_in_time() {
        let mut seen = Seen::default();
        assert!(seen.insert("a".to_owned()));
        assert!(!seen.insert("a".to_owned()));
        for n in 0..SEEN_LIMIT {
            seen.insert(format!("id{n}"));
        }
        assert!(seen.insert("a".to_owned()), "the oldest has been forgotten");
        assert_eq!(seen.ids.len(), SEEN_LIMIT);
    }

    // --- the whole thing, against a stand-in for YouTube ---

    /// Answers each connection with the next canned response, in order, and
    /// keeps the head and body of every request.
    async fn serve(responses: Vec<String>) -> (Url, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let handle = tokio::spawn(async move {
            let mut requests = Vec::new();
            for response in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut data = Vec::new();
                let mut buf = [0u8; 4096];
                let wanted = loop {
                    let n = socket.read(&mut buf).await.unwrap();
                    if n == 0 {
                        break data.len();
                    }
                    data.extend_from_slice(&buf[..n]);
                    if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&data[..end]).to_ascii_lowercase();
                        let length = head
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .and_then(|value| value.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        break end + 4 + length;
                    }
                };
                while data.len() < wanted {
                    let n = socket.read(&mut buf).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n]);
                }
                requests.push(String::from_utf8_lossy(&data).into_owned());
                socket.write_all(response.as_bytes()).await.unwrap();
                let _ = socket.shutdown().await;
            }
            requests
        });
        (url, handle)
    }

    fn http(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    async fn next_text(feed: &mut YoutubeFeed) -> Option<String> {
        match feed.next().await {
            FeedItem::Message(message) => Some(message.text),
            FeedItem::Ended => None,
            FeedItem::Status(_) => panic!("unexpected status"),
        }
    }

    #[tokio::test]
    async fn the_feed_skips_the_chat_that_was_there_and_reads_what_comes_after() {
        let first = answer(
            &[text_message("old", "UCold", "Old", "!yt old")],
            Some(("T2", 0)),
        );
        let second = answer(
            &[
                text_message("old", "UCold", "Old", "!yt old"), // a repeat
                text_message("n1", "UCaaa", "Sari", "!yt new1"),
                text_message("n2", "UCbbb", "Budi", "!yt new2"),
            ],
            Some(("T3", 0)),
        );
        let last = answer(&[text_message("n3", "UCaaa", "Sari", "!yt new3")], None);
        let (base, server) = serve(vec![
            http(&watch_page(Some(live_chat()))),
            http(&first.to_string()),
            http(&second.to_string()),
            http(&last.to_string()),
        ])
        .await;

        let mut connector = YoutubeConnector::with_base_url(VIDEO, base, Duration::ZERO);
        let mut feed = connector.connect().await.unwrap();
        assert_eq!(next_text(&mut feed).await.as_deref(), Some("!yt new1"));
        assert_eq!(next_text(&mut feed).await.as_deref(), Some("!yt new2"));
        assert_eq!(next_text(&mut feed).await.as_deref(), Some("!yt new3"));
        assert_eq!(
            next_text(&mut feed).await,
            None,
            "no token: the chat is over"
        );

        let requests = server.await.unwrap();
        assert!(
            requests[0].starts_with(&format!("GET /watch?v={VIDEO} HTTP/1.1")),
            "{}",
            requests[0]
        );
        let lower = requests[0].to_ascii_lowercase();
        assert!(
            lower.contains("user-agent: stream-spectre/") && !lower.contains("cookie"),
            "{lower}"
        );
        // Each poll sends the token of the one before.
        assert!(
            requests[1].starts_with("POST /youtubei/v1/live_chat/get_live_chat?key=AIzaSyFake")
        );
        assert!(
            requests[1].contains(r#""continuation":"FIRST""#),
            "{}",
            requests[1]
        );
        assert!(
            requests[2].contains(r#""continuation":"T2""#),
            "{}",
            requests[2]
        );
        assert!(
            requests[3].contains(r#""continuation":"T3""#),
            "{}",
            requests[3]
        );
    }

    #[tokio::test]
    async fn a_video_that_is_not_live_waits_and_trouble_with_youtube_is_not_that() {
        let (base, server) = serve(vec![http(&watch_page(None))]).await;
        let error = YoutubeConnector::with_base_url(VIDEO, base, Duration::ZERO)
            .connect()
            .await
            .err()
            .unwrap();
        assert_eq!(error.kind, ConnectFailure::NotLive);
        server.await.unwrap();

        let (base, server) = serve(vec![
            "HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_owned(),
        ])
        .await;
        let error = YoutubeConnector::with_base_url(VIDEO, base, Duration::ZERO)
            .connect()
            .await
            .err()
            .unwrap();
        assert_eq!(error.kind, ConnectFailure::Blocked);
        server.await.unwrap();

        // Something that is not a video ID never reaches the network.
        let (base, _server) = serve(vec![]).await;
        let error = YoutubeConnector::with_base_url("../x", base, Duration::ZERO)
            .connect()
            .await
            .err()
            .unwrap();
        assert_eq!(error.kind, ConnectFailure::Other);
    }

    #[tokio::test]
    async fn a_failing_poll_ends_the_connection_so_the_supervisor_reconnects() {
        let first = answer(&[], Some(("T2", 0)));
        let (base, server) = serve(vec![
            http(&watch_page(Some(live_chat()))),
            http(&first.to_string()),
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_owned(),
        ])
        .await;
        let mut feed = YoutubeConnector::with_base_url(VIDEO, base, Duration::ZERO)
            .connect()
            .await
            .unwrap();
        assert_eq!(next_text(&mut feed).await, None);
        server.await.unwrap();
    }
}
