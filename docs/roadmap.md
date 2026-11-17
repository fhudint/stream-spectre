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

**What was checked** (2026-11-17, with `curl`, public links only)
- A link from the web (`tiktok.com/@name/video/<19 digits>?...`) carries the
  video ID in its path. The query string can be dropped.
- A link from the app (`vt.tiktok.com/<code>/`) carries no ID. Asking for it
  without following redirects gave `301` with `Location:
  https://www.tiktok.com/@name/video/<id>?...`, so the ID and the account name
  come from that one header. A made-up code gave `302` to the home page, which
  has no `/video/` in it.
- oEmbed (`tiktok.com/oembed?url=...`) answered 200 for both kinds of link, but
  only with an `<iframe>` pointing at `tiktok.com/player/v1/<id>` and the ID
  (`embed_product_id`). **No title, author, thumbnail or duration.** For a short
  link it also resolves the ID. For a made-up short code it answered 400.
- oEmbed answered 200 for a video ID that does not exist
  (`7000000000000000001`), so it cannot tell whether a video exists.
- `tiktok.com/player/v1/<id>` answered 200 with no `X-Frame-Options` and no
  `frame-ancestors`, so the headers do not stop it from being shown in an
  iframe on another origin.

**Open questions**
- Does the player report "ended" and errors back to the page? TikTok is
  believed to have a `postMessage` API for the embed player (*unverified*: its
  page is a JavaScript shell, so `curl` cannot show it; it needs a real browser
  or OBS). Without an "ended" signal the overlay would have to move on by a
  timer or by the skip button. Does it need a non-IP origin like YouTube's did
  (*unverified*)?
- With no duration known before approval, the maximum-duration rule cannot be
  enforced up front for TikTok. Manual approval, by someone who can see the
  video in a preview, is the only filter, so the default for TikTok should stay
  manual.
- With no title or thumbnail, a queue card can show only the account name and
  the ID. A preview inside the dashboard (the same embed in an iframe) may be
  the way to let a moderator look before approving.
- A video that is private, deleted or blocked in a country cannot be told apart
  from a good one before it plays, so the overlay needs to skip a video that
  fails to load.
- Short links need one request to TikTok's own host. It is a security risk
  (SSRF) for text a stranger typed, so it must be limited: only
  `vt.tiktok.com` and `vm.tiktok.com`, a code checked by a regex, `https` only,
  redirects not followed (read `Location` and check its host and path), a short
  timeout, and the result cached.
- Copyright and platform rules for re-playing other people's TikToks on stream
  need the same warning as YouTube, and probably a stricter default.

**Built.** TikTok videos can be asked for with a command whose source is
`tiktok` (set on the Rules page or in `[command]`). Not tried on a real stream
or in OBS yet.
- What a viewer types after the command: a bare video ID
  (`7700124541145599252`), a bare short code (`ZSpGELffb`, case kept), a web
  link (`tiktok.com/@name/video/<id>`, also `www.` and `m.`), or a short link
  (`vt.tiktok.com/<code>`, `vm.`). Anything else is refused. A command takes
  only its own source; a link of the other one is answered with the command to
  use. A YouTube ID made only of letters and digits also looks like a code
  after the TikTok word; it is asked of TikTok, leads nowhere and costs one
  cached request.
- The resolver builds the address itself from a validated code, never follows a
  redirect, checks `Location`, never reads the body, has a 5 second timeout and
  caches.
- One queue for both. A TikTok entry shows the account (when known) or the ID,
  has no length or thumbnail, and always waits for a moderator even with
  automatic approval. The same gates apply, and the blacklist takes TikTok IDs
  in `video_ids` and accounts in `channels`.
- The overlay plays it in TikTok's embedded player (`player/v1/<id>`), in a
  frame in the same stage. It cannot be paused from the dashboard and there is
  no progress. It ends when the player says so (`onStateChange` 0), and a video
  the player cannot play (`onPlayerError`) is skipped with a notice. Checked in
  Firefox with a 23 second video, and with a video that does not exist
  (1001 INVALID_VIDEO); not yet in OBS. A video that has not started within 15
  seconds is skipped as a failure (`timeout`): the player does not play videos
  with a product link (affiliate, the yellow basket) in a browser and stays
  silent. A video that runs but never says it is over stays until a moderator
  skips it.
- Sound. The player mutes every video it starts. Checked in a real browser
  (Firefox, headless): the player takes commands by `postMessage`
  (`setVolume` with 0 to 100, which also unmutes, `unMute`, `mute`, `play`) and
  reports back (`onMute`, `onStateChange`, `onCurrentTime`, `onVolumeChange`).
  The overlay asks for sound as soon as the player speaks and follows the
  dashboard's volume. Where the browser allows sound (OBS) the video then plays
  with sound at that volume; where it refuses (an ordinary tab) the video is
  paused by the browser, so the overlay mutes it and sets it playing again.
  Both outcomes were checked by switching Firefox's autoplay setting; OBS
  itself has been confirmed only for the muted start, not yet for the fix.

**Still open**
- Whether the embedded player works in OBS as it does in a browser (autoplay
  with sound, no error like YouTube's 150 for an IP address origin).
- Using more of what the player reports. `onCurrentTime` carries the position
  and the **length** (`{"currentTime": 7.28, "duration": 23.3}`), about twice a
  second. That could show progress and the length on the dashboard, enforce a
  maximum length (only after the video has started, since the length is not
  known before; whether to skip a video that is too long, or let a moderator
  decide, is open) and allow pause (`pause` is believed to be a command, like
  `play`; not tried). Not built.
- A preview of the video inside the dashboard, so a moderator can look before
  approving, since there is no title or thumbnail.
- A length limit for TikTok, which needs the length from somewhere.

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
  empty in `config.toml` and entered on the Account page, and it takes effect
  without a restart. What is still missing is saving what only `config.toml`
  holds today (language, port), and the first-run screen below.
- A first-run screen that runs before the config is complete, so a new install
  can start without a config file and ask for the key. The program can already
  start without a key or a username, and the Account page is where both are
  entered.
- A safe home for the API key, probably the operating system's keychain, never
  plain text in a log or in a page. The dashboard must never send the key back
  to the browser once it is saved.
- Validation in the form: for example, test the key with one cheap request and
  say clearly whether it is valid.
- The server only listens on the loopback address by default, and that rule
  must hold for the setup screen too: saving a key is more sensitive than
  approving a video.

**Decided for the account menu (the avatar at the top right).** Built, except
for what item 8 covers.
- The avatar opens a small menu with the channel name and three entries
  (TikTok username, YouTube API key, dashboard token), each leading to an
  `#account` page. The username form moves there from Settings.
- The API key is stored in a separate secrets file readable only by its owner
  (mode 0600 on Linux and macOS). It is never sent back to the browser (the
  page only learns that it is set, and its last four characters), is checked
  with one cheap YouTube request before it is saved, and never appears in a
  log, a response or an event.
- A key saved from the dashboard wins over the environment variable, and the
  log at startup says so, as for the saved settings. The program may start
  without a key: video requests are then refused with a clear notice until one
  is entered. A new key takes effect at once, and the lookup cache and the
  "quota exhausted" pause are cleared.
- Credentials can only be changed from the computer the program runs on
  (a loopback connection), even when the dashboard is opened to the network
  with a token.
- The dashboard token is only *shown* (in use or not) for now; it is still set
  in `.env`. See item 8.
- The program can start without a key, a username, or both, and says clearly
  what is missing. Starting without a config file at all is still not possible.

**Order.** Items 3 and 4 depend on each other only partly: saving settings and a
first-run screen can be built into the existing web dashboard first, and the
native window added around it later.

## 5. One configurable command per video source

**Goal.** The streamer chooses which chat word means which source, for example
`!yt` for YouTube and `!vt` for TikTok, or any other words they like. It also
makes a settings screen in a future app (items 3 and 4) simple: a small table of
"word, source" rows.

**Today (built).** `[command]` has one shared prefix and a list of
`{ name, source }` entries; the old `name = "v"` form still reads as one entry.
Names are checked at startup and when changed (1 to 32 letters, digits or `_`;
unique ignoring case; 1 to 8 entries). They can be changed on the dashboard's
Rules page and take effect for the next chat message. Matching is
case-insensitive and whole-word, so `!vid` is not `!v` and `!vt` is not `!v`.

**Also built (with item 1).** The sources are `youtube` and `tiktok`; the
parser returns which one a command is for, the queue and events carry it
(`source`), and a link of the other source is answered with the configured
command for it ("That is a TikTok link: use `!vt` for TikTok"), read from the
same list.

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

## 7. Stronger storage for secrets

**Goal.** Keep the YouTube API key (and later other secrets) somewhere safer
than a plain file.

**Today.** The first version stores secrets in a file only its owner
can read (0600). That protects against other users of the same computer, but
the key is still plain text on disk, and on Windows the 0600 mode does not
exist: protection there comes only from the user's profile folder.

**Ideas**
- The operating system's keychain (macOS Keychain, Windows Credential Manager,
  the Secret Service on Linux), probably through a crate such as `keyring`. Its
  maintenance must be checked on crates.io first. On Linux it needs a running
  Secret Service, which a bare server or a minimal desktop may not have, so a
  fallback to the file would still be needed.
- On Windows, set an access control list on the secrets file so only the
  current user can read it.
- Rewriting `.env` was considered and not chosen: the program would be writing
  to a file the user edits by hand, with the same problems as `config.toml`.

**Decision needed.** Whether the extra complexity is worth it for a program
that runs on the streamer's own computer. Most useful once the desktop app
(item 3) exists, because installers can ask the system for the keychain.

## 8. Regenerating the dashboard token from the dashboard

**Goal.** Replace a leaked or old `DASHBOARD_TOKEN` without editing `.env` and
restarting.

**Today.** The account page only shows whether a token is in use.

**Decided.** The token can only be *regenerated*, not typed in: a button makes
a new random token, shows it once, and the old one stops working at once. It
is never sent to the browser again afterwards.

**What it needs**
- A clear warning before and after: the overlay address in OBS contains the
  token, so it must be updated or the overlay stops working, and every
  moderator's session needs the new token.
- The same rule as other credentials: only from the computer the program runs
  on.
- Saving the new token to the secrets file, and a decision on what happens to
  open WebSocket connections (probably closed, so they sign in again).
- Care when no token was set at all: the program refuses to listen beyond
  loopback without one, so a token must never be removed while it is.

## 9. Several chat sessions at once (for example YouTube Live chat)

**Goal.** Read the chat of more than one platform in the same run, for example
TikTok Live and YouTube Live, with each one connected or disconnected on its
own.

**Today.** One chat source runs at a time, chosen by `chat.source` (only
`tiktok` exists). The `ChatSource` trait and the reconnecting supervisor are
platform-independent, so another source is an implementation of the trait. The
account menu's "Disconnect chat" switches that one source off and on; the
username is the TikTok one.

**What it needs**
- A decision on how sources are chosen: a list in the config instead of a single
  `chat.source`, each with its own settings table.
- The pipeline to run one supervisor per source, and the chat status
  (`chat_status`, `hello.chat`) to say which source it is about, which is a
  change to the protocol.
- "Disconnect chat" to become one switch per source, in the menu or on the
  account page.
- The command parser, the sender gate (role and levels) and the cooldowns to
  work across sources: a viewer's identity on one platform is not the same
  identity on another.
- YouTube Live chat has its own rules and quota (the chat API costs quota units),
  which must be checked before it is built; none of that has been looked at.
