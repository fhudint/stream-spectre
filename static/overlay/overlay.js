/* Overlay page: connects the WebSocket, the YouTube player and the DOM to the
 * logic in overlay-core.js.
 *
 * Open it as /overlay/ (add ?token=... when a dashboard token is set).
 * Texts that come from other people (names, titles) are only ever assigned
 * with textContent, never as markup.
 */
(function () {
  'use strict';

  var core = window.OverlayCore;
  var params = new URLSearchParams(location.search);
  var token = params.get('token') || '';

  var texts = Object.create(null);
  var socket = null;
  var attempt = 0;

  var root = document.getElementById('root');
  var requester = document.getElementById('requester');
  var title = document.getElementById('title');

  var ui = {
    show: function (info) {
      requester.textContent = info.requestedBy;
      title.textContent = info.title;
      root.classList.remove('hidden');
    },
    hide: function () {
      requester.textContent = '';
      title.textContent = '';
      root.classList.add('hidden');
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
    var wantPaused = false; // a moderator paused this video

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
            // A video that is loaded while paused starts, so stop it again.
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
        wantPaused = false; // a new video always plays
        volume = requestedVolume;
        if (failed) {
          callbacks.onError('api_unavailable');
        } else if (!ready) {
          pending = { videoId: videoId, volume: requestedVolume };
        } else {
          load({ videoId: videoId, volume: requestedVolume });
        }
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
      /* Seconds into the video while it plays, otherwise null. */
      position: function () {
        if (!ready || !playing || typeof yt.getCurrentTime !== 'function') return null;
        return yt.getCurrentTime();
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
    send: send,
    text: function (key) {
      return texts[key];
    },
  });

  /* Reconnects with a growing delay, up to ten seconds. */
  function connect() {
    var scheme = location.protocol === 'https:' ? 'wss:' : 'ws:';
    var query = token ? '?token=' + encodeURIComponent(token) : '';
    var ws = new WebSocket(scheme + '//' + location.host + '/ws/overlay' + query);
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
})();
