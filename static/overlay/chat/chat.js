/* Chat overlay page: connects the WebSocket and the DOM to the logic in
 * chat-core.js.
 *
 * Open it as /overlay/chat/panel/ (with a background behind the lines) or
 * /overlay/chat/float/ (transparent); add ?token=... when a dashboard token
 * is set, and theme=name to give this one source a look of its own. Size the
 * OBS browser source to what the look needs: 360 x 640 for most, the width of
 * the scene for the bar (ticker).
 * Names and texts come from viewers: they are only ever assigned with
 * textContent, never as markup.
 */
(function () {
  'use strict';

  var core = window.ChatCore;
  var params = new URLSearchParams(location.search);
  var token = params.get('token') || '';

  var texts = Object.create(null);
  var socket = null;
  var attempt = 0;

  var root = document.getElementById('root');
  var list = document.getElementById('list');
  var panel = document.getElementById('panel');
  // The address says which of the two overlays this is.
  var kind = /\/float\/?$/.test(location.pathname) ? 'float' : 'panel';
  var forcedTheme = core.isTheme(params.get('theme'), kind) ? params.get('theme') : '';

  /* The words around the lines; some looks draw a few of them. Texts that
   * are code-like (the terminal's prompt) are in the style sheet. */
  var WORDS = {
    title: ['overlay.chat.title', 'Live chat'],
    paper: ['overlay.chat.paper', 'Conversation'],
    live: ['overlay.chat.live', 'live'],
    name: ['overlay.chat.name', 'Chat'],
    receipt: ['overlay.chat.receipt', 'CHAT RECEIPT'],
    thanks: ['overlay.chat.thanks', 'THANKS FOR CHATTING'],
  };

  function paintWords() {
    Object.keys(WORDS).forEach(function (id) {
      document.getElementById(id).textContent = texts[WORDS[id][0]] || WORDS[id][1];
    });
  }

  function span(className, text) {
    var node = document.createElement('span');
    node.className = className;
    node.textContent = text;
    return node;
  }

  function div(className, children) {
    var node = document.createElement('div');
    node.className = className;
    children.forEach(function (child) {
      node.appendChild(child);
    });
    return node;
  }

  /* Which chat the line is from: a small picture in the style sheet, named for
   * screen readers. */
  function sourceIcon(source) {
    var node = span('src source-' + source, '');
    node.setAttribute('role', 'img');
    node.setAttribute('aria-label', texts['overlay.chat.source.' + source] || (source === 'youtube' ? 'YouTube' : 'TikTok'));
    return node;
  }

  /* One line of the chat. Every look styles these same parts. */
  function draw(line, fresh) {
    var item = document.createElement('li');
    item.className =
      'msg tint-' + line.tint + (line.command ? ' cmd' : ' say') + (line.gift ? ' gift' : '') + (fresh ? ' enter' : '');
    item.style.setProperty('--rot', line.rotation + 'deg');
    item.appendChild(span('av', line.initial));
    item.appendChild(
      div('body', [
        div('head', [sourceIcon(line.source), span('time', line.time), span('who', line.who), span('badge', line.badge)]),
        div('line', [span('text', line.text), span('status', line.status)]),
      ])
    );
    return item;
  }

  /* More than this much past the edge is a line too many (a note turned a few
   * degrees reaches a little past its own box; a line cut at the edge is fine). */
  var SLACK = 16;

  /* Whether the lines are more than the source has room for: the list, which
   * the look keeps as high as the source or as wide, or the whole panel, which
   * a look that grows with its lines keeps inside the source. */
  function overflows() {
    return (
      list.scrollHeight > list.clientHeight + SLACK ||
      list.scrollWidth > list.clientWidth + SLACK ||
      panel.offsetHeight > root.clientHeight + SLACK
    );
  }

  /* Takes the oldest lines away until the rest fits, so a tall source shows a
   * full panel and a short one only what it can hold. */
  function fit() {
    while (list.children.length > 1 && overflows()) list.removeChild(list.firstChild);
  }

  var ui = {
    setTheme: function (name) {
      root.setAttribute('data-theme', name);
      paintWords();
    },
    render: function (lines, newestId) {
      list.replaceChildren.apply(
        list,
        lines.map(function (line) {
          return draw(line, false);
        })
      );
      root.classList.remove('hidden');
      fit();
      // Marked as new only now: the move it starts must not count as too big.
      var last = list.children[list.children.length - 1];
      if (last && lines.length && lines[lines.length - 1].id === newestId) last.classList.add('enter');
    },
  };

  var controller = core.createController({
    kind: kind,
    ui: ui,
    forcedTheme: forcedTheme,
    texts: function () {
      return texts;
    },
  });

  /* Reconnects with a growing delay, up to ten seconds. */
  function connect() {
    var scheme = location.protocol === 'https:' ? 'wss:' : 'ws:';
    var query = token ? '?token=' + encodeURIComponent(token) : '';
    var ws = new WebSocket(scheme + '//' + location.host + '/ws/overlay/chat/' + kind + query);
    ws.onopen = function () {
      socket = ws;
      attempt = 0;
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
      paintWords();
    })
    .catch(function () {});

  paintWords();
  connect();
})();
