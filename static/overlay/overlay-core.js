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
  var DEFAULT_REQUESTED_BY = 'Requested by {name}';

  function isVideoId(value) {
    return typeof value === 'string' && VIDEO_ID.test(value);
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

  /* Error codes come from the player; keep a short safe token. */
  function sanitizeCode(code) {
    var cleaned = String(code).replace(/[^A-Za-z0-9_.-]/g, '').slice(0, 16);
    return cleaned || 'unknown';
  }

  /* deps:
   *   player: { play(videoId, volume), stop(), setVolume(volume),
   *             position() -> seconds into the video, or null when not playing }
   *   ui:     { show({ requestedBy, title }), hide() }
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
    var unsent = null; // a report that could not be sent yet

    function report(message) {
      if (!send(message)) unsent = message;
    }

    function clear() {
      current = null;
      reported = false;
      player.stop();
      ui.hide();
    }

    function start(item, requestedVolume) {
      if (!item || typeof item !== 'object' || !Number.isInteger(item.id)) return;
      // Set before playing: the player may report an error synchronously.
      current = { id: item.id };
      reported = false;
      unsent = null;
      if (!isVideoId(item.video_id)) {
        reported = true;
        report({ type: 'player_error', item_id: item.id, code: 'invalid' });
        return;
      }
      var requester = item.requested_by;
      var name = requester && typeof requester.name === 'string' ? requester.name : '';
      volume = clampVolume(requestedVolume, volume);
      player.play(item.video_id, volume);
      ui.show({
        requestedBy: format(text('overlay.requested_by') || DEFAULT_REQUESTED_BY, { name: name }),
        title: typeof item.title === 'string' ? item.title : '',
      });
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
        send({ type: 'progress', item_id: current.id, position_secs: Math.floor(seconds) });
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
    format: format,
    isVideoId: isVideoId,
    sanitizeCode: sanitizeCode,
  };
});
