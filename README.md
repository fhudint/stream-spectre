# stream-spectre

An OBS browser-source overlay for streamers, written in Rust. The first feature
is **mediashare through chat**: viewers send a YouTube video in chat, a
moderator approves it in a dashboard, and it plays on your stream.

```
viewer: !v dQw4w9WgXcQ   →   checked   →   dashboard (approve / reject / skip)   →   plays in OBS
```

> **Status: early.** The code is complete for the first version and well
> tested, but it has not yet been run against a real TikTok live or inside OBS.
> See [Known limitations](#known-limitations).

## Copyright warning

Playing other people's videos on your stream can get you copyright claims,
muted VODs, or strikes, depending on the platform and the video. Embedding does
not make that go away. **Leave moderation on** (the default is manual
approval), set a maximum duration, and use the blacklist. You are responsible
for what plays on your stream.

## Features

- Chat command `!v <id or link>`. Accepts an 11-character video ID,
  `youtu.be/ID`, `youtube.com/watch?v=ID` and `youtube.com/shorts/ID`.
  The prefix and command name are configurable.
- Checks each video with the YouTube Data API (one quota unit per new video,
  results cached): rejects videos that cannot be embedded, are age-restricted,
  are live streams, or are too long.
- Moderation: manual approval by default (or automatic), minimum role
  (everyone / follower / subscriber / moderator), minimum TikTok gifter level
  and fan club level, per-viewer cooldown, maximum duration, queue length limit,
  blacklist (video IDs, channel names, words in the title), skip button, and a
  volume limit.
- A dashboard for moderators and a transparent overlay page for OBS, both
  served by the same single binary. No installation of Node or a web server.
- Dashboard and overlay in English or Indonesian.
- Chat sources sit behind a trait, so Twitch or YouTube Live chat can be added
  later. TikTok Live is the only one for now.

## Install

There are no prebuilt binaries yet; build from source. You need
[Rust](https://rustup.rs) 1.85 or newer.

```sh
git clone https://github.com/fhudint/stream-spectre.git
cd stream-spectre
cargo build --release
```

The binary is `target/release/stream-spectre`. It is a single file with the
pages built in.

Development so far has been on Linux (Fedora). CI also builds on Windows and
macOS, but those have not been tried by hand.

### Try it without TikTok or an API key

```sh
cargo run --example demo
```

Open <http://127.0.0.1:8787/dashboard/>. Pretend viewers send real public
YouTube videos every few seconds; approve one and it plays in
<http://127.0.0.1:8787/overlay/>.

## Set up

### 1. Get a YouTube API key

1. In the [Google Cloud console](https://console.cloud.google.com/), create a
   project and enable **YouTube Data API v3**.
2. Create an API key under *Credentials*. Restricting it to the YouTube Data
   API v3 is a good idea.

The free daily quota (10,000 units) is plenty: each new video costs 1 unit, and
repeats are served from a cache.

### 2. Configure

```sh
cp config.example.toml config.toml
cp .env.example .env
```

Edit `config.toml` and set at least your TikTok username:

```toml
[general]
language = "en"            # "id" or "en"

[chat.tiktok]
username = "your_tiktok_username"

[moderation]
mode = "manual"            # every request waits for approval
min_role = "follower"      # who may submit
cooldown_secs = 60
max_duration_secs = 600
```

Put the secret in `.env` (which is git-ignored) or in the environment:

```sh
YOUTUBE_API_KEY=your-key-here
```

`config.example.toml` lists every option with its default and an explanation.
The config file is looked up in this order: `--config <path>`, then
`STREAM_SPECTRE_CONFIG`, then `./config.toml`, then the per-user directory
(`~/.config/stream-spectre/` on Linux, `%APPDATA%\stream-spectre\` on Windows,
`~/Library/Application Support/stream-spectre/` on macOS).

### 3. Run

```sh
./target/release/stream-spectre
```

The program waits for your TikTok live to start and connects by itself, and
reconnects if the connection drops. The dashboard shows the chat status.

### 4. Add the overlay to OBS

1. In OBS, add a **Browser** source.
2. URL: `http://127.0.0.1:8787/overlay/`
3. Set width and height to your canvas (for example 1920 × 1080). The page is
   transparent; videos appear when one is playing and the source is empty
   otherwise.
4. Tick **Control audio via OBS** if you want the video's sound in the audio
   mixer. The volume is set from the dashboard.
5. Move and resize the source in your scene.

Open <http://127.0.0.1:8787/dashboard/> in a normal browser (for you or your
moderators) to approve, reject and skip requests, and to change the settings
while live. Settings changed in the dashboard are not written back to
`config.toml`.

### Letting moderators on another computer in

By default the server listens on `127.0.0.1` only. To open it to your network,
set `bind` in `[server]` and set a token (at least 16 characters):

```toml
[server]
bind = "0.0.0.0"
```

```sh
DASHBOARD_TOKEN=a-long-random-string
```

The program refuses to start on a non-local address without a token. Then
open `http://<your-ip>:8787/dashboard/#token=a-long-random-string` (the part
after `#` is never sent to the server) and, in OBS,
`http://<your-ip>:8787/overlay/?token=a-long-random-string`.

The token is sent without encryption over plain HTTP. Use this on a network you
trust, not across the internet.

## Known limitations

- **Not yet tried on a real stream.** The TikTok connection, the role and level
  mapping (follower, subscriber, gifter and fan club levels), and playback
  inside OBS are tested against fakes and in a regular browser, not against a
  live TikTok room or OBS. Please report what you find.
- **TikTok has no official chat API.** The connection uses an unofficial client
  (`piratetok-live-rs`) and can break whenever TikTok changes something.
- **"Embeddable" is not a promise.** YouTube sometimes reports a video as
  embeddable and then refuses to play it. The overlay reports that back, the
  video is skipped, and the moderators get a notice.
- **Autoplay in OBS** depends on the OBS version and browser source settings;
  it has not been checked yet.
- **Settings are in memory.** Changes made in the dashboard last until the
  program restarts.
- **Windows and macOS** are built in CI but have not been tested by hand.

## For developers

- [`docs/architecture.md`](docs/architecture.md): how the pieces fit together
  and why.
- [`docs/websocket-events.md`](docs/websocket-events.md): the HTTP API and the
  event protocol, for anyone writing another client.

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
node --test "tests/js/*.test.js"      # Node 22 or newer
```

## License

[AGPL-3.0-only](LICENSE). If you run a modified version as a network service,
you must offer its source to the people who use it.
