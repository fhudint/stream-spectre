/* Overlay page: connects the WebSocket, the YouTube player and the DOM to the
 * logic in overlay-core.js.
 *
 * Open it as /overlay/mediashare/ (add ?token=... when a dashboard token is
 * set, and theme=name to give this one source a look of its own).
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
  var barFill = document.getElementById('bar-fill');
  var pausedLabel = document.getElementById('paused-label');
  // The texts of the card; some themes draw a label before a few of them.
  var fields = {};
  ['kicker', 'left', 'requested', 'name', 'role', 'title', 'channel', 'time', 'avatar'].forEach(function (id) {
    fields[id] = document.getElementById(id);
  });
  // ?theme=name shows that look whatever the dashboard chose, for one source.
  var forcedTheme = core.isTheme(params.get('theme')) ? params.get('theme') : '';

  function put(id, value) {
    fields[id].textContent = value;
  }

  var noticeBox = document.getElementById('notice');
  var noticeTimer = null;

  var ui = {
    /* The developer's notice, for a few seconds over whatever is on screen. */
    notice: function (info) {
      document.getElementById('notice-badge').textContent = info.badge;
      document.getElementById('notice-name').textContent = '@' + info.name.replace(/^@+/, '');
      document.getElementById('notice-text').textContent = info.text;
      noticeBox.classList.remove('hidden');
      clearTimeout(noticeTimer);
      noticeTimer = setTimeout(function () {
        noticeBox.classList.add('hidden');
      }, info.durationMs);
    },
    show: function (info) {
      put('kicker', texts['overlay.kicker'] || 'Request from chat');
      fields.name.setAttribute('data-label', texts['overlay.label.from'] || 'From');
      fields.role.setAttribute('data-label', texts['overlay.label.role'] || 'Status');
      fields.time.setAttribute('data-label', texts['overlay.label.time'] || 'Played');
      put('requested', info.requestedBy);
      put('name', info.name);
      put('role', info.role);
      put('title', info.title);
      put('channel', info.channel);
      put('avatar', info.name.replace(/^@/, '').slice(0, 1).toUpperCase());
      root.classList.remove('hidden');
    },
    hide: function () {
      Object.keys(fields).forEach(function (id) {
        put(id, '');
      });
      this.setPaused(false, '');
      root.classList.add('hidden');
    },
    /* The look of the overlay: a name the styles know (see overlay.css). */
    setTheme: function (name) {
      root.setAttribute('data-theme', forcedTheme || name);
    },
    /* The "Paused" notice over the video, and the level meter standing still. */
    setPaused: function (paused, label) {
      pausedLabel.textContent = paused ? label : '';
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
      // A pause from a moderator is not the browser refusing sound.
      paused: function () {
        return wantPaused;
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
      // The player drops commands until it plays, so a video that was paused
      // before that is paused as soon as it does.
      if (wantPaused && data && data.type === 'onStateChange' && data.value === 1) {
        postTiktok({ type: 'pause' });
      }
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
        wantPaused = true;
        if (tiktokOn) {
          postTiktok({ type: 'pause' });
          return;
        }
        if (ready && typeof yt.pauseVideo === 'function') yt.pauseVideo();
      },
      resume: function () {
        wantPaused = false;
        if (tiktokOn) {
          postTiktok({ type: 'play' });
          return;
        }
        if (ready && typeof yt.playVideo === 'function') yt.playVideo();
      },
      stop: function () {
        pending = null;
        wantPaused = false;
        playing = false;
        hideTiktok();
        if (ready) yt.stopVideo();
      },
      /* How far the video is, as { position, duration } in seconds (a number
       * is NaN when not known), for the line under it and the clock. Unlike
       * `position` it also tells while paused. */
      clock: function () {
        if (tiktokOn) return tiktokClock || { position: NaN, duration: NaN };
        if (!ready || typeof yt.getCurrentTime !== 'function' || typeof yt.getDuration !== 'function') {
          return { position: NaN, duration: NaN };
        }
        return { position: yt.getCurrentTime(), duration: yt.getDuration() };
      },
      /* Seconds into the video while it plays, otherwise null. For TikTok it
       * is what its player last said. */
      position: function () {
        if (tiktokOn) return tiktokClock ? tiktokClock.position : null;
        if (!ready || !playing || typeof yt.getCurrentTime !== 'function') return null;
        return yt.getCurrentTime();
      },
      /* How long the video is, as its player says, or null. The server uses
       * it in preference to the one in the queue item. */
      duration: function () {
        var length = this.clock().duration;
        return length > 0 ? length : null;
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
    var ws = new WebSocket(scheme + '//' + location.host + '/ws/overlay/mediashare' + query);
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
    var clock = player.clock();
    barFill.style.transform =
      'scaleX(' + (core.progressPercent(clock.position, clock.duration) / 100).toFixed(4) + ')';
    var shown = core.clockTexts(clock.position, clock.duration, texts['overlay.left']);
    put('time', shown.time);
    put('left', shown.left);
  }, 250);
})();
