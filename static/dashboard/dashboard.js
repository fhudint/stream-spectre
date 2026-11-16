/* Dashboard page: connects the REST API, the WebSocket and the DOM to the
 * logic in dashboard-core.js.
 *
 * Open it as /dashboard/ (add #token=... when a dashboard token is set; the
 * fragment is never sent to the server and is removed from the address bar).
 * Texts that come from other people (names, titles) are only ever assigned
 * with textContent, never as markup.
 */
(function () {
  'use strict';

  var core = window.DashboardCore;
  var TOKEN_KEY = 'stream-spectre-token';
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
      node.appendChild(child);
    });
    return node;
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

  /* Takes a complete snapshot (the /api/state shape) as the current state. */
  function adopt(data) {
    if (!data || typeof data !== 'object') return;
    state = core.reduce(state, { v: core.PROTOCOL_VERSION, seq: data.seq, type: 'hello', data: data });
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

  // --- drawing ---

  function applyTexts() {
    document.querySelectorAll('[data-i18n]').forEach(function (node) {
      node.textContent = t(node.getAttribute('data-i18n'));
    });
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

  function itemView(item, actions) {
    var info = [
      h('div', { className: 'item-title', text: item.title }),
      h('div', { className: 'meta', text: tf('dashboard.item.requested_by', { name: item.requested_by.name }) }),
      h('div', { className: 'meta', text: tf('dashboard.item.channel', { channel: item.channel }) }),
      h('div', {
        className: 'meta',
        text: tf('dashboard.item.duration', { duration: core.formatDuration(item.duration_secs) }),
      }),
      h('div', { className: 'actions' }, actions),
    ];
    var children = [];
    var thumb = core.thumbnailUrl(item);
    if (thumb) {
      children.push(
        h('img', {
          className: 'thumb',
          attrs: { src: thumb, alt: '', width: '120', height: '68', loading: 'lazy', referrerpolicy: 'no-referrer' },
        })
      );
    }
    children.push(h('div', { className: 'info' }, info));
    return h('div', { className: 'item' }, children);
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
  }

  function renderNow() {
    var item = state.queue.now_playing;
    var box = $('now');
    if (!item) {
      box.replaceChildren(h('p', { className: 'empty', text: t('dashboard.now_playing.none') }));
      return;
    }
    box.replaceChildren(
      itemView(item, [
        button('dashboard.action.skip', 'skip', function () {
          act('POST', '/api/skip');
        }),
      ])
    );
  }

  function renderList(id, list, actionsFor) {
    var box = $(id);
    if (list.length === 0) {
      box.replaceChildren(h('li', { className: 'empty', text: t('dashboard.queue.empty') }));
      return;
    }
    box.replaceChildren.apply(
      box,
      list.map(function (item) {
        return h('li', {}, [itemView(item, actionsFor(item))]);
      })
    );
  }

  function itemPath(item, action) {
    return '/api/queue/' + encodeURIComponent(String(item.id)) + '/' + action;
  }

  function renderQueues() {
    renderList('pending', state.queue.pending, function (item) {
      return [
        button('dashboard.action.approve', 'approve', function () {
          act('POST', itemPath(item, 'approve'));
        }),
        button('dashboard.action.reject', 'reject', function () {
          act('POST', itemPath(item, 'reject'));
        }),
      ];
    });
    renderList('approved', state.queue.approved, function (item) {
      return [
        button('dashboard.action.reject', 'reject', function () {
          act('POST', itemPath(item, 'reject'));
        }),
      ];
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

  /* Fills the forms from the server's settings. Fields being edited are left
   * alone, and nothing is touched when the settings have not changed. */
  function renderSettings() {
    if (!state.settings || !built) return;
    var json = JSON.stringify(state.settings);
    if (json === lastSettings) return;
    lastSettings = json;
    var moderation = state.settings.moderation;
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

  function render() {
    renderStatus();
    renderNow();
    renderQueues();
    renderVolume();
    renderNotices();
    renderSettings();
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
