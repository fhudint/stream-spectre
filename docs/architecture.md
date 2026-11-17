# Architecture

stream-spectre is one Rust binary. It reads a live chat, turns `!yt <video>` and
`!vt <video>` messages into moderated YouTube and TikTok requests, and serves
two web pages (the OBS overlay and the moderator dashboard) from the same
process.

```
 TikTok Live chat
        │  ChatMessage { sender, text }
        ▼
┌─────────────────────────── app::Pipeline ───────────────────────────┐
│ command::CommandParser                                              │
│        │ VideoId (validated; the raw text is dropped here)          │
│        ▼                                                            │
│ queue::Gatekeeper, stage 1: role / levels, queue full,              │
│        blacklisted ID, per-user cooldown        (no network)        │
│        ▼                                                            │
│ youtube::LookupService: cache → videos.list (1 quota unit)          │
│        ▼                                                            │
│ queue::Gatekeeper, stage 2: not found, live, age-restricted,        │
│        not embeddable, too long, blacklisted channel / title word   │
│        ▼                                                            │
│ queue::PlayQueue: pending → approved → now playing                  │
│        │                                                            │
│ app::EventBus ── events (seq, ts, type, data) ──────────────────────┼──┐
└──────────────▲──────────────────────────────────────────────────────┘  │
               │ approve / reject / skip / settings (REST)               │
               │ ended / player_error (WebSocket)                        │
┌──────────────┴──────────── server (axum) ───────────────────────────┐  │
│ guard: Host, Origin, token  →  /api/*  /ws/*  /dashboard/  /overlay/│◄─┘
└──────────────▲────────────────────────────────────▲─────────────────┘
               │                                    │
        dashboard page                         overlay page
        (moderators)                      (OBS browser source,
                                            YouTube IFrame player)
```

## Modules

| Module | Responsibility |
|--------|----------------|
| `config` | Loads `config.toml`, secrets (environment or `.env`), validates everything before anything starts. |
| `i18n` | The `locales/id.json` and `locales/en.json` catalogs, embedded in the binary. All viewer- and moderator-facing text comes from here. |
| `command` | Turns chat text into `Parsed::{NotACommand, Invalid, Video}`. Accepts an 11-character ID and `youtu.be`, `youtube.com/watch`, `youtube.com/shorts` links. |
| `youtube` | Video IDs and links, the Data API client, response parsing, a cached and quota-aware lookup service, and the rules a video must pass. |
| `queue` | Plain rules and data with no I/O: the play queue, cooldowns, blacklist, and the `Gatekeeper` that combines them. Time is passed in, so tests are deterministic. |
| `chat` | The `ChatSource` trait, a platform-independent reconnecting supervisor, and the TikTok adapter. |
| `app` | `Pipeline` (the logic above), the event types, and the event bus. The server is a thin layer on top of it. |
| `server` | axum router, the request `guard`, REST handlers, WebSocket handlers, and the embedded static pages. |
| `platform` | OS-specific code behind `#[cfg(target_os = ...)]`: config directory lookup and the shutdown signal. |
| `text` | `sanitize_text`: control characters replaced, length capped. Applied to everything that comes from other people before it is stored or shown. |

The pages live in `static/overlay/` and `static/dashboard/` as plain HTML, CSS
and JavaScript, embedded with `rust-embed`. The dashboard is one page with five
views chosen by the address fragment (`#queue`, `#blacklist`, `#rules`,
`#overlay`, `#settings`); its fonts are served from `static/dashboard/fonts/`
so it needs nothing from other sites. Each page has a `*-core.js` file
with no browser dependencies (state handling, parsing) and a thin `*.js` file
that connects it to the DOM, the REST API and the WebSocket. The core files are
tested with Node (`tests/js/`).

## Design decisions

**Chat is untrusted input.** The parser hands on only a validated `VideoId`
(`^[A-Za-z0-9_-]{11}$`). Raw chat text never reaches a URL or HTML. Names and
titles are sanitized on the way in and shown with `textContent` on the way out.

**Quota is spent as late as possible.** YouTube's `videos.list` costs one unit
per call and the daily quota is finite. Every check that does not need video
details (role, levels, queue length, blacklisted ID, cooldown) happens first.
Lookups are cached, concurrent lookups of the same video are coalesced, and
after a quota error the service stops calling the API for a while. The cooldown
is spent before the lookup and given back when the lookup fails for a reason
that is not the viewer's fault.

**The TikTok username can change while the program runs.** `Pipeline` holds
the username (`update_tiktok`, from `PATCH /api/chat/tiktok`) and publishes it
in a `watch` channel. `run_chat_sources` reads the chat of the current name,
and when the name changes it drops the old source between two messages (a
message being handled is finished first) and makes a new one with the factory it
was given; with no name it reads nothing and reports `not_configured`. A moderator
can also disconnect the chat (`set_chat_paused`): the source is dropped the same
way and the state is `paused` until it is connected again. That switch lives only
in memory, so a restart connects by itself. The
username is also one of the tables saved beside `config.toml`.

The chat words are a list of `{ name, source }` behind one shared prefix
(`[command]`). `CommandParser` is built from it and kept in the pipeline's
state, so `update_commands` swaps it while the program runs; the next message
is read with the new words. A name matches as a whole word, ignoring capitals,
and what follows it is read according to the command's source (a YouTube
command accepts only YouTube IDs and links). `[command]` is saved beside
`config.toml` like the other tables.

**`ChatSource` is the only thing that knows about TikTok.** The rest of the app
sees `ChatMessage { sender, text }` and `ChatStatus`. TikTok has no official
API; the adapter uses the `piratetok-live-rs` crate (pinned to the audited
version). Another source (Twitch, YouTube Live) means implementing the trait.
The reconnect logic in `chat::connection` is shared.

**Chat is shown to moderators, within limits.** Every chat message is cleaned
with `sanitize_text` and published as a `chat_message` event, for the dashboard
only. Chat can be very fast and shares one event bus with the events that
matter, so at most 20 messages per second are let through (the rest are still
parsed as commands, only not displayed, and the next message shown says how many
were left out). The last few messages are kept in memory (`chat_history`, 0 to
200) and sent with `hello`; nothing is written to disk.

**The dashboard shows counters and progress without keeping them itself.**
The server counts the requests that entered the queue and what the moderators
decided (`StatsView`; at any moment `received = approved + rejected + waiting`)
and sends them as `stats_updated`. The overlay reports the playback position
once a second as `progress`; the server checks that it belongs to the video
that is playing, limits it to the video's length, drops repeats and sends it on
to the dashboards as `playback`. Neither is sent to the overlay, which only
gets the events that change what is on screen. The dashboard page counts
waiting times itself, but on the server's clock (taken from the `ts` of the
events it receives), so a moderator's wrong clock does not show.

**A moderator can pause the video that is playing.** The pipeline keeps one
flag, `playback_paused`, and publishes `playback_paused` when it changes; the
overlay tells its YouTube player to pause or play. The flag is cleared whenever
another video starts or the screen empties (`play_next_or_stop`), so a pause
never carries over to the next video, and it is in `hello` so an overlay that
reloads in the middle of a pause stays paused.

**Two sources share one queue.** A queue entry holds a `Media`: a YouTube
video (with the details the API gave) or a TikTok video (an ID and maybe an
account). `CommandParser` returns `Parsed::Youtube` or `Parsed::Tiktok` by the
`source` of the command word that was used, and refuses the other source's
links with the command to use instead. A short TikTok code is looked up by
`tiktok::ShortLinkResolver` (one request to `vt.tiktok.com`, redirects not
followed). The gates are the same for both (role, level, queue length,
cooldown, blacklist), but a TikTok entry has no length, age or embedding check
and so never skips the approval queue (`Media::can_auto_approve`). The overlay
shows YouTube in its IFrame player and TikTok in a frame of TikTok's embedded
player, in the same stage; the frame is replaced or removed with the next
video. TikTok's player mutes every video it starts; the overlay talks to it by
`postMessage` (only to and from TikTok's origin and the frame on screen) to ask
for sound and to set the volume, and mutes it and sets it playing again when the
browser refuses sound (`createTiktokSound` in `overlay-core.js`). It also tells
when the video ends (state 0) and when it cannot be played (`onPlayerError`);
the overlay turns those into the same `ended` and `player_error` reports as
for YouTube (`tiktokOutcome`). Nothing else it reports is used.

**One event stream, versioned.** Everything the pages show comes from the
events described in [`websocket-events.md`](websocket-events.md). A client
subscribes, receives `hello` with the whole state, and then applies events
newer than `hello.seq`. A slow client gets a fresh `hello` instead of the
events it missed, so there is no partial state to repair.

**The server trusts nobody by default.** Every request goes through `guard`:
the `Host` header must match the loopback address and port (against DNS
rebinding), a browser `Origin` must be the server itself (against cross-site
requests), and when `DASHBOARD_TOKEN` is set the API and WebSockets need it
(compared in constant time). Binding to a non-loopback address is refused at
startup unless a token is configured. The pages are served with a strict
Content-Security-Policy (no inline script or style) and a per-page
Referrer-Policy: the overlay must send a referrer, or YouTube's embedded
player refuses to play.

**Settings changed in the dashboard are saved beside `config.toml`, never in
it.** `config.toml` is written by hand and has comments, so the program only
reads it. `app::autosave` listens to the same events as the pages and, a moment
after settings or the volume change, writes the tables that differ from
`config.toml` to `config.settings.toml` (written to a temporary file and renamed,
so a crash never leaves half a file). At startup `config::load` puts those
tables on top of `config.toml` and validates the result; a damaged file stops
the program with a message that names it. Whole tables are stored, so a saved
table replaces the table in `config.toml` entirely. A failed write is logged and
shown to the moderators as a notice.

**A YouTube API key can be replaced while the program runs.** `Credentials`
(in `app`) holds the key saved from the dashboard. Changing it checks the new
key with one real request, writes it to the secrets file (created readable only
by its owner, through a temporary file that is renamed, so the key is never in a
file others can read), and only then switches the client over; `VideoLookup`
forgets its cache and any quota pause, since they belong to the old key. A key
YouTube refuses, or a file that cannot be written, leaves everything as it was.
The key is write-only: the dashboard learns whether a key is set, where it came
from and its last four characters, never the key. Only a connection from the
computer the program runs on may change it. With no key at all the program still
starts and refuses video requests with a notice (`NoApiKey`).

**Secrets stay out of config, URLs and logs.** API keys come from the
environment or `.env`, are wrapped in a `Secret` type that does not print, and
are sent to YouTube in the `x-goog-api-key` header rather than the URL.

**No panics on odd input.** `unsafe` is forbidden, and `unwrap`, `expect` and
`panic` are denied by lint outside tests.

## Adding things

- **A new chat source:** implement `ChatSource` in `src/chat/`, choose it with
  `chat.source` in the config, and map the platform's user data to `Sender`
  (`id`, display name, `Role`, gifter and fan club levels).
- **A new event:** add a variant in `src/app/events.rs`, publish it from
  `Pipeline`, document it in `websocket-events.md`, and handle it in the page
  cores. Clients ignore unknown event types, so adding one is not a breaking
  change; changing an existing one is, and bumps `v`.
- **A new UI language:** add `locales/<code>.json` with the same keys as the
  others (a test checks key parity), and the matching `Language` variant.
- **A new feature module** (polls, alerts, ...): give it its own module under
  `src/`, its own state inside or beside `Pipeline`, and its own event types.
  The mediashare code does not need to change.

## Testing

- **Rust:** unit tests next to the code, plus real-socket tests that start the
  server on a local port and talk to it with a WebSocket client.
  `cargo test` runs them all.
- **JavaScript:** `node --test "tests/js/*.test.js"` runs the page tests; the
  dashboard glue test builds a fake DOM from the real `index.html`.
- **Demo:** `cargo run --example demo` runs the full app with pretend viewers
  and no network access to TikTok or YouTube's API, which is the quickest way to
  see a change in a browser.
