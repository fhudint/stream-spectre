# WebSocket events

The overlay and the dashboard receive the same stream of JSON events. The
Rust definitions in [`src/app/events.rs`](../src/app/events.rs) are the source
of truth; this page describes them for people writing clients.

## Endpoints and access

| Endpoint | Purpose |
|----------|---------|
| `GET /ws/overlay` | WebSocket for the OBS overlay page |
| `GET /ws/dashboard` | WebSocket for the dashboard page |
| `GET /api/state` | The current state, same shape as `hello`'s `data` |
| `GET /api/i18n` | UI texts in the configured language |
| `POST /api/queue/{id}/approve` | Approve a pending request (`204`) |
| `POST /api/queue/{id}/reject` | Remove a pending or approved request (`204`) |
| `POST /api/skip` | Skip the playing video: `{"skipped": true or false}` |
| `POST /api/volume` | Body `{"volume": 0-100}`; answers `{"volume": n}` with the volume in effect |
| `PATCH /api/settings` | Change some moderation settings, keep the rest; answers the new state |
| `PATCH /api/blacklist` | Replace some blacklist lists (`video_ids`, `channels`, `title_words`), keep the rest |
| `PATCH /api/dashboard` | Change dashboard settings (`chat_history`), keep the rest |
| `PATCH /api/overlay` | Change the overlay's look (`{"theme": "pixel"}`); an unknown theme is refused |
| `PATCH /api/commands` | Change the chat commands: `{"prefix": "!", "commands": [{"name": "v", "source": "youtube"}, {"name": "vt", "source": "tiktok"}]}`. `source` is `youtube` or `tiktok`. Either field may be left out; `commands` is always the whole list. Takes effect for the next chat message. `400` for an empty list, more than 8, a name used twice (capitals do not count), a name with anything but ASCII letters, digits and `_`, an unknown source, or a prefix that is empty, longer than 4 or has spaces. Answers the new state. |
| `PATCH /api/chat/tiktok` | Body `{"username": "name", "enabled": true}`, either or both: set the TikTok username (an empty string stops reading chat) and switch the chat on or off; the chat follows without a restart. Letters, digits, `_` and `.`, at most 64; a leading `@` and spaces are dropped. Answers the new state. |
| `PATCH /api/chat/youtube` | Body `{"video": "link or ID", "enabled": true}`, either or both: set the YouTube live whose chat is read (a link or an 11-character ID, kept as the ID; an empty string stops reading chat) and switch the chat on or off. Something that is not a video is refused. Answers the new state. |
| `PUT /api/playback/pause` | Body `{"paused": true or false}`: pauses or resumes the video that is playing. Answers the new state. Works for YouTube and TikTok videos alike. `409` with `"code": "nothing_playing"` when no video is playing. |
| `PUT /api/chat/connection` | Body `{"connected": true or false}`: disconnects the chat (`false`) or connects it again (`true`), for as long as the program runs; not saved. Answers the new state. |
| `GET /api/account` | What is set, never the secrets: `{"youtube_key": {"set", "source": "none"/"environment"/"saved", "last4"}, "dashboard_token": {"set"}, "editable"}`. `last4` is the key's last four characters, or `null` (no key, or one too short to show any part of). `editable` is false for a connection that does not come from the computer the program runs on. |
| `PUT /api/account/youtube-key` | Body `{"key": "..."}`. Checks the key with one YouTube request (1 quota unit), saves it in the secrets file and starts using it at once. Answers like `GET /api/account` plus `"checked": "valid"` or `"quota_exhausted"` (the key is real but today's quota is used up). Errors carry a `code`: `key_empty`, `key_bad_format` (400), `key_rejected` (400, YouTube refused it), `cannot_check` (502, could not reach YouTube), `not_saved` (500), `loopback_only` (403). The key is never sent back. |
| `DELETE /api/account/youtube-key` | Forgets the saved key and goes back to the one in the environment, if any. Answers like `GET /api/account`. |
| `GET /dashboard/`, `GET /overlay/` | The pages themselves |

Changing a credential (`PUT` and `DELETE` on `/api/account/youtube-key`) is only
allowed for a connection from the computer the program runs on (loopback, including
an IPv4 address mapped into IPv6), even when the dashboard is opened to the
network with a token; any other connection gets `403` with `"code": "loopback_only"`.
Reading `GET /api/account` is allowed to anyone with the token.

The two sockets carry the events below. The overlay socket only receives
what changes the screen (`play`, `skip`, `stop`, `volume`, `overlay_theme`, `playback_paused`) and a trimmed
`hello` without waiting requests, blacklist, chat status or chat messages; the dashboard
socket receives everything. Dashboard actions go through the REST API.

Errors from the API are JSON: `{"error": "..."}` with `400` (bad or invalid
input, nothing changed), `404` (no such queue entry), `409` (the entry is not
in a state that allows this), `401` or `403` (see below), or `413` (body over
64 KiB). A `PATCH` with an unknown field or a value that fails validation is
refused as a whole.

### Who may connect

- **Token.** When `DASHBOARD_TOKEN` is set, everything under `/api/` and
  `/ws/` needs it, as `Authorization: Bearer <token>` or a `?token=<token>`
  query parameter (browsers cannot set headers on a WebSocket). The pages
  themselves contain nothing secret and are served without it; open the
  dashboard as `/dashboard/#token=...` (a fragment is never sent to the
  server) and the overlay as `/overlay/?token=...`. Without a configured
  token the server only listens on the loopback address and needs none.
- **Host.** On a loopback address the `Host` header must be `127.0.0.1`,
  `localhost` or `[::1]` with the server's port. This stops other websites
  from reaching the server through DNS rebinding. Reaching a loopback server
  through a tunnel or reverse proxy therefore does not work; listen on a
  network address (which requires a token) instead.
  Browsers should use the name `localhost` for the overlay rather than
  `127.0.0.1`: YouTube's embedded player was seen refusing every video (error
  150) when the page's address was a bare IP address.
- **Origin.** If a browser sends an `Origin` header it must be this server
  itself, so other websites cannot make your browser call the API. Scripts
  that send no `Origin` are unaffected.
- **Limits.** At most 32 WebSocket connections at once, client messages up to
  4 KiB, and a client that cannot take a message within 10 seconds is dropped.
  The server pings idle connections every 30 seconds.

## Envelope

Every event is one JSON text message:

```json
{ "v": 1, "seq": 12, "ts": 1700000000000, "type": "volume", "data": { "volume": 30 } }
```

| Field  | Meaning |
|--------|---------|
| `v`    | Protocol version, currently `1`. Bumped when a change would break old clients. |
| `seq`  | 1, 2, 3, ... in publishing order, shared by all clients. |
| `ts`   | Milliseconds since the Unix epoch (`new Date(ts)` in JavaScript). |
| `type` | Event name, see below. |
| `data` | The event's payload, always an object. |

Unknown `type` values and unknown fields must be ignored, so newer servers do
not break older clients.

## Connecting and staying in sync

1. Connect, and the server sends a `hello` event with the complete current
   state. Its `data.seq` is the sequence number of the last event the snapshot
   already includes.
2. Apply the events that follow, ignoring any with `seq <= hello.seq`.
3. If the connection drops, reconnect and start again at step 1.
4. If a client reads too slowly and misses events, the server sends a new
   `hello` instead of the missing events. Treat every `hello` as "replace
   everything you know", not only the first one.

Because `queue_updated` always carries the whole queue, a client never has to
patch its own copy of the queue.

An overlay that is reloaded (for example when OBS refreshes the browser
source) receives `hello`. If `data.queue.now_playing` is set, it should start
that video again from the beginning.

## Events from the server

### `hello`

Sent once to each client, first.

```json
{ "type": "hello", "data": {
    "server_version": "0.1.0",
    "language": "id",
    "seq": 41,
    "settings": { "moderation": { ... }, "blacklist": { ... } },
    "volume": 50,
    "queue": { "pending": [], "approved": [], "now_playing": null },
    "chat": { "state": "connected", "channel": "streamer", "text": "..." },
    "chat_log": [ ChatLine ],
    "history": [ HistoryEntry ],
    "stats": { "since_ms": 1700000000000, "received": 30, "approved": 15, "rejected": 9 },
    "playback_paused": false
} }
```

`chat` is `null` until the chat connection has reported its first status.
`chat_log` holds the most recent chat messages, oldest first, at most
`settings.dashboard.chat_history` of them (see [`chat_message`](#chat_message)).
`history` holds every request kept, oldest first (see
[`history_entry`](#history_entry)). `stats` is described under
[`stats_updated`](#stats_updated). The overlay's `hello` carries `stats` with
every number zero, like its empty `chat_log` and `history`.

### `queue_updated`

The queue changed. `data` is the whole queue:

```json
{ "type": "queue_updated", "data": {
    "pending":     [ Item ],
    "approved":    [ Item ],
    "now_playing": Item | null
} }
```

- `pending`: waiting for a moderator, oldest first.
- `approved`: approved and waiting to play, in play order.
- `now_playing`: the video on screen, if any.

**Item**

```json
{
  "id": 7,
  "source": "youtube",
  "video_id": "dQw4w9WgXcQ",
  "title": "Song",
  "channel": "Channel name",
  "duration_secs": 213,
  "thumbnail_url": "https://i.ytimg.com/vi/dQw4w9WgXcQ/mqdefault.jpg",
  "requested_by": { "name": "Budi", "role": "subscriber" },
  "requested_at_ms": 1700000000123
}
```

`id` is unique while the server runs. `source` is `"youtube"` or `"tiktok"`
(an item from a server that predates TikTok has no `source`, which means
YouTube). `video_id` is in the source's own form and always safe to use in a
URL: exactly 11 characters from `A-Z a-z 0-9 _ -` for YouTube, 15 to 20 digits
for TikTok.

A **TikTok** item carries little, because TikTok gives nothing more without a
developer account: `title` is the account (`"@name"`) when it is known (from the
link, or looked up for a bare ID on a best-effort basis), otherwise the ID; `channel` is `"TikTok"`; `duration_secs` is `0`, meaning not
known; `thumbnail_url` is an empty string. Such an item always waits for a
moderator, even when automatic approval is on, because nothing about it
(length, age, whether it can be embedded) has been checked. The overlay shows
it in TikTok's own embedded player. That player tells the overlay when the video
ends (state 0), and the overlay sends `ended`; when it cannot play a video
(`onPlayerError`, for example 1001 for one that does not exist) the overlay
sends `player_error` with that code, and the video is skipped with a notice. A
video the player has not started playing within 15 seconds gets `player_error`
with the code `timeout` (TikTok does not play videos with a product link in a
browser, and says nothing then). A video that runs but never says it is over
stays until a moderator skips it. A TikTok
item can be paused and resumed (the overlay sends the player `pause` and `play`).
Its `duration_secs` stays `0`: the length is only known to the player, which tells
the overlay, which tells the server in `progress` (see [`playback`](#playback)).
The player starts muted: the overlay asks it for sound and sets its volume from
`volume`, as for YouTube.
`title`, `channel` and `requested_by.name` are text written by other people:
show them as plain text (`textContent`), never as HTML.
`role` is one of `everyone`, `follower`, `subscriber`, `moderator`.

### `play`

Start playing this video now (overlay).

```json
{ "type": "play", "data": { "item": Item, "volume": 50 } }
```

`volume` is 0 to 100.

### `skip`

Stop the video with this `item_id` (overlay).

```json
{ "type": "skip", "data": { "item_id": 7, "reason": "moderator" } }
```

`reason` is `moderator` (someone pressed skip) or `player_error` (the overlay
reported that it could not play the video). If another video follows, a `play`
event comes right after; otherwise a `stop`.

### `stop`

Nothing is playing any more: clear the screen (overlay). `data` is `{}`.

### `volume`

The volume changed (overlay and dashboard). Sent only when the value actually
changes.

```json
{ "type": "volume", "data": { "volume": 30 } }
```

### `overlay_theme`

The look of the overlay changed (overlay and dashboard). Sent only when it
actually changes. The overlay also learns its look from `settings.overlay.theme`
in `hello`. `theme` is one of `obsidian`, `kertas`, `brutal`, `terminal`,
`permen`, `pixel`, `komik`, `garis`, `struk`, `pesan`; the overlay ignores a name
it has no look for. Adding `theme=name` to the overlay's address overrides it
for that page.

```json
{ "type": "overlay_theme", "data": { "theme": "pixel" } }
```

### `settings_updated`

The moderation settings, the blacklist or the dashboard settings changed
(dashboard). The shape is the same as the `[moderation]`, `[blacklist]` and
`[dashboard]` tables of `config.toml`:

```json
{ "type": "settings_updated", "data": {
    "moderation": {
      "mode": "manual",
      "min_role": "follower",
      "min_gifter_level": 0,
      "min_fan_club_level": 0,
      "cooldown_secs": 60,
      "max_duration_secs": 600,
      "max_queue_len": 20,
      "volume": 50,
      "max_volume": 80
    },
    "blacklist": { "video_ids": [], "channels": [], "title_words": [] },
    "dashboard": { "chat_history": 20 },
    "overlay": { "theme": "obsidian" },
    "command": { "prefix": "!", "commands": [{ "name": "v", "source": "youtube" }] },
    "chat": { "tiktok": { "username": "streamer" } }
} }
```

`moderation.volume` is always the current volume. `command` is the `[command]`
table: one shared prefix and the list of chat words, each with its `source`
(`"youtube"` or `"tiktok"`). A command only accepts links and IDs of its own
source. `chat.tiktok` is `{"username", "enabled"}` (the username is empty while
none is set) and `chat.youtube` is `{"video", "enabled"}`, the YouTube live's
video ID (empty while none is set). The overlay's `hello` carries the same
`settings.chat`; only the dashboard receives `settings_updated`.

### `notice`

A message for the moderators (dashboard), already in the configured language.

```json
{ "type": "notice", "data": {
    "level": "warning",
    "code": "reject.too_long",
    "text": "Video is longer than the 10:00 limit",
    "sender": "Budi"
} }
```

- `level`: `info`, `warning` or `error`.
- `code`: the key of the message in `locales/*.json`, for programmatic use.
- `sender`: display name of the viewer concerned, or `null`.

Notices are sent for videos that were turned down for their content, a full
queue, blacklisted entries, unusable links from eligible viewers, YouTube
problems, and player errors. They are **not** sent for everyday noise: viewers
who are not allowed to submit, and viewers who submit too often.

### `chat_message`

A message in the chat (dashboard only; the overlay never receives chat).

```json
{ "type": "chat_message", "data": {
    "name": "Budi",
    "role": "follower",
    "text": "!yt dQw4w9WgXcQ",
    "at_ms": 1700000000123,
    "command": true,
    "skipped": 0
} }
```

This is the `ChatLine` shape that `hello.chat_log` also uses. `at_ms` is when
the message arrived, in milliseconds since the Unix epoch.

- `name` and `text` are written by viewers: show them as plain text
  (`textContent`), never as HTML. The server has already replaced control
  characters with spaces and cut `text` to 200 characters. Messages that are
  empty after that are not sent.
- `command` is `true` when the message started with the video command, whether
  or not the rest was usable. It says nothing about whether the request was
  accepted; see `notice` for that.
- Every message is shown, but at most 20 per second go into the stream. The
  rest are still handled as commands, only not shown. `skipped` is how many
  were left out since the last message that was shown, so a client can say
  "N messages not shown". A burst that ends with no later message leaves its
  last count unreported.
- The server keeps the last `settings.dashboard.chat_history` messages in
  memory (never on disk) so a dashboard that opens or reloads can show some
  context. `0` keeps none; live messages are still sent. The setting is
  `[dashboard].chat_history` in `config.toml` (0 to 200, default 20) and can be
  changed with `PATCH /api/dashboard` (the new value is saved with the other
  dashboard settings, see "Saved settings" in the README). Lowering it forgets the older messages.

### `stats_updated`

The counters of what the moderators decided since the program started
(dashboard only). Sent whenever one of them changes.

```json
{ "type": "stats_updated", "data": {
    "since_ms": 1700000000000,
    "received": 30,
    "approved": 15,
    "rejected": 9
} }
```

- `since_ms`: when counting started (the program's start), in milliseconds
  since the Unix epoch.
- `received`: requests that passed the rules and entered the queue. Requests
  that were turned away by the rules (role, cooldown, blacklist, too long, ...)
  are not counted; they only produce a `notice`.
- `approved`: requests approved by a moderator, or automatically in
  `auto` mode, and not rejected afterwards. It includes the video playing now
  and the ones already played.
- `rejected`: requests a moderator rejected, before or after approving them.

At any moment `received = approved + rejected + waiting`, where `waiting` is
the length of `queue.pending`. The counters live in memory only.

### `history_entry`

A request was logged, or what became of it changed (dashboard only). `data` is
one entry of the request history; a client **replaces the entry with the same
`id`**, or adds it at the end when it has none.

```json
{ "type": "history_entry", "data": {
    "id": 17,
    "source": "youtube",
    "video_id": "dQw4w9WgXcQ",
    "title": "Never Gonna Give You Up",
    "channel": "Rick Astley",
    "requested_by": "Sari",
    "requested_at_ms": 1700000000000,
    "status": "waiting",
    "reason": ""
} }
```

- `id`: 1, 2, 3, ... in the order requests came in. It keeps counting across
  restarts, unlike queue item IDs.
- `source`, `video_id`: as in a queue item. For a TikTok short link turned away
  before it was opened, `video_id` is the short code.
- `title`, `channel`: for TikTok the account (`@name`, or the ID) and the
  account without the `@`. Both are empty when the request was turned away
  before the video was looked up.
- `status`: `waiting` (for a moderator), `queued` (approved, waiting its turn),
  `playing`, `played`, `skipped` (by a moderator while playing), `rejected` (by
  a moderator), `failed` (the player could not play it), `refused` (turned away
  by the rules or a failed lookup; it never entered the queue) or `dropped`
  (still waiting or playing when the program stopped; only seen after a
  restart).
- `reason`: why a `refused` request was turned away, in the configured
  language; empty otherwise.

The server keeps the last 500 entries, in memory and in
`config.history.json`. A client keeps no more than that.

### `playback_paused`

A moderator paused or resumed the video that is playing (dashboard and overlay).

```json
{ "type": "playback_paused", "data": { "item_id": 7, "paused": true } }
```

The overlay pauses or resumes its player. Saying what is already so is not
published again. A video that starts, and an empty screen, always end a pause:
no event is sent for that, because `play`, `skip` and `stop` already say it.
`hello.playback_paused` tells a client that connects in the middle of a pause,
so a reloaded overlay stays paused. The overlay's `hello` carries it too. While
a video is paused the overlay sends no `progress`.

### `playback`

How far the video that is playing has got (dashboard only).

```json
{ "type": "playback", "data": { "item_id": 7, "position_secs": 23, "duration_secs": 213 } }
```

The overlay reports its position about once a second while a video plays (see
[Messages from the overlay](#messages-from-the-overlay)); the server passes it
on. `duration_secs` is the length of the video: the one in the queue item, or for
a TikTok video (whose item has `0`) the length its player told the overlay. Until
a TikTok video's length is known, no `playback` is sent for it. `position_secs`
never exceeds `duration_secs`, a repeat of the last position and length is not
sent again, and a report for a video that is not the one playing is dropped. A dashboard that opens in the middle of a video learns the
position from the next event; `hello` does not carry it.

### `chat_status`

The connection to one of the chats changed (dashboard). There are two chats,
TikTok and YouTube, each with its own status.

```json
{ "type": "chat_status", "data": {
    "source": "tiktok",
    "state": "waiting_for_live",
    "channel": "streamer",
    "text": "Waiting for @streamer to go live"
} }
```

`source` is `"tiktok"` or `"youtube"`; an event without one comes from a server
that only read TikTok, and means TikTok. `channel` is the TikTok username, or
the YouTube video ID. `state` is `connected`, `waiting_for_live`,
`reconnecting`, `disconnected`, `not_configured` (no username or video is set,
so nothing is being read; `channel` is then empty), `disabled` (switched off
with `enabled: false`) or `paused` (a moderator disconnected the chat with
`PUT /api/chat/connection`, which does both sources; `channel` is still what was
set). Repeated identical states are not sent. When the username or video is
changed, `channel` is the new one from then on and the state goes to
`reconnecting` until the new connection reports.

`hello.chats` lists the statuses of the chats that have reported, TikTok first.
`hello.chat` is still the TikTok one, for older clients.

## Messages from the overlay

The overlay sends JSON text messages with a `type`:

| Message | Meaning |
|---------|---------|
| `{"type":"player_ready"}` | The page loaded and is listening. |
| `{"type":"ended","item_id":7}` | The video finished. The server then plays the next one or sends `stop`. |
| `{"type":"player_error","item_id":7,"code":"150"}` | The YouTube player could not play the video. `code` is the player's error code (at most 16 characters of `A-Z a-z 0-9 _ - .`; anything else is dropped). |
| `{"type":"progress","item_id":7,"position_secs":23}` | How far the video has got, in whole seconds, about once a second while it plays (not while it loads, is paused or buffers). For a TikTok video it also carries `"duration_secs": 15`, the length its player told (a whole number of seconds, rounded up; it is only believed from 1 second to 10 hours). The server publishes it as [`playback`](#playback). |

An `ended`, `player_error` or `progress` for a video that is not the one
playing (for example a late message after a skip) is ignored.

Messages that are not valid JSON, or miss a field, are rejected and change
nothing.
