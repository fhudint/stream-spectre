/* Song overlay page: connects the WebSocket and the YouTube player to the
 * logic in ../mediashare/overlay-core.js (the same as the mediashare overlay's,
 * which also plays only YouTube here: a song is a YouTube video).
 *
 * Open it as /overlay/song/ (add ?token=... when a dashboard token is set).
 * The YouTube player is the cover of the card and stays visible while a song
 * plays: YouTube does not allow it hidden or smaller than 200 x 200.
 * Texts that come from other people (names, titles, messages) are only ever
 * assigned with textContent, never as markup.
 */
(function () {
  'use strict';

  var core = window.OverlayCore;
  var params = new URLSearchParams(location.search);
  var token = params.get('token') || '';

  // ?theme=name shows that look whatever the dashboard chose, for one source.
  var forcedTheme = core.isTheme(params.get('theme')) ? params.get('theme') : '';

  var texts = Object.create(null);
  var socket = null;
  var attempt = 0;

  var root = document.getElementById('root');
  var barFill = document.getElementById('bar-fill');
  var fields = {};
  ['kicker', 'title', 'channel', 'requested', 'message', 'time', 'left', 'paused'].forEach(function (id) {
    fields[id] = document.getElementById(id);
  });

  function put(id, value) {
    fields[id].textContent = value;
  }

  var ui = {
    show: function (info) {
      put('kicker', texts['overlay.song.kicker'] || 'Song request');
      put('title', info.videoTitle || info.title);
      put('channel', info.videoChannel);
      put('requested', info.requestedBy);
      put('message', info.message ? '“' + info.message + '”' : '');
      root.classList.remove('hidden');
    },
    hide: function () {
      Object.keys(fields).forEach(function (id) {
        put(id, '');
      });
      barFill.style.transform = 'scaleX(0)';
      root.classList.remove('paused');
      root.classList.add('hidden');
    },
    /* The look of the overlay: a name the styles know (see song.css). */
    setTheme: function (name) {
      root.setAttribute('data-theme', forcedTheme || name);
    },
    /* The "Paused" notice, and the level meter standing still. */
    setPaused: function (paused, label) {
      put('paused', paused ? label : '');
      root.classList[paused ? 'add' : 'remove']('paused');
    },
  };

  function send(message) {
    if (socket && socket.readyState === WebSocket.OPEN) {
      socket.send(JSON.stringify(message));
      return true;
    }
    return false;
  }

  /* The YouTube IFrame Player, loaded from youtube.com. */
  function createPlayer(callbacks) {
    var yt = null;
    var ready = false;
    var failed = false;
    var volume = 50;
    var pending = null;
    var playing = false;
    var wantPaused = false; // a moderator paused this song

    function load(next) {
      playing = false;
      volume = next.volume;
      yt.setVolume(volume);
      yt.loadVideoById(next.videoId);
    }

    window.onYouTubeIframeAPIReady = function () {
      yt = new YT.Player('player', {
        width: '100%',
        height: '100%',
        playerVars: {
          autoplay: 1,
          controls: 0,
          disablekb: 1,
          fs: 0,
          iv_load_policy: 3,
          modestbranding: 1,
          playsinline: 1,
          rel: 0,
          origin: location.origin,
        },
        events: {
          onReady: function () {
            ready = true;
            yt.setVolume(volume);
            if (pending) {
              var next = pending;
              pending = null;
              load(next);
            }
          },
          onStateChange: function (event) {
            playing = event.data === YT.PlayerState.PLAYING;
            // A song that is loaded while paused starts, so stop it again.
            if (playing && wantPaused) yt.pauseVideo();
            if (event.data === YT.PlayerState.ENDED) callbacks.onEnded();
          },
          onError: function (event) {
            callbacks.onError(String(event.data));
          },
        },
      });
    };

    var script = document.createElement('script');
    script.src = 'https://www.youtube.com/iframe_api';
    script.async = true;
    script.onerror = function () {
      failed = true;
      if (pending) {
        pending = null;
        callbacks.onError('api_unavailable');
      }
    };
    document.head.appendChild(script);

    return {
      play: function (videoId, requestedVolume) {
        wantPaused = false; // a new song always plays
        volume = requestedVolume;
        if (failed) callbacks.onError('api_unavailable');
        else if (!ready) pending = { videoId: videoId, volume: requestedVolume };
        else load({ videoId: videoId, volume: requestedVolume });
      },
      pause: function () {
        wantPaused = true;
        if (ready && typeof yt.pauseVideo === 'function') yt.pauseVideo();
      },
      resume: function () {
        wantPaused = false;
        if (ready && typeof yt.playVideo === 'function') yt.playVideo();
      },
      stop: function () {
        pending = null;
        wantPaused = false;
        playing = false;
        if (ready) yt.stopVideo();
      },
      /* How far the song is, as { position, duration } in seconds (NaN when not known). */
      clock: function () {
        if (!ready || typeof yt.getCurrentTime !== 'function' || typeof yt.getDuration !== 'function') {
          return { position: NaN, duration: NaN };
        }
        return { position: yt.getCurrentTime(), duration: yt.getDuration() };
      },
      /* Seconds into the song while it plays, otherwise null. */
      position: function () {
        if (!ready || !playing || typeof yt.getCurrentTime !== 'function') return null;
        return yt.getCurrentTime();
      },
      /* How long the song is, as its player says, or null. */
      duration: function () {
        var length = this.clock().duration;
        return length > 0 ? length : null;
      },
      setVolume: function (requestedVolume) {
        volume = requestedVolume;
        if (ready) yt.setVolume(requestedVolume);
      },
    };
  }

  var controller = null;
  var player = createPlayer({
    onEnded: function () {
      controller.onPlayerEnded();
    },
    onError: function (code) {
      controller.onPlayerError(code);
    },
  });
  controller = core.createController({
    player: player,
    ui: ui,
    overlay: 'song',
    send: send,
    text: function (key) {
      return texts[key];
    },
  });

  /* Reconnects with a growing delay, up to ten seconds. */
  function connect() {
    var scheme = location.protocol === 'https:' ? 'wss:' : 'ws:';
    var query = token ? '?token=' + encodeURIComponent(token) : '';
    var ws = new WebSocket(scheme + '//' + location.host + '/ws/overlay/song' + query);
    ws.onopen = function () {
      socket = ws;
      attempt = 0;
      controller.onSocketOpen();
    };
    ws.onmessage = function (event) {
      var message;
      try {
        message = JSON.parse(event.data);
      } catch (error) {
        return;
      }
      controller.handle(message);
    };
    ws.onclose = function () {
      if (socket === ws) socket = null;
      var delay = Math.min(1000 * Math.pow(2, attempt), 10000);
      attempt += 1;
      setTimeout(connect, delay);
    };
  }

  /* Translated texts are optional: the overlay works with its defaults. */
  var headers = token ? { Authorization: 'Bearer ' + token } : {};
  fetch('/api/i18n', { headers: headers })
    .then(function (response) {
      return response.ok ? response.json() : {};
    })
    .then(function (loaded) {
      Object.assign(texts, loaded);
    })
    .catch(function () {});

  connect();
  setInterval(controller.tick, 1000);
  // The line under the title moves smoothly between these steps (see the CSS).
  setInterval(function () {
    var clock = player.clock();
    barFill.style.transform =
      'scaleX(' + (core.progressPercent(clock.position, clock.duration) / 100).toFixed(4) + ')';
    var shown = core.clockTexts(clock.position, clock.duration, texts['overlay.left']);
    put('time', shown.time);
    put('left', shown.left);
  }, 250);
})();
