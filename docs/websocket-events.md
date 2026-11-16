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
| `GET /dashboard/`, `GET /overlay/` | The pages themselves |

The two sockets carry the events below. The overlay socket only receives
what changes the screen (`play`, `skip`, `stop`, `volume`) and a trimmed
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
    "stats": { "since_ms": 1700000000000, "received": 30, "approved": 15, "rejected": 9 }
} }
```

`chat` is `null` until the chat connection has reported its first status.
`chat_log` holds the most recent chat messages, oldest first, at most
`settings.dashboard.chat_history` of them (see [`chat_message`](#chat_message)).
`stats` is described under [`stats_updated`](#stats_updated). The overlay's
`hello` carries `stats` with every number zero, like its empty `chat_log`.

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
  "video_id": "dQw4w9WgXcQ",
  "title": "Song",
  "channel": "Channel name",
  "duration_secs": 213,
  "thumbnail_url": "https://i.ytimg.com/vi/dQw4w9WgXcQ/mqdefault.jpg",
  "requested_by": { "name": "Budi", "role": "subscriber" },
  "requested_at_ms": 1700000000123
}
```

`id` is unique while the server runs. `video_id` is always exactly 11
characters from `A-Z a-z 0-9 _ -`, so it is safe to use in a URL.
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
    "dashboard": { "chat_history": 20 }
} }
```

`moderation.volume` is always the current volume.

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
    "text": "!v dQw4w9WgXcQ",
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

### `playback`

How far the video that is playing has got (dashboard only).

```json
{ "type": "playback", "data": { "item_id": 7, "position_secs": 23 } }
```

The overlay reports its position about once a second while a video plays (see
[Messages from the overlay](#messages-from-the-overlay)); the server passes it
on. `position_secs` never exceeds the video's `duration_secs`, a repeat of the
last position is not sent again, and a report for a video that is not the one
playing is dropped. A dashboard that opens in the middle of a video learns the
position from the next event; `hello` does not carry it.

### `chat_status`

The connection to the chat changed (dashboard).

```json
{ "type": "chat_status", "data": {
    "state": "waiting_for_live",
    "channel": "streamer",
    "text": "Waiting for @streamer to go live"
} }
```

`state` is `connected`, `waiting_for_live`, `reconnecting` or `disconnected`.
Repeated identical states are not sent.

## Messages from the overlay

The overlay sends JSON text messages with a `type`:

| Message | Meaning |
|---------|---------|
| `{"type":"player_ready"}` | The page loaded and is listening. |
| `{"type":"ended","item_id":7}` | The video finished. The server then plays the next one or sends `stop`. |
| `{"type":"player_error","item_id":7,"code":"150"}` | The YouTube player could not play the video. `code` is the player's error code (at most 16 characters of `A-Z a-z 0-9 _ - .`; anything else is dropped). |
| `{"type":"progress","item_id":7,"position_secs":23}` | How far the video has got, in whole seconds, about once a second while it plays (not while it loads, is paused or buffers). The server publishes it as [`playback`](#playback). |

An `ended`, `player_error` or `progress` for a video that is not the one
playing (for example a late message after a skip) is ignored.

Messages that are not valid JSON, or miss a field, are rejected and change
nothing.
