<p align="center"><img src="assets/icon-128.png" width="96" height="96" alt="stream-spectre logo"></p>

# stream-spectre

An OBS browser-source overlay for streamers, written in Rust. The first feature
is **mediashare through chat**: viewers send a YouTube video in chat, a
moderator approves it in a dashboard, and it plays on your stream.

```
viewer: !yt dQw4w9WgXcQ   →   checked   →   dashboard (approve / reject / skip)   →   plays in OBS
```

> **Status: early.** The code is complete for the first version and well
> tested, but it has had only light testing in OBS and against real TikTok
> lives. See [Known limitations](#known-limitations).

## Copyright warning

Playing other people's videos on your stream can get you copyright claims,
muted VODs, or strikes, depending on the platform and the video. Embedding does
not make that go away. **Leave moderation on** (the default is manual
approval), set a maximum duration, and use the blacklist. You are responsible
for what plays on your stream.

## Features

- Chat command `!yt <id or link>`. Accepts an 11-character video ID,
  `youtu.be/ID`, `youtube.com/watch?v=ID` and `youtube.com/shorts/ID`.
  The prefix and the command words are configurable, and you can have several
  (for example `!yt` and `!v` for the same thing), on the dashboard's Rules
  page or in `[command]`.
- TikTok videos too, with a command of their own (`!vt` in the example config,
  or any word you add on the Rules page with the source TikTok). After it a viewer sends a video
  ID (`!vt 7700124541145599252`), the code of a short link from the app
  (`!vt ZSpGELffb`, from `vt.tiktok.com/ZSpGELffb`), or a full link. A command
  takes only its own source: `!yt` with a TikTok link is answered with the
  command to use. TikTok videos always wait for a moderator, are played in the
  same overlay and the same queue, and move on by themselves when they end (see
  [Known limitations](#known-limitations)).
- Checks each video with the YouTube Data API (one quota unit per new video,
  results cached): rejects videos that cannot be embedded, are age-restricted,
  are live streams, or are too long.
- Moderation: manual approval by default (or automatic), minimum role
  (everyone / follower / subscriber / moderator), minimum TikTok gifter level
  and fan club level, per-viewer cooldown, maximum duration, queue length limit,
  blacklist (video IDs, channel names, words in the title), skip button, and a
  volume limit.
- A dashboard for moderators (the video playing with its progress, the approval
  queue with keyboard shortcuts, a live view of the chat with video requests
  highlighted, counters for the session, the rules in force, settings and the
  blacklist) that also works on a phone, and a transparent overlay page for OBS,
  both served by the same single binary. No installation of Node or a web server,
  and nothing loaded from the internet by the dashboard except video pictures
  from YouTube.
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

Open <http://localhost:8787/dashboard/>. Pretend viewers send real public
YouTube videos, and now and then a TikTok one, every few seconds, using the
commands of `config.example.toml` (`!yt` and `!vt`); approve one and it plays in
<http://localhost:8787/overlay/>.

Always use `localhost`, not `127.0.0.1`, in these addresses: see
[the note on addresses](#use-localhost-not-an-ip-address) below.

To try videos of your own, copy `demo-videos.example.json` to
`demo-videos.json` (it is git-ignored, like `.env`) and list the videos in it:
`youtube` takes IDs or links, `tiktok` takes IDs, short link codes or links. A
key you leave out keeps the built-in list, `[]` turns that source off, and an
entry the program would not understand is reported and skipped. `DEMO_VIDEOS`
names another file.

## Set up

### 1. Get a YouTube API key

1. In the [Google Cloud console](https://console.cloud.google.com/), create a
   project and enable **YouTube Data API v3**.
2. Create an API key under *Credentials*. Restricting it to the YouTube Data
   API v3 is a good idea.

The free daily quota (10,000 units) is plenty: each new video costs 1 unit, and
repeats are served from a cache.

You can put the key in `.env` (below) or enter it later on the dashboard's
**Account** page. The program starts without one; until a key is set it refuses
video requests, with a notice for the moderators that says why.

### 2. Configure

```sh
cp config.example.toml config.toml
cp .env.example .env
```

Edit `config.toml`. Your TikTok username can go here, or you can leave it empty
and enter it later on the dashboard's **Account** page (see below):

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

Put the secret in `.env` (which is git-ignored) or in the environment, or skip
this and enter the key on the dashboard:

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
2. URL: `http://localhost:8787/overlay/` (`localhost`, not `127.0.0.1`: see
   [the note below](#use-localhost-not-an-ip-address))
3. Set width and height to your canvas (for example 1920 × 1080). The page is
   transparent; videos appear when one is playing and the source is empty
   otherwise.
4. Tick **Control audio via OBS** if you want the video's sound in the audio
   mixer. The volume is set from the dashboard.
5. Move and resize the source in your scene.

Open <http://localhost:8787/dashboard/> in a normal browser (for you or your
moderators) to approve, reject and skip requests, and to change the settings
while live. Settings changed in the dashboard are kept across restarts: they
are saved to `config.settings.toml` next to `config.toml`, which the program
writes and `config.toml` never is. See [Saved settings](#saved-settings).

#### Using the dashboard

The menu has six pages: **Queue** (the video playing and how far it has got,
the requests waiting for a decision, the play queue, the rules in force, the
chat, and counters for this session), **Requests** (every request viewers sent,
newest first, with what became of it: waiting, in the play queue, playing,
played, skipped, rejected, could not play, or refused by the rules with the
reason), **Blacklist**, **Rules** (the moderation settings), **Overlay** (the
address to copy into OBS, and the volume) and **Settings** (how much chat the
dashboard remembers). Each page has its own address (`#queue`, `#requests`,
`#blacklist`, `#rules`, `#overlay`, `#settings`). On a phone the menu is at the
bottom and the queue page shows the waiting requests, the chat and the notices
as three tabs.

Next to a waiting or approved request, and on every row of the **Requests**
page, **Blacklist video** adds the video's ID to the blacklist and **Blacklist
channel** (**account** for TikTok) adds its channel. On the queue page the
request is rejected at the same time. The button is greyed out once the entry
is listed, and a row has no button for what cannot be listed (a TikTok short
link that was never opened).

The **Requests** list keeps the last 500 requests, also across restarts, in
`config.history.json` next to `config.toml` (it holds viewers' display names;
delete it to clear the list). A request that was still waiting or playing when
the program stopped shows as "Never played". A busy chat with a full queue or
viewers who ask too often fills the list with refused requests.

The avatar at the top right opens a small menu with two items:

- **Account settings** leads to the **Account** page (`#account`), with three
  parts: your **TikTok username**, the **YouTube API key** and the **dashboard
  token**.
- **Disconnect chat** stops reading the live chat of the running session, so no
  new video requests arrive; the username stays. The item then reads **Connect
  chat** to read it again. This only lasts for the current run: after a restart
  the chat connects by itself. It is not shown while no username is set.

- The username can be set, changed or emptied there, and the program follows it
  **without a restart**: it drops the old chat connection and connects to the
  new channel. With no username the program still starts; the Queue page says
  that no chat is being read and links to the Account page.
- The API key is tried with one YouTube request (1 quota unit) before it is
  saved, takes effect at once, and is **never shown again**: the page only says
  that a key is set, where it came from and its last four characters. It is
  saved in `config.secrets.toml` next to `config.toml`, a file only your user can
  read (mode 0600 on Linux and macOS; on Windows the protection is your profile
  folder), which is git-ignored. A key saved there **replaces** `YOUTUBE_API_KEY`;
  "Remove saved key" goes back to the environment's. Only plain text is kept, so
  treat that file like a password.
- The dashboard token is only shown as in use or not. It is still set with
  `DASHBOARD_TOKEN` in `.env`.

Credentials can only be changed from the computer the program runs on, even
when moderators open the dashboard over the network with a token: they see what
is set, and the form is locked for them.

On the Queue page the keyboard works when you are not typing in a field:
**↑ ↓** choose a waiting request, **A** approves it, **R** rejects it, **S**
skips the video that is playing and **P** pauses it or lets it go on. The
playing card also has a **Pause** button (it becomes **Resume**). A pause lasts
for that one video: the next one always plays.

How long a request has waited is measured on the program's clock, so it is
right even when a moderator's computer has the wrong time. The session counters
count only requests that passed the rules and entered the queue; they start from
zero each time the program starts. "Copy overlay URL" copies the address with
`localhost` and, if you use a token, the token.

#### Saved settings

`config.toml` is yours: the program only reads it, so your comments stay. When
you change moderation settings, the volume, the blacklist, the chat commands,
the chat history or the TikTok username in the dashboard, the program writes the changed tables to
`config.settings.toml` (named after your config file, in the same folder). At
the next start, a table in that file **replaces** the same table in
`config.toml`, and the log says which ones. This also means that editing a
table in `config.toml` has no effect while the saved file holds that table.

To go back to `config.toml`, stop the program and delete the table from
`config.settings.toml`, or the whole file. A table you change back to the value
in `config.toml` is dropped from the file by itself. If the file cannot be
written (read-only folder, full disk), the dashboard shows a notice and the
change is lost on restart. If the file is damaged, the program refuses to start
and names the file and the problem.

#### Use `localhost`, not an IP address

Open the overlay as `http://localhost:8787/overlay/`. When the overlay was
opened as `http://127.0.0.1:8787/overlay/`, every video failed with YouTube
player error 150 and was skipped at once, even ordinary ones. With `localhost`
the same videos played. This was measured in Firefox, and switching to
`localhost` fixed it in the author's own setup; the exact rule YouTube applies
is not documented, so treat it as observed behaviour rather than a guarantee.

#### Autoplay

In OBS a video starts by itself. In a regular browser tab, autoplay with sound
is blocked until you have clicked on the page once, so the video may sit
paused there; that is the browser's rule, not a bug. Test in OBS, or click the
page first.

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
after `#` is never sent to the server). Keep OBS on the same computer as the
program if you can, and use `http://localhost:8787/overlay/?token=a-long-random-string`
there. An overlay opened through a bare IP address is likely to hit the
player error 150 described [above](#use-localhost-not-an-ip-address); if OBS has
to run on another computer, try a hostname for the server instead (not tested).

The token is sent without encryption over plain HTTP. Use this on a network you
trust, not across the internet.

## Known limitations

- **Lightly tested on real streams.** Playback has been tried in OBS. The
  TikTok connection and the role and level mapping (follower, subscriber,
  gifter and fan club levels) are mostly tested against fakes, so they may not
  match every kind of viewer. Please report what you find.
- **TikTok has no official chat API.** The connection uses an unofficial client
  (`piratetok-live-rs`) and can break whenever TikTok changes something.
- **Some videos still fail to play.** The YouTube API saying a video is
  embeddable does not guarantee it plays (the owner may restrict playback). The
  overlay reports the failure, the video is skipped, and the moderators get a
  notice with the player's error code. If *every* video fails with code 150,
  you most likely opened the overlay through an IP address; see
  [above](#use-localhost-not-an-ip-address).
- **TikTok videos are new and not tried in OBS yet.** They play in TikTok's own
  embedded player, in the same overlay. TikTok tells us nothing about a video
  without a developer account, so there is no title, thumbnail or length: the
  queue shows the account (when it is known) and the ID, the maximum length
  cannot be enforced, and they always wait for a moderator even with automatic
  approval. TikTok's player tells the overlay when a video ends and when it
  cannot be played (it does not exist, is private or is removed), and then the
  queue moves on, with a notice for the moderators in the second case. If the
  player has not started a video within 15 seconds, the overlay gives up on it
  and skips it with a notice (code `timeout`); a video that runs but never says
  it is over stays until a moderator presses skip. **Videos with a product link
  (TikTok Shop or affiliate videos, the yellow basket) cannot be played in a
  browser at the moment, only in TikTok's own app**, so they end up skipped by
  that timeout; nothing tells them apart beforehand, and a moderator can reject
  one that shows the basket when it is waiting. A video cannot be paused from
  the dashboard. TikTok's player starts
  every video muted, so the overlay asks it for sound (with the volume of the
  dashboard's slider): in OBS that works, in an ordinary browser tab the browser
  usually refuses sound without a click and the video plays silently. If a
  TikTok video has no sound in OBS, check that "Control audio via OBS" is on for
  the browser source. A private, deleted or region-locked video cannot be told
  from a good one beforehand, only when it is tried. A short link is opened with
  one request to TikTok's own server, which only the code is sent to.
  **The account is known only when it is in the link.** For a bare ID
  (`!vt 7703227524993649928`) the program tries to read it from the page
  TikTok serves for that video (one request that only the ID goes into, a few
  seconds at most, remembered afterwards). That page is not an official source:
  it tells the account for some videos and not for others, and it may change.
  Without the account the queue shows the ID, and **Blacklist account** and a
  blacklisted account name do not apply, so send the full link or the short
  link from the TikTok app when the account matters.
- **Saved settings replace whole tables.** A table saved from the dashboard
  (for example all of `[moderation]`) replaces that table in `config.toml`;
  single keys are not merged. Apart from the TikTok username, the settings for
  the chat source, command, server and YouTube are not changed from the
  dashboard and stay in `config.toml`.
- **Chat is not stored.** The dashboard's chat view shows what arrives while it
  is open, plus the last few messages the program remembers in memory (20 by
  default, `chat_history` in `config.toml`). At most 20 messages per second are
  shown; video requests are still handled when chat is faster than that.
- **Windows and macOS** are built in CI but have not been tested by hand.

## For developers

- [`docs/architecture.md`](docs/architecture.md): how the pieces fit together
  and why.
- [`docs/websocket-events.md`](docs/websocket-events.md): the HTTP API and the
  event protocol, for anyone writing another client.
- [`docs/roadmap.md`](docs/roadmap.md): ideas for the next update.

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
node --test "tests/js/*.test.js"      # Node 22 or newer
```

## Credits

The idea of mediashare through chat, where viewers put a command and a video ID
in a comment, comes from Antonius Wilson (GLHFSon): Instagram
[@glhfson](https://www.instagram.com/glhfson/), TikTok
[@jendralson](https://www.tiktok.com/@jendralson).

## License

[AGPL-3.0-only](LICENSE). If you run a modified version as a network service,
you must offer its source to the people who use it.

The dashboard includes the fonts Inter and Fira Code (latin subsets), under the
SIL Open Font License 1.1. Their license texts are in
[`static/dashboard/fonts/`](static/dashboard/fonts/).
