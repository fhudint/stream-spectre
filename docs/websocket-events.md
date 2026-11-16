# WebSocket events

The overlay and the dashboard receive the same stream of JSON events. The
Rust definitions in [`src/app/events.rs`](../src/app/events.rs) are the source
of truth; this page describes them for people writing clients.

> **Status:** the event format below is implemented and tested. The HTTP and
> WebSocket endpoints that carry it (paths, REST actions, authentication) are
> added in the server step and will be documented here then.

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
3. If the connection drops, reconnect and start again at step 1. If the server
   closes it because the client fell too far behind, that is the same case.

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
    "chat": { "state": "connected", "channel": "streamer", "text": "..." }
} }
```

`chat` is `null` until the chat connection has reported its first status.

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

The moderation settings or the blacklist changed (dashboard). The shape is the
same as the `[moderation]` and `[blacklist]` tables of `config.toml`:

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
    "blacklist": { "video_ids": [], "channels": [], "title_words": [] }
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

An `ended` or `player_error` for a video that is not the one playing (for
example a late message after a skip) is ignored.

Messages that are not valid JSON, or miss a field, are rejected and change
nothing.
