# Roadmap

Ideas for the next update. Nothing here is promised or scheduled: the
maintainer decides what gets built and in what order. Each item says what
exists today, what is still open, and what has been checked so far. Anything
marked *unverified* is a belief that has not been tested.

## 1. TikTok videos in mediashare

**Goal.** Viewers can send a TikTok video link and it plays on stream, the
same way YouTube videos do.

**Today.** Only YouTube. The command parser accepts a YouTube ID or link and
hands on a validated `VideoId`; the overlay plays it with the YouTube IFrame
player; the queue, moderation and events all assume a YouTube video.

**Open questions**
- How does the overlay play a TikTok video? TikTok offers an embeddable player
  and an oEmbed endpoint (*unverified*: what each returns, whether the player
  reports "ended" and errors back to the page, and whether it needs a
  non-IP origin like YouTube's did).
- What can be known before approval? oEmbed gives a title, author and
  thumbnail but, as far as is known, not a duration, so the maximum-duration
  rule may not be enforceable up front (*unverified*).
- Short links (`vm.tiktok.com/...`) need a redirect to find the video ID.
  Following redirects for text a stranger typed is a security risk (SSRF): it
  must be limited to TikTok's own hosts, with a small redirect limit.
- Copyright and platform rules for re-playing other people's TikToks on stream
  need the same warning as YouTube, and probably a stricter default.

**Shape of the change (a sketch).** A `VideoRef` that is either a YouTube or a
TikTok video, a second lookup implementation behind the existing `VideoSource`
idea, a `source` field on queue items and `play` events (a protocol addition,
so clients that ignore unknown fields keep working), and a second player in the
overlay behind one small interface. The chat word that selects the source is
item 5.

## 2. Check YouTube IDs cheaply before using the API key

**Goal.** Spend as little YouTube API quota as possible, in particular on IDs
that do not exist.

**Today.** These already save quota: the ID's format is checked first (11
characters of `A-Z a-z 0-9 _ -`), then role and level, queue length, the
blacklist and the per-viewer cooldown, all before any request. Lookups are
cached (including "no such video"), concurrent lookups of one video are
coalesced, and after a quota error the API is left alone for a while.

**What was checked.** For a real video, YouTube's public oEmbed endpoint
(`youtube.com/oembed`) answered 200 and the thumbnail
(`i.ytimg.com/vi/<id>/default.jpg`) answered 200. For two IDs that fit the
format but do not exist (`aaaaaaaaaaa`, `terimakasih`), oEmbed answered 400 and
the thumbnail 404. Neither endpoint costs API quota.

**Limits.** oEmbed also answered 200 for a video the embedded player later
refused (error 150, which turned out to come from opening the overlay through
`127.0.0.1`), and it says nothing about duration, age restriction or live
streams. It can filter out IDs that do not exist; it cannot replace
`videos.list`. It is not documented as a stable API, and its rate limits are
unknown.

**Ideas**
- Ask oEmbed first; call `videos.list` only when it answers 200. Costs one
  extra request to YouTube per new ID, saves one quota unit for each ID that
  does not exist.
- Batch several waiting IDs into one `videos.list` call (it accepts up to 50 IDs
  for the same 1 unit). Only helps when requests arrive close together.
- Treat "no such video" more strictly: remember bad IDs longer than good ones.

**Decision needed.** Whether the saving justifies depending on an undocumented
endpoint. The default quota (10,000 units a day) is large for one streamer, so
this may only matter for heavy use.

A keyless mode (no `YOUTUBE_API_KEY`, oEmbed only) was also discussed. Its cost
is that duration, age restriction and live status would only be known once the
player loads, so the maximum duration could not be enforced before approval.

## 3. A desktop app

**Goal.** Turn stream-spectre into an app a streamer can install and open,
instead of a program started from a terminal.

**Today.** One binary with the pages built in. Started from a terminal, it reads
`config.toml` and a `.env` file.

**Likely approach.** Tauri: a native window showing the existing dashboard,
with the server running in the same process, and installers for Windows, macOS
and Linux. The pages and the server logic can be reused as they are. The overlay
stays a browser source in OBS, because OBS needs a URL for it.

**Open questions**
- Installers and signing: unsigned apps show security warnings on Windows and
  macOS. Windows and macOS have so far only been built in CI, not run by hand.
- Whether to start with a lighter step (tray icon, open the dashboard
  automatically, a "copy overlay URL" button) before a full app.
- Icons for the installers and the window: `.ico` (Windows) and `.icns`
  (macOS) can be made from `assets/icon.svg`, the source of the current icon
  files. Tauri also wants a set of PNG sizes, which `assets/` already has.

## 4. Fill in the API key, username and settings from the app

**Goal.** No hand-editing of `config.toml` or `.env`: the YouTube API key, the
TikTok username and the other settings are entered in the app.

**Today.** Settings changed in the dashboard (moderation, volume, blacklist,
chat history, TikTok username) are saved to `config.settings.toml` next to `config.toml` and
come back at the next start (done; `config.toml` itself is never written, so its
comments are safe). Secrets still come from the environment or `.env`, and the
program refuses to start without a valid config and an API key.

**What it needs**
- Saving settings to disk: **done** for the dashboard settings and the TikTok
  username, in a second file the program owns. The username can also be left
  empty in `config.toml` and entered on the Settings page, and it takes effect
  without a restart. What is still missing is saving what only `config.toml`
  holds today (language, port), and the first-run screen below.
- A first-run screen that runs before the config is complete, so a new install
  can start without a config file and ask for the key (the username already has
  its place on the Settings page). The program still refuses to start without
  an API key.
- A safe home for the API key, probably the operating system's keychain, never
  plain text in a log or in a page. The dashboard must never send the key back
  to the browser once it is saved.
- Validation in the form: for example, test the key with one cheap request and
  say clearly whether it is valid.
- The server only listens on the loopback address by default, and that rule
  must hold for the setup screen too: saving a key is more sensitive than
  approving a video.

**Order.** Items 3 and 4 depend on each other only partly: saving settings and a
first-run screen can be built into the existing web dashboard first, and the
native window added around it later.

## 5. One configurable command per video source

**Goal.** The streamer chooses which chat word means which source, for example
`!yt` for YouTube and `!vt` for TikTok, or any other words they like. It also
makes a settings screen in a future app (items 3 and 4) simple: a small table of
"word, source" rows.

**Today.** Prefix and name are already configurable (`[command]` in
`config.toml`, default `!v`), but there is exactly one command and it means
YouTube. The name is checked at startup (1 to 32 characters), the prefix too,
and matching is case-insensitive on the name and whole-word, so `!vid` is not
`!v`.

**What it needs**
- Config for several commands, each with a name and a source, for example a
  list of `{ name, source }` entries next to the shared prefix. The current
  `name` keeps working as one entry (an additive change, so existing configs do
  not break).
- Startup validation: names must be unique (ignoring case), and each source must
  be one the program has. A name that is the start of another one is fine,
  because names match as whole words.
- The parser returns which source was asked for, and the queue and events carry
  it. This is the same `source` field that item 1 sketches, so the two should be
  designed together; this item can be built first with YouTube as the only
  source.
- Dashboard and notice text that says "`!v`" must read the configured names
  from the config instead of assuming one.

**Decided**
- **Strict.** A command only accepts links of its own source: `!yt` with a
  TikTok link is rejected as the wrong command, not guessed from the link.
- **One shared prefix** for all commands (`!yt`, `!vt`), not one per command.
  Each entry is just a name and a source.
- **No separate alias feature.** Several words for one source (`!v` and `!yt`
  both meaning YouTube) are written as several entries with the same source.
- **The rejection notice names the right command** ("use `!vt` for TikTok
  links"), read from the same list of commands, not written into the text.

## 6. An icon inside the Windows program file

**Goal.** `stream-spectre.exe` shows the ghost icon in Explorer, the taskbar
and the Alt+Tab list, instead of the plain default.

**Today.** The program has no icon of its own on any system. The dashboard has
a favicon, and `assets/` holds the icon as SVG and PNG, but nothing is
embedded in the binary. Only Linux has been used so far, where a bare
executable has no icon anyway.

**What it needs**
- A `.ico` file made from `assets/icon.svg` (several sizes in one file).
- A build script that embeds it in the Windows build only, which means a
  build-time dependency such as a resource-embedding crate. Its maintenance
  status must be checked on crates.io first, as with every new crate here.
- A look at the result on a real Windows machine; CI only builds there so far.

**Order.** Not needed until Windows users exist. If item 3 is built, its
installer tooling may do this on its own, which would make this item
unnecessary.
