/* Dashboard page: connects the REST API, the WebSocket and the DOM to the
 * logic in dashboard-core.js.
 *
 * Open it as /dashboard/ (add #token=... when a dashboard token is set; the
 * fragment is never sent to the server and is removed from the address bar).
 * The menu pages are chosen with the address fragment (#queue, #blacklist,
 * #rules, #overlay, #settings).
 * Texts that come from other people (names, titles) are only ever assigned
 * with textContent, never as markup.
 */
(function () {
  'use strict';

  var core = window.DashboardCore;
  var TOKEN_KEY = 'stream-spectre-token';
  /* The heading each page moves the keyboard focus to when it is opened. */
  var HEADINGS = {
    queue: 'queue-heading',
    blacklist: 'blacklist-heading',
    rules: 'settings-heading',
    overlay: 'overlay-heading',
    settings: 'chat-settings-heading',
  };
  /* Shown before the translations can be fetched (sign-in, a server that is down). */
  var FALLBACK = {
    'dashboard.auth.invalid': 'Invalid token',
    'dashboard.error.request_failed': 'Request failed: {message}',
  };

  var state = core.initialState();
  var texts = Object.create(null);
  var token = '';
  var link = 'disconnected'; // disconnected | connected | reconnecting
  var attempt = 0;
  var started = false;
  var built = false;
  var flashTimer = null;
  var lastSettings = '';
  var chatDrawn = null; // the newest chat line already in the page
  var commandsOnly = false;
  var view = 'queue';
  var pane = 'queue'; // queue | chat | notices
  var selectedId = null; // the waiting request the keyboard acts on
  var selectedIndex = 0;
  var clockOffset = 0; // browser clock minus server clock, in milliseconds
  var nowRefs = null; // the parts of the "now playing" card that change
  var waiting = []; // { node, item }: labels that count up every second

  function $(id) {
    return document.getElementById(id);
  }

  function t(key) {
    if (texts[key] !== undefined) return texts[key];
    return FALLBACK[key] !== undefined ? FALLBACK[key] : key;
  }

  function tf(key, values) {
    return core.format(t(key), values);
  }

  /* Builds an element. Text goes in as text, never as markup. */
  function h(tag, options, children) {
    var node = document.createElement(tag);
    var o = options || {};
    if (o.className) node.className = o.className;
    if (o.text !== undefined) node.textContent = o.text;
    Object.keys(o.attrs || {}).forEach(function (name) {
      node.setAttribute(name, o.attrs[name]);
    });
    Object.keys(o.on || {}).forEach(function (name) {
      node.addEventListener(name, o.on[name]);
    });
    (children || []).forEach(function (child) {
      if (child) node.appendChild(child);
    });
    return node;
  }

  function nowMs() {
    return Date.now() - clockOffset;
  }

  function timeTemplates() {
    return {
      seconds: t('dashboard.time.seconds'),
      minutes_seconds: t('dashboard.time.minutes_seconds'),
      hours_minutes: t('dashboard.time.hours_minutes'),
    };
  }

  // --- token ---

  function readStored() {
    try {
      return window.sessionStorage.getItem(TOKEN_KEY) || '';
    } catch (error) {
      return '';
    }
  }

  function store(value) {
    try {
      window.sessionStorage.setItem(TOKEN_KEY, value);
    } catch (error) {
      /* Private mode: the token then lasts until the page is closed. */
    }
  }

  var fromHash = new URLSearchParams(location.hash.replace(/^#/, '')).get('token');
  if (fromHash) {
    token = fromHash;
    store(token);
    history.replaceState(null, '', location.pathname + location.search);
  } else {
    token = readStored();
  }

  // --- talking to the server ---

  function api(method, path, body) {
    var headers = {};
    if (token) headers.Authorization = 'Bearer ' + token;
    var init = { method: method, headers: headers };
    if (body !== undefined) {
      headers['Content-Type'] = 'application/json';
      init.body = JSON.stringify(body);
    }
    return fetch(path, init).then(
      function (response) {
        return response.text().then(function (text) {
          var data = null;
          try {
            data = text ? JSON.parse(text) : null;
          } catch (error) {
            data = null;
          }
          return { ok: response.ok, status: response.status, data: data };
        });
      },
      function () {
        return { ok: false, status: 0, data: null };
      }
    );
  }

  function showError(message) {
    var box = $('error');
    box.textContent = message;
    box.classList.remove('hidden');
  }

  function clearError() {
    $('error').classList.add('hidden');
  }

  function flash(message) {
    var box = $('flash');
    box.textContent = message;
    box.classList.remove('hidden');
    if (flashTimer) clearTimeout(flashTimer);
    flashTimer = setTimeout(function () {
      box.classList.add('hidden');
    }, 3000);
  }

  function failure(result) {
    if (result.status === 401) {
      showLogin(false);
      return;
    }
    var message =
      result.data && typeof result.data.error === 'string'
        ? result.data.error
        : result.status
          ? 'HTTP ' + result.status
          : 'network';
    showError(tf('dashboard.error.request_failed', { message: message }));
  }

  /* Sends an action; `onOk` runs with the response when it succeeded. */
  function act(method, path, body, onOk) {
    clearError();
    return api(method, path, body).then(function (result) {
      if (result.ok) {
        if (onOk) onOk(result);
      } else {
        failure(result);
      }
      return result;
    });
  }

  /* Takes a complete snapshot (the /api/state shape) as the current state.
   * Chat lines the page already has are kept: they are a longer record than
   * the snapshot's, which only holds what the server remembers for a reload.
   * So is the playback position, which a snapshot does not carry. */
  function adopt(data) {
    if (!data || typeof data !== 'object') return;
    var lines = state.messages;
    var playback = state.playback;
    state = core.reduce(state, { v: core.PROTOCOL_VERSION, seq: data.seq, type: 'hello', data: data });
    if (lines.length > 0) state = Object.assign({}, state, { messages: lines });
    if (playback) state = Object.assign({}, state, { playback: playback });
    render();
  }

  // --- sign-in ---

  function showLogin(invalid) {
    $('app').classList.add('hidden');
    $('login').classList.remove('hidden');
    $('login-error').textContent = invalid ? t('dashboard.auth.invalid') : '';
  }

  $('login-form').addEventListener('submit', function (event) {
    event.preventDefault();
    var value = $('token-input').value.trim();
    if (!value) return;
    token = value;
    store(token);
    boot(true);
  });

  // --- pages and panes ---

  /* Shows one page of the menu and marks it in both menus. */
  function showView(name, focusHeading) {
    view = name;
    core.VIEWS.forEach(function (candidate) {
      $('view-' + candidate).classList[candidate === name ? 'remove' : 'add']('hidden');
      ['nav-', 'bnav-'].forEach(function (prefix) {
        $(prefix + candidate).setAttribute('aria-current', candidate === name ? 'page' : 'false');
      });
    });
    var heading = $(HEADINGS[name]);
    if (focusHeading && heading && heading.focus) heading.focus();
  }

  /* On a phone the queue page shows one of three panes at a time; on a wide
   * screen the queue is always shown and the side card shows chat or notices. */
  function setPane(name) {
    pane = name;
    $('view-queue').setAttribute('data-pane', name);
    [
      ['tab-queue', name === 'queue'],
      ['tab-chat', name === 'chat'],
      ['tab-notices', name === 'notices'],
      ['rail-chat', name !== 'notices'],
      ['rail-notices', name === 'notices'],
    ].forEach(function (entry) {
      $(entry[0]).setAttribute('aria-pressed', entry[1] ? 'true' : 'false');
    });
  }

  window.addEventListener('hashchange', function () {
    showView(core.parseView(location.hash), true);
  });

  [
    ['tab-queue', 'queue'],
    ['tab-chat', 'chat'],
    ['tab-notices', 'notices'],
    ['rail-chat', 'chat'],
    ['rail-notices', 'notices'],
  ].forEach(function (entry) {
    $(entry[0]).addEventListener('click', function () {
      setPane(entry[1]);
    });
  });

  // --- drawing ---

  function applyTexts() {
    document.querySelectorAll('[data-i18n]').forEach(function (node) {
      node.textContent = t(node.getAttribute('data-i18n'));
    });
    $('keys').setAttribute('aria-label', t('dashboard.keys.label'));
    document.title = t('dashboard.title');
    if (state.language) document.documentElement.lang = state.language;
  }

  function button(label, kind, onClick) {
    return h('button', {
      className: kind === 'approve' ? 'primary' : kind === 'reject' ? 'danger' : '',
      text: t(label),
      attrs: { type: 'button', 'data-action': kind },
      on: { click: onClick },
    });
  }

  /* A video's picture, or an empty box when it has none we may show. */
  function thumb(item, className, width, height) {
    var url = core.thumbnailUrl(item);
    var children = [];
    if (url) {
      children.push(
        h('img', {
          attrs: {
            src: url,
            alt: '',
            width: String(width),
            height: String(height),
            loading: 'lazy',
            referrerpolicy: 'no-referrer',
          },
        })
      );
    }
    return { url: url, children: children, className: className + (url ? '' : ' empty') };
  }

  function metaText(item) {
    var role = t('role.' + item.requested_by.role);
    var seconds = core.waitingSeconds(item, nowMs());
    if (seconds === null) return role;
    return tf('dashboard.row.meta', { role: role, time: core.formatWaiting(seconds, timeTemplates()) });
  }

  /* One request in a list: picture, title, who asked and for how long. */
  function rowView(item, selected, actions) {
    var picture = thumb(item, 'thumb', 96, 54);
    if (item.duration_secs > 0) {
      picture.children.push(h('span', { className: 'duration', text: core.formatDuration(item.duration_secs) }));
    }
    var label = h('span', { className: 'who-meta', text: metaText(item) });
    waiting.push({ node: label, item: item });
    var tint = core.tintIndex(item.requested_by.name);
    var li = h(
      'li',
      {
        className: selected ? 'row selected' : 'row',
        attrs: selected ? { 'aria-current': 'true' } : {},
        on: {
          click: function () {
            if (state.queue.pending.indexOf(item) >= 0) select(item.id);
          },
        },
      },
      [
        h('div', { className: picture.className }, picture.children),
        h('div', { className: 'info' }, [
          h('div', { className: 'row-title', text: item.title }),
          h('div', { className: 'row-sub' }, [
            h('span', { text: item.channel }),
            h('span', { className: 'sep', text: ' · ' }),
            h('code', { text: item.video_id }),
          ]),
        ]),
        h('div', { className: 'who' }, [
          h('span', {
            className: 'avatar tint-' + tint,
            text: core.initial(item.requested_by.name),
            attrs: { 'aria-hidden': 'true' },
          }),
          h('div', { className: 'who-text' }, [h('span', { className: 'who-name', text: item.requested_by.name }), label]),
        ]),
        h('div', { className: 'actions' }, actions),
      ]
    );
    return li;
  }

  function renderStatus() {
    var connection = $('connection');
    connection.textContent = t('dashboard.connection.' + link);
    connection.className = 'pill ' + (link === 'connected' ? 'state-ok' : 'state-bad');
    var chat = $('chat-status');
    if (state.chat && typeof state.chat.text === 'string') {
      chat.textContent = state.chat.text;
      chat.className = 'pill ' + (state.chat.state === 'connected' ? 'state-ok' : 'state-bad');
    } else {
      chat.textContent = '';
      chat.className = 'pill';
    }
    // Without a username nothing is read: say so, with a way to fix it.
    var unset = state.chat !== null && state.chat.state === 'not_configured';
    $('setup-hint').classList[unset ? 'remove' : 'add']('hidden');
    $('setup-hint-text').textContent = unset && typeof state.chat.text === 'string' ? state.chat.text : '';
    var channel = state.chat && typeof state.chat.channel === 'string' ? state.chat.channel : '';
    $('channel-name').textContent = channel;
    $('channel-initial').textContent = channel ? core.initial(channel) : '';
  }

  function renderSummary() {
    var pending = state.queue.pending;
    var box = $('queue-summary');
    if (pending.length === 0) {
      box.textContent = t('dashboard.queue.summary_none');
      return;
    }
    var oldest = core.oldestWaiting(pending, nowMs());
    box.textContent =
      oldest === null
        ? tf('dashboard.queue.summary', { count: pending.length })
        : tf('dashboard.queue.summary_oldest', {
            count: pending.length,
            time: core.formatWaiting(oldest, timeTemplates()),
          });
  }

  function renderCounts() {
    var pending = String(state.queue.pending.length);
    ['nav-queue-count', 'tab-queue-count', 'queue-count'].forEach(function (id) {
      $(id).textContent = pending;
    });
    $('approved-count').textContent = String(state.queue.approved.length);
  }

  function renderToday() {
    var stats = state.stats;
    $('stat-received').textContent = String(stats.received);
    $('stat-approved').textContent = String(stats.approved);
    $('stat-rejected').textContent = String(stats.rejected);
    $('stat-waiting').textContent = String(state.queue.pending.length);
    var since = core.formatClock(stats.since_ms);
    $('today-since').textContent = since ? tf('dashboard.today.since', { time: since.slice(0, 5) }) : '';
  }

  function renderRules() {
    var chips = core.ruleChips(state.settings, function (key, values) {
      return tf(key, values);
    });
    var list = $('rules-chips');
    list.replaceChildren.apply(
      list,
      chips.map(function (text) {
        return h('li', { className: 'chip', text: text });
      })
    );
  }

  /* The card for the video that is playing. It is built once per video and
   * after that only its progress and the "next" line change, so the picture
   * is not loaded again every second. */
  function buildNow(item) {
    var picture = thumb(item, 'preview', 288, 162);
    var caption = h('span', {
      className: 'caption',
      text: tf('dashboard.now.caption', { name: item.requested_by.name }),
    });
    picture.children.push(caption);
    var progress = h('progress', { attrs: { max: String(Math.max(1, item.duration_secs)), value: '0' } });
    var elapsed = h('span');
    var remaining = h('span');
    var next = h('p', { className: 'dim small' });
    nowRefs = { id: item.id, progress: progress, elapsed: elapsed, remaining: remaining, next: next };
    $('now').replaceChildren(
      h('div', { className: 'now-media' }, [
        h('span', { className: 'label', text: t('dashboard.now.preview') }),
        h('div', { className: picture.className }, picture.children),
      ]),
      h('div', { className: 'now-info' }, [
        h('span', { className: 'label live' }, [
          h('span', { className: 'dot' }),
          h('span', { text: t('dashboard.now_playing.title') }),
        ]),
        h('div', { className: 'now-title', text: item.title }),
        h('div', { className: 'row-sub' }, [
          h('span', { text: item.channel }),
          h('span', { className: 'sep', text: ' · ' }),
          h('code', { text: item.video_id }),
          h('span', { className: 'sep', text: ' · ' }),
          h('span', {
            text:
              tf('dashboard.item.requested_by', { name: item.requested_by.name }) +
              ' (' +
              t('role.' + item.requested_by.role) +
              ')',
          }),
        ]),
        h('div', { className: 'progress' }, [progress, h('div', { className: 'times' }, [elapsed, remaining])]),
        h('div', { className: 'actions' }, [
          button('dashboard.action.skip', 'skip', function () {
            act('POST', '/api/skip');
          }),
        ]),
        next,
      ])
    );
  }

  function renderNow() {
    var item = state.queue.now_playing;
    if (!item) {
      nowRefs = null;
      $('now').replaceChildren(h('p', { className: 'empty', text: t('dashboard.now_playing.none') }));
      return;
    }
    if (!nowRefs || nowRefs.id !== item.id) buildNow(item);
    var position = core.playbackPosition(state);
    nowRefs.progress.setAttribute('value', String(position));
    nowRefs.elapsed.textContent = tf('dashboard.now.elapsed', {
      position: core.formatDuration(position),
      duration: core.formatDuration(item.duration_secs),
    });
    nowRefs.remaining.textContent = tf('dashboard.now.remaining', {
      time: core.formatWaiting(Math.max(0, item.duration_secs - position), timeTemplates()),
    });
    var upcoming = state.queue.approved[0];
    nowRefs.next.textContent = upcoming
      ? tf('dashboard.now.next', { title: upcoming.title })
      : t('dashboard.now.next_none');
  }

  function itemPath(item, action) {
    return '/api/queue/' + encodeURIComponent(String(item.id)) + '/' + action;
  }

  function approve(item) {
    return act('POST', itemPath(item, 'approve'));
  }

  function reject(item) {
    return act('POST', itemPath(item, 'reject'));
  }

  function pendingIds() {
    return state.queue.pending.map(function (item) {
      return item.id;
    });
  }

  function select(id) {
    selectedId = id;
    selectedIndex = Math.max(0, pendingIds().indexOf(id));
    renderQueues();
  }

  /* Keeps the keyboard's choice on a request that still waits: the same one,
   * or else the one that took its place. */
  function syncSelection() {
    var ids = pendingIds();
    var at = ids.indexOf(selectedId);
    if (at >= 0) {
      selectedIndex = at;
    } else {
      selectedId = ids.length ? ids[Math.min(selectedIndex, ids.length - 1)] : null;
    }
  }

  function renderList(id, list, rowFor) {
    var box = $(id);
    if (list.length === 0) {
      box.replaceChildren(h('li', { className: 'empty', text: t('dashboard.queue.empty') }));
      return;
    }
    box.replaceChildren.apply(box, list.map(rowFor));
  }

  function renderQueues() {
    syncSelection();
    waiting = [];
    renderList('pending', state.queue.pending, function (item) {
      return rowView(item, item.id === selectedId, [
        button('dashboard.action.reject', 'reject', function () {
          reject(item);
        }),
        button('dashboard.action.approve', 'approve', function () {
          approve(item);
        }),
      ]);
    });
    renderList('approved', state.queue.approved, function (item) {
      return rowView(item, false, [
        button('dashboard.action.reject', 'reject', function () {
          reject(item);
        }),
      ]);
    });
  }

  function renderVolume() {
    var slider = $('volume');
    if (state.settings && state.settings.moderation) {
      slider.max = String(state.settings.moderation.max_volume);
    }
    slider.value = String(state.volume);
    $('volume-value').textContent = state.volume + '%';
  }

  function renderNotices() {
    var list = $('notices');
    list.replaceChildren.apply(
      list,
      state.notices.map(function (notice) {
        var text = notice.sender ? notice.sender + ': ' + notice.text : notice.text;
        return h('li', { className: 'notice-' + notice.level, text: text });
      })
    );
    $('notices-empty').classList[state.notices.length ? 'add' : 'remove']('hidden');
  }

  function appendChatLine(box, line) {
    if (line.skipped > 0) {
      box.appendChild(
        h('li', { className: 'chat-skipped', text: tf('dashboard.chat.skipped', { count: line.skipped }) })
      );
    }
    if (commandsOnly && !line.command) return;
    var main = [h('span', { className: 'chat-name name-' + core.tintIndex(line.name), text: line.name })];
    if (line.role !== 'everyone') {
      main.push(h('span', { className: 'chat-role', text: t('role.' + line.role) }));
    }
    main.push(h('span', { className: line.command ? 'chat-text chat-cmd' : 'chat-text', text: line.text }));
    box.appendChild(
      h('li', { className: line.command ? 'chat-line chat-command' : 'chat-line' }, [
        h('span', { className: 'chat-time', text: core.formatClock(line.at_ms) }),
        h('div', { className: 'chat-main' }, main),
      ])
    );
  }

  /* Adds the chat lines that arrived since the last drawing. Everything is
   * drawn again only when the lines were replaced (a new snapshot) or the
   * filter changed, so a busy chat stays cheap. The view follows the newest
   * line unless the moderator has scrolled up to read. */
  function renderChat(rebuild) {
    var box = $('chat-log');
    var lines = state.messages;
    var atBottom = box.scrollHeight - box.scrollTop - box.clientHeight < 24;
    var from = 0;
    if (!rebuild && chatDrawn) {
      var at = lines.lastIndexOf(chatDrawn);
      if (at >= 0) {
        from = at + 1;
      } else {
        rebuild = true;
      }
    }
    if (rebuild || !chatDrawn) {
      box.replaceChildren();
      from = 0;
    }
    for (var i = from; i < lines.length; i += 1) appendChatLine(box, lines[i]);
    chatDrawn = lines.length ? lines[lines.length - 1] : null;
    while (box.children.length > core.MAX_CHAT_LINES) box.removeChild(box.firstChild);
    $('chat-empty').classList[box.children.length ? 'add' : 'remove']('hidden');
    if (atBottom) box.scrollTop = box.scrollHeight;
  }

  /* Fills the forms from the server's settings. Fields being edited are left
   * alone, and nothing is touched when the settings have not changed. */
  function renderSettings() {
    if (!state.settings || !built) return;
    var json = JSON.stringify(state.settings);
    if (json === lastSettings) return;
    lastSettings = json;
    var moderation = state.settings.moderation;
    var history = $('chat-history');
    if (state.settings.dashboard && history !== document.activeElement) {
      history.value = String(state.settings.dashboard.chat_history);
    }
    var username = $('tiktok-username');
    if (state.settings.chat && state.settings.chat.tiktok && username !== document.activeElement) {
      username.value = String(state.settings.chat.tiktok.username);
    }
    core.SETTINGS_FIELDS.forEach(function (field) {
      var input = $('setting-' + field.key);
      if (input !== document.activeElement) input.value = String(moderation[field.key]);
    });
    core.BLACKLIST_FIELDS.forEach(function (field) {
      var input = $('blacklist-' + field.key);
      if (input !== document.activeElement) {
        input.value = core.linesToText(state.settings.blacklist[field.key]);
      }
    });
  }

  /* The address OBS needs, as shown (no token) and as copied (with it). */
  function renderOverlay() {
    var shown = core.overlayUrl(location.protocol, location.host, '').replace(/^https?:\/\//, '');
    $('overlay-address').textContent = shown;
    $('overlay-shown').textContent = shown;
  }

  function render() {
    renderStatus();
    renderSummary();
    renderCounts();
    renderToday();
    renderRules();
    renderNow();
    renderQueues();
    renderVolume();
    renderChat(false);
    renderNotices();
    renderSettings();
    renderOverlay();
  }

  // --- forms ---

  function field(id, labelKey, control) {
    return h('div', { className: 'field' }, [
      h('label', { text: t(labelKey), attrs: { for: id } }),
      control,
    ]);
  }

  function submitButton(labelKey) {
    return h('div', { className: 'actions' }, [
      h('button', { className: 'primary', text: t(labelKey), attrs: { type: 'submit' } }),
    ]);
  }

  function buildForms() {
    if (built) return;
    built = true;

    var settingsForm = $('settings-form');
    core.SETTINGS_FIELDS.forEach(function (spec) {
      var id = 'setting-' + spec.key;
      var control =
        spec.kind === 'select'
          ? h(
              'select',
              { attrs: { id: id, name: spec.key } },
              spec.options.map(function (option) {
                return h('option', { text: t(option.label), attrs: { value: option.value } });
              })
            )
          : h('input', {
              attrs: { id: id, name: spec.key, type: 'number', min: '0', step: '1', inputmode: 'numeric' },
            });
      settingsForm.appendChild(field(id, spec.label, control));
    });
    settingsForm.appendChild(submitButton('dashboard.settings.save'));
    settingsForm.addEventListener('submit', function (event) {
      event.preventDefault();
      saveSettings();
    });

    var blacklistForm = $('blacklist-form');
    core.BLACKLIST_FIELDS.forEach(function (spec) {
      var id = 'blacklist-' + spec.key;
      blacklistForm.appendChild(
        field(id, spec.label, h('textarea', { attrs: { id: id, name: spec.key, rows: '4', spellcheck: 'false' } }))
      );
    });
    blacklistForm.appendChild(submitButton('dashboard.blacklist.save'));
    blacklistForm.addEventListener('submit', function (event) {
      event.preventDefault();
      saveBlacklist();
    });
  }

  function saveSettings() {
    var raw = {};
    core.SETTINGS_FIELDS.forEach(function (spec) {
      raw[spec.key] = $('setting-' + spec.key).value;
    });
    var read = core.readSettings(raw);
    if (read.error) {
      var bad = core.SETTINGS_FIELDS.filter(function (spec) {
        return spec.key === read.error;
      })[0];
      showError(tf('dashboard.error.invalid_value', { field: t(bad.label) }));
      return;
    }
    var changes = core.diffSettings(state.settings && state.settings.moderation, read.value);
    if (Object.keys(changes).length === 0) {
      flash(t('dashboard.settings.saved'));
      return;
    }
    act('PATCH', '/api/settings', changes, function (result) {
      adopt(result.data);
      flash(t('dashboard.settings.saved'));
    });
  }

  function saveBlacklist() {
    var body = {};
    core.BLACKLIST_FIELDS.forEach(function (spec) {
      body[spec.key] = core.parseLines($('blacklist-' + spec.key).value);
    });
    act('PATCH', '/api/blacklist', body, function (result) {
      adopt(result.data);
      flash(t('dashboard.blacklist.saved'));
    });
  }

  $('chat-commands-only').addEventListener('change', function () {
    commandsOnly = $('chat-commands-only').checked === true;
    renderChat(true);
  });

  $('tiktok-form').addEventListener('submit', function (event) {
    event.preventDefault();
    var read = core.readTiktokUsername($('tiktok-username').value);
    if (read.error) {
      showError(tf('dashboard.error.invalid_value', { field: t('dashboard.tiktok.username') }));
      return;
    }
    var current = state.settings && state.settings.chat && state.settings.chat.tiktok;
    if (current && current.username === read.value) {
      $('tiktok-username').value = read.value;
      flash(t('dashboard.tiktok.saved'));
      return;
    }
    act('PATCH', '/api/chat/tiktok', { username: read.value }, function (result) {
      adopt(result.data);
      $('tiktok-username').value = read.value;
      flash(t('dashboard.tiktok.saved'));
    });
  });

  $('chat-form').addEventListener('submit', function (event) {
    event.preventDefault();
    var size = core.readChatHistory($('chat-history').value);
    if (size === null) {
      showError(tf('dashboard.error.invalid_value', { field: t('dashboard.chat.history') }));
      return;
    }
    var current = state.settings && state.settings.dashboard;
    if (current && current.chat_history === size) {
      flash(t('dashboard.chat.history_saved'));
      return;
    }
    act('PATCH', '/api/dashboard', { chat_history: size }, function (result) {
      adopt(result.data);
      flash(t('dashboard.chat.history_saved'));
    });
  });

  var volumeSlider = $('volume');
  volumeSlider.addEventListener('input', function () {
    $('volume-value').textContent = volumeSlider.value + '%';
  });
  volumeSlider.addEventListener('change', function () {
    act('POST', '/api/volume', { volume: Number(volumeSlider.value) }, function (result) {
      if (result.data && Number.isFinite(result.data.volume)) {
        state = Object.assign({}, state, { volume: result.data.volume });
        renderVolume();
      }
    });
  });

  // --- copying the overlay address ---

  /* The old way, for pages that are not a secure context (a moderator
   * opening the dashboard by the computer's address on the network). */
  function copyByHand(text) {
    try {
      var area = h('textarea', { className: 'offscreen', attrs: { readonly: 'readonly', 'aria-hidden': 'true' } });
      area.value = text;
      document.body.appendChild(area);
      area.select();
      var copied = document.execCommand('copy') === true;
      document.body.removeChild(area);
      return copied;
    } catch (error) {
      return false;
    }
  }

  function copyOverlay() {
    var url = core.overlayUrl(location.protocol, location.host, token);
    var done = function () {
      clearError();
      flash(t('dashboard.overlay.copied'));
    };
    var fallback = function () {
      if (copyByHand(url)) done();
      else showError(t('dashboard.overlay.copy_failed'));
    };
    if (typeof navigator !== 'undefined' && navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(url).then(done, fallback);
    } else {
      fallback();
    }
  }

  $('copy-overlay').addEventListener('click', copyOverlay);
  $('copy-overlay-page').addEventListener('click', copyOverlay);

  // --- keyboard ---

  function typing(target) {
    var tag = target && target.tagName ? String(target.tagName).toLowerCase() : '';
    return tag === 'input' || tag === 'textarea' || tag === 'select' || (target && target.isContentEditable === true);
  }

  document.addEventListener('keydown', function (event) {
    if (view !== 'queue' || $('app').classList.contains('hidden')) return;
    // Typing, and the chat list, which uses the arrow keys to scroll.
    if (typing(event.target) || event.target === $('chat-log')) return;
    var command = core.keyCommand(event);
    if (!command) return;

    var ids = pendingIds();
    var chosen = state.queue.pending.filter(function (item) {
      return item.id === selectedId;
    })[0];
    if (command === 'up' || command === 'down') {
      if (ids.length === 0) return;
      event.preventDefault();
      select(core.selectAfter(ids, selectedId, command === 'up' ? -1 : 1));
    } else if (command === 'approve' && chosen) {
      event.preventDefault();
      approve(chosen);
    } else if (command === 'reject' && chosen) {
      event.preventDefault();
      reject(chosen);
    } else if (command === 'skip' && state.queue.now_playing) {
      event.preventDefault();
      act('POST', '/api/skip');
    }
  });

  // --- live connection ---

  /* Reconnects with a growing delay, up to ten seconds. A refused connection
   * may mean the token stopped being accepted, so that is checked first. */
  function connect() {
    var scheme = location.protocol === 'https:' ? 'wss:' : 'ws:';
    var query = token ? '?token=' + encodeURIComponent(token) : '';
    var ws = new WebSocket(scheme + '//' + location.host + '/ws/dashboard' + query);
    ws.onopen = function () {
      link = 'connected';
      attempt = 0;
      renderStatus();
    };
    ws.onmessage = function (event) {
      var message;
      try {
        message = JSON.parse(event.data);
      } catch (error) {
        return;
      }
      // Waiting times are measured on the server's clock.
      if (message && typeof message.ts === 'number' && isFinite(message.ts) && message.ts > 0) {
        clockOffset = Date.now() - message.ts;
      }
      state = core.reduce(state, message);
      render();
    };
    ws.onclose = function () {
      link = 'reconnecting';
      renderStatus();
      api('GET', '/api/state').then(function (result) {
        if (result.status === 401) {
          showLogin(false);
          started = false;
          return;
        }
        var delay = Math.min(1000 * Math.pow(2, attempt), 10000);
        attempt += 1;
        setTimeout(connect, delay);
      });
    };
  }

  /* Waiting times count up without any message from the server. */
  setInterval(function () {
    if (!started) return;
    renderSummary();
    waiting.forEach(function (entry) {
      entry.node.textContent = metaText(entry.item);
    });
  }, 1000);

  function boot(fromLogin) {
    return api('GET', '/api/state').then(function (result) {
      if (result.status === 401) {
        showLogin(fromLogin);
        return;
      }
      if (!result.ok) {
        showError(tf('dashboard.error.request_failed', { message: result.status ? 'HTTP ' + result.status : 'network' }));
        $('app').classList.remove('hidden');
        setTimeout(boot, 3000);
        return;
      }
      clearError();
      $('login').classList.add('hidden');
      $('app').classList.remove('hidden');
      return api('GET', '/api/i18n').then(function (i18n) {
        if (i18n.ok && i18n.data && typeof i18n.data === 'object') Object.assign(texts, i18n.data);
        adopt(result.data);
        applyTexts();
        buildForms();
        showView(core.parseView(location.hash), false);
        setPane(pane);
        render();
        if (!started) {
          started = true;
          connect();
        }
      });
    });
  }

  boot(false);
})();
