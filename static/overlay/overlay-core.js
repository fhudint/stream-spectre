/* Overlay logic with no browser dependencies, so it can be tested with Node.
 *
 * It turns server events (see docs/websocket-events.md) into calls on a
 * `player` (play, stop, setVolume) and a `ui` (show, hide), and turns what the
 * player reports into messages for the server. overlay.js connects it to the
 * real WebSocket, YouTube player and page.
 */
(function (root, factory) {
  'use strict';
  var api = factory();
  if (typeof module === 'object' && module.exports) {
    module.exports = api;
  } else {
    root.OverlayCore = api;
  }
})(typeof self !== 'undefined' ? self : this, function () {
  'use strict';

  var PROTOCOL_VERSION = 1;
  var VIDEO_ID = /^[A-Za-z0-9_-]{11}$/;
  var TIKTOK_VIDEO_ID = /^[0-9]{15,20}$/;
  var DEFAULT_REQUESTED_BY = 'Requested by {name}';
  var DEFAULT_PAUSED = 'Paused';

  function isVideoId(value) {
    return typeof value === 'string' && VIDEO_ID.test(value);
  }

  /* Whether `value` is an ID a video of `source` can have. An item from a
   * server that predates TikTok has no source, which means YouTube. */
  function isVideoRef(source, value) {
    if (typeof value !== 'string') return false;
    if (source === undefined || source === 'youtube') return VIDEO_ID.test(value);
    if (source === 'tiktok') return TIKTOK_VIDEO_ID.test(value);
    return false;
  }

  /* Replaces {name} placeholders in one pass. Unknown placeholders and stray
   * braces are kept, and inserted values are never scanned again. */
  function format(template, values) {
    var out = '';
    var rest = String(template);
    for (;;) {
      var open = rest.indexOf('{');
      if (open < 0) break;
      out += rest.slice(0, open);
      var after = rest.slice(open + 1);
      var close = after.indexOf('}');
      if (close < 0) {
        out += '{';
        rest = after;
        continue;
      }
      var name = after.slice(0, close);
      if (name.indexOf('{') >= 0) {
        out += '{';
        rest = after;
        continue;
      }
      out += Object.prototype.hasOwnProperty.call(values, name)
        ? String(values[name])
        : '{' + name + '}';
      rest = after.slice(close + 1);
    }
    return out + rest;
  }

  function clampVolume(value, fallback) {
    if (typeof value !== 'number' || !isFinite(value)) return fallback;
    return Math.min(100, Math.max(0, Math.round(value)));
  }

  /* How much of a video is done, as a percentage from 0 to 100, for the line
   * under it. 0 while the length or the position is not known. */
  function progressPercent(position, duration) {
    if (typeof position !== 'number' || typeof duration !== 'number') return 0;
    if (!isFinite(position) || !isFinite(duration) || duration <= 0 || position <= 0) return 0;
    return Math.min(100, (position / duration) * 100);
  }

  /* Error codes come from the player; keep a short safe token. */
  function sanitizeCode(code) {
    var cleaned = String(code).replace(/[^A-Za-z0-9_.-]/g, '').slice(0, 16);
    return cleaned || 'unknown';
  }

  /* Sound for TikTok's embedded player.
   *
   * The player starts every video muted (it does so itself, so that a browser
   * allows autoplay). It takes commands and reports by `postMessage`, always
   * with `"x-tiktok-player": true`: `setVolume` (0 to 100, which also
   * unmutes), `unMute`, `mute` and `play` as commands; `onMute`,
   * `onStateChange` (2 is paused) and `onCurrentTime` among its reports.
   *
   * This asks the player for sound as soon as it speaks, and again a few
   * times if it stays muted. If the browser refuses sound without a click, it
   * pauses the video right after; then the player is muted and set playing
   * again, so the picture goes on, silent. OBS lets pages play sound, so there
   * the first ask is enough.
   *
   * A pause the moderators asked for also makes the player report state 2;
   * `paused()` tells that apart from the browser refusing.
   *
   * deps: post(command), volume() -> 0..100, now() -> milliseconds,
   * paused() -> whether a moderator paused the video (optional).
   */
  var TIKTOK_ENDED = 0;
  var TIKTOK_PAUSED = 2;
  var TIKTOK_MAX_ASKS = 6;
  /* A pause this soon after asking for sound is the browser refusing... */
  var TIKTOK_REFUSAL_MS = 3000;
  /* ...unless the video is at its end, which also pauses before it ends. */
  var TIKTOK_END_MARGIN_SECS = 1;

  /* What a message from TikTok's player says about how the video went, as
   * { ended: true } (it finished: state 0, as for YouTube) or
   * { ended: false, code } (it could not play: `onPlayerError`, for example
   * 1001 INVALID_VIDEO for a video that does not exist), or null for any
   * other message. */
  function tiktokOutcome(data) {
    if (!data || typeof data !== 'object' || data['x-tiktok-player'] !== true) return null;
    if (data.type === 'onStateChange' && data.value === TIKTOK_ENDED) return { ended: true };
    if (data.type === 'onPlayerError') {
      var detail = data.value && typeof data.value === 'object' ? data.value : {};
      var code = Number.isInteger(detail.errorCode)
        ? String(detail.errorCode)
        : typeof detail.errorType === 'string'
          ? detail.errorType
          : 'tiktok';
      return { ended: false, code: code };
    }
    return null;
  }

  /* How long TikTok's player gets to start a video before the overlay gives
   * up on it. A video it never starts (one with a product link, for example,
   * which TikTok only plays in its own app) says nothing at all. */
  var TIKTOK_START_MS = 15000;
  var TIKTOK_PLAYING = 1;

  /* Whether a message from TikTok's player shows that the video is running:
   * it is playing (state 1), or its clock ticks (`onCurrentTime`). */
  function tiktokStarted(data) {
    if (!data || typeof data !== 'object' || data['x-tiktok-player'] !== true) return false;
    return (data.type === 'onStateChange' && data.value === TIKTOK_PLAYING) || data.type === 'onCurrentTime';
  }

  function createTiktokSound(deps) {
    var muted;
    var asks;
    var askedAt;
    var silent; // the browser refused sound: stay muted
    var position; // seconds into the video, and how long it is, as last told
    var length;

    function begin() {
      muted = null;
      asks = 0;
      askedAt = null;
      silent = false;
      position = null;
      length = null;
    }

    function atTheEnd() {
      return length !== null && position !== null && position >= length - TIKTOK_END_MARGIN_SECS;
    }

    function ask() {
      asks += 1;
      askedAt = deps.now();
      deps.post({ type: 'setVolume', value: deps.volume() });
      deps.post({ type: 'unMute' });
    }

    function wantsAsking() {
      return !silent && muted !== false && asks < TIKTOK_MAX_ASKS;
    }

    begin();
    return {
      begin: begin,

      /* One message from the player; anything else is ignored. */
      handle: function (data) {
        if (!data || typeof data !== 'object' || data['x-tiktok-player'] !== true) return;
        if (typeof data.type !== 'string') return;
        if (data.type === 'onMute') muted = data.value === true;
        if (data.type === 'onCurrentTime' && data.value && typeof data.value === 'object') {
          position = Number.isFinite(data.value.currentTime) ? data.value.currentTime : position;
          length = Number.isFinite(data.value.duration) ? data.value.duration : length;
        }
        if (
          data.type === 'onStateChange' &&
          data.value === TIKTOK_PAUSED &&
          !(deps.paused && deps.paused()) &&
          !silent &&
          !atTheEnd() &&
          askedAt !== null &&
          deps.now() - askedAt <= TIKTOK_REFUSAL_MS
        ) {
          silent = true;
          deps.post({ type: 'mute' });
          deps.post({ type: 'play' });
          return;
        }
        if (wantsAsking()) ask();
      },

      /* The volume changed while the video plays. */
      setVolume: function () {
        if (!silent && muted === false) deps.post({ type: 'setVolume', value: deps.volume() });
      },

      state: function () {
        return { muted: muted, asks: asks, silent: silent };
      },
    };
  }

  /* deps:
   *   player: { play(videoId, volume, source), stop(), setVolume(volume),
   *             pause(), resume(),
   *             position() -> seconds into the video, or null when not playing }
   *   ui:     { show({ requestedBy, title }), hide(),
   *             setPaused(paused, label) (optional): the "Paused" notice }
   *   send:   (message) => boolean, false when the socket is not open
   *   text:   (key) => string | undefined, for translated UI texts
   */
  function createController(deps) {
    var player = deps.player;
    var ui = deps.ui;
    var send = deps.send;
    var text = deps.text || function () { return undefined; };

    var lastSeq = 0;
    var volume = 50;
    var current = null; // { id } of the video that should be on screen
    var reported = false; // ended or error already reported for `current`
    var paused = false; // a moderator paused `current`
    var unsent = null; // a report that could not be sent yet

    function report(message) {
      if (!send(message)) unsent = message;
    }

    /* Pauses or resumes the video on screen; saying what is already so does
     * nothing. */
    function setPaused(flag) {
      if (flag === paused) return;
      paused = flag;
      var action = flag ? player.pause : player.resume;
      if (typeof action === 'function') action.call(player);
      showPaused();
    }

    /* Tells the screen whether to say "Paused" over the video. */
    function showPaused() {
      if (typeof ui.setPaused === 'function') ui.setPaused(paused, text('overlay.paused') || DEFAULT_PAUSED);
    }

    function clear() {
      current = null;
      reported = false;
      paused = false;
      player.stop();
      ui.hide();
    }

    function start(item, requestedVolume) {
      if (!item || typeof item !== 'object' || !Number.isInteger(item.id)) return;
      // Set before playing: the player may report an error synchronously.
      current = { id: item.id };
      reported = false;
      paused = false; // a video that starts always plays
      unsent = null;
      if (!isVideoRef(item.source, item.video_id)) {
        reported = true;
        report({ type: 'player_error', item_id: item.id, code: 'invalid' });
        return;
      }
      var requester = item.requested_by;
      var name = requester && typeof requester.name === 'string' ? requester.name : '';
      volume = clampVolume(requestedVolume, volume);
      player.play(item.video_id, volume, item.source === 'tiktok' ? 'tiktok' : 'youtube');
      ui.show({
        requestedBy: format(text('overlay.requested_by') || DEFAULT_REQUESTED_BY, { name: name }),
        title: typeof item.title === 'string' ? item.title : '',
      });
      showPaused(); // a new video always plays: the notice goes away
    }

    function applyHello(message) {
      var data = message.data && typeof message.data === 'object' ? message.data : {};
      if (Number.isInteger(message.seq)) lastSeq = message.seq;
      volume = clampVolume(data.volume, volume);
      var playing = data.queue && data.queue.now_playing;
      var id = playing && Number.isInteger(playing.id) ? playing.id : null;
      if (unsent && unsent.item_id !== id) unsent = null;
      if (id === null) {
        clear();
        player.setVolume(volume);
      } else if (current && current.id === id) {
        // The same video is still the current one (reconnect): keep playing.
        player.setVolume(volume);
      } else {
        start(playing, volume);
      }
      // A page that opens, or reconnects, in the middle of a pause stays paused.
      if (id !== null) setPaused(data.playback_paused === true);
    }

    function handle(message) {
      if (!message || typeof message !== 'object') return;
      if (message.v !== PROTOCOL_VERSION) return;
      if (message.type === 'hello') {
        applyHello(message);
        return;
      }
      // Events already included in the last snapshot, or repeated.
      if (!Number.isInteger(message.seq) || message.seq <= lastSeq) return;
      lastSeq = message.seq;
      var data = message.data && typeof message.data === 'object' ? message.data : {};
      switch (message.type) {
        case 'play':
          start(data.item, data.volume);
          break;
        case 'skip':
          if (current && current.id === data.item_id) clear();
          break;
        case 'stop':
          clear();
          break;
        case 'volume':
          volume = clampVolume(data.volume, volume);
          player.setVolume(volume);
          break;
        case 'playback_paused':
          if (current && current.id === data.item_id) setPaused(data.paused === true);
          break;
        default:
          break; // events this overlay does not use
      }
    }

    return {
      handle: handle,

      /* The video finished. */
      onPlayerEnded: function () {
        if (!current || reported) return;
        reported = true;
        report({ type: 'ended', item_id: current.id });
      },

      /* The player could not play the video. */
      onPlayerError: function (code) {
        if (!current || reported) return;
        reported = true;
        report({ type: 'player_error', item_id: current.id, code: sanitizeCode(code) });
      },

      /* Called about once a second. Tells the server how far the video has
       * got, so the dashboard can show it. A report that cannot be sent is
       * simply dropped: the next one replaces it. */
      tick: function () {
        if (!current || reported || typeof player.position !== 'function') return;
        var seconds = player.position();
        if (typeof seconds !== 'number' || !isFinite(seconds) || seconds < 0) return;
        var message = { type: 'progress', item_id: current.id, position_secs: Math.floor(seconds) };
        // A TikTok video's length is only known to its player.
        var length = typeof player.duration === 'function' ? player.duration() : null;
        if (typeof length === 'number' && isFinite(length) && length > 0) message.duration_secs = Math.ceil(length);
        send(message);
      },

      /* The socket (re)connected. */
      onSocketOpen: function () {
        send({ type: 'player_ready' });
        if (unsent && send(unsent)) unsent = null;
      },

      state: function () {
        return { lastSeq: lastSeq, volume: volume, current: current, reported: reported, unsent: unsent };
      },
    };
  }

  return {
    PROTOCOL_VERSION: PROTOCOL_VERSION,
    createController: createController,
    createTiktokSound: createTiktokSound,
    tiktokOutcome: tiktokOutcome,
    progressPercent: progressPercent,
    tiktokStarted: tiktokStarted,
    TIKTOK_START_MS: TIKTOK_START_MS,
    format: format,
    isVideoId: isVideoId,
    isVideoRef: isVideoRef,
    sanitizeCode: sanitizeCode,
  };
});
