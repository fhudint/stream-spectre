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
  var TIKTOK_ORIGIN = 'https://www.tiktok.com';
  var params = new URLSearchParams(location.search);
  var token = params.get('token') || '';

  var texts = Object.create(null);
  var socket = null;
  var attempt = 0;

  var root = document.getElementById('root');
  var stage = document.getElementById('stage');
  var tiktokHost = document.getElementById('tiktok');
  var requester = document.getElementById('requester');
  var title = document.getElementById('title');
  var barFill = document.getElementById('bar-fill');

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
    var tiktokOn = false; // the TikTok frame is what is on screen
    var tiktokTimer = null; // gives up on a TikTok video that never starts
    var tiktokClock = null; // { position, duration } as TikTok's player last said
    var tiktokFrame = null;
    var sound = core.createTiktokSound({
      post: postTiktok,
      volume: function () {
        return volume;
      },
      now: function () {
        return Date.now();
      },
    });

    /* A command for TikTok's player, sent to that origin only. */
    function postTiktok(command) {
      if (!tiktokFrame || !tiktokFrame.contentWindow) return;
      tiktokFrame.contentWindow.postMessage(
        Object.assign({ 'x-tiktok-player': true }, command),
        TIKTOK_ORIGIN
      );
    }

    /* Only what the TikTok frame on screen says is listened to. */
    window.addEventListener('message', function (event) {
      if (!tiktokFrame || event.origin !== TIKTOK_ORIGIN) return;
      if (event.source !== tiktokFrame.contentWindow) return;
      sound.handle(event.data);
      var data = event.data;
      if (data && data.type === 'onCurrentTime' && data.value && typeof data.value === 'object') {
        tiktokClock = { position: data.value.currentTime, duration: data.value.duration };
      }
      if (core.tiktokStarted(event.data)) stopTiktokTimer();
      var outcome = core.tiktokOutcome(event.data);
      if (outcome === null) return;
      stopTiktokTimer();
      if (outcome.ended) callbacks.onEnded();
      else callbacks.onError(outcome.code);
    });

    /* TikTok's embedded player: one frame per video, removed afterwards. The
     * ID is digits only (checked by overlay-core). Its sound is asked for as
     * described at `createTiktokSound`. It says when the video ends and when
     * it cannot be played (`tiktokOutcome`). A video that has not started
     * after `TIKTOK_START_MS` is reported as failed (code `timeout`), so it
     * does not hold the screen; once it runs, only a moderator skipping it
     * ends a video that never says it is over. */
    function showTiktok(videoId) {
      hideTiktok();
      var frame = document.createElement('iframe');
      frame.src =
        'https://www.tiktok.com/player/v1/' +
        encodeURIComponent(videoId) +
        '?autoplay=1&music_info=1&description=1';
      frame.setAttribute('allow', 'autoplay; fullscreen');
      frame.setAttribute('allowfullscreen', '');
      frame.setAttribute('referrerpolicy', 'strict-origin-when-cross-origin');
      tiktokHost.appendChild(frame);
      tiktokFrame = frame;
      tiktokClock = null;
      tiktokTimer = setTimeout(function () {
        tiktokTimer = null;
        if (tiktokFrame === frame) callbacks.onError('timeout');
      }, core.TIKTOK_START_MS);
      sound.begin();
      stage.classList.add('tiktok');
      tiktokOn = true;
    }

    function stopTiktokTimer() {
      if (tiktokTimer !== null) clearTimeout(tiktokTimer);
      tiktokTimer = null;
    }

    function hideTiktok() {
      stopTiktokTimer();
      tiktokHost.replaceChildren();
      tiktokFrame = null;
      stage.classList.remove('tiktok');
      tiktokOn = false;
    }

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
      play: function (videoId, requestedVolume, source) {
        wantPaused = false; // a new video always plays
        volume = requestedVolume;
        if (source === 'tiktok') {
          pending = null;
          playing = false;
          // Whatever YouTube was playing stops, so only one sound is heard.
          if (ready && typeof yt.stopVideo === 'function') yt.stopVideo();
          showTiktok(videoId);
          return;
        }
        hideTiktok();
        if (failed) {
          callbacks.onError('api_unavailable');
        } else if (!ready) {
          pending = { videoId: videoId, volume: requestedVolume };
        } else {
          load({ videoId: videoId, volume: requestedVolume });
        }
      },
      pause: function () {
        if (tiktokOn) return; // its player cannot be paused from here
        wantPaused = true;
        if (ready && typeof yt.pauseVideo === 'function') yt.pauseVideo();
      },
      resume: function () {
        wantPaused = false;
        if (tiktokOn) return;
        if (ready && typeof yt.playVideo === 'function') yt.playVideo();
      },
      stop: function () {
        pending = null;
        wantPaused = false;
        playing = false;
        hideTiktok();
        if (ready) yt.stopVideo();
      },
      /* How much of the video is done, 0 to 100 (0 when not known), for the
       * line under it. Unlike `position` it also tells while paused. */
      percent: function () {
        if (tiktokOn) {
          return tiktokClock ? core.progressPercent(tiktokClock.position, tiktokClock.duration) : 0;
        }
        if (!ready || typeof yt.getCurrentTime !== 'function' || typeof yt.getDuration !== 'function') return 0;
        return core.progressPercent(yt.getCurrentTime(), yt.getDuration());
      },
      /* Seconds into the video while it plays, otherwise null. */
      position: function () {
        if (!ready || !playing || typeof yt.getCurrentTime !== 'function') return null;
        return yt.getCurrentTime();
      },
      setVolume: function (requestedVolume) {
        volume = requestedVolume;
        if (ready) yt.setVolume(requestedVolume);
        if (tiktokOn) sound.setVolume();
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
  // The line under the video moves smoothly between these steps (see the CSS).
  setInterval(function () {
    barFill.style.width = player.percent().toFixed(1) + '%';
  }, 250);
})();
