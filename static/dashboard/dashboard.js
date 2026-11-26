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
    overview: 'overview-heading',
    queue: 'queue-heading',
    chat: 'chat-heading',
    requests: 'requests-heading',
    blacklist: 'blacklist-heading',
    sources: 'sources-heading',
    rules: 'settings-heading',
    overlay: 'overlay-heading',
    settings: 'chat-settings-heading',
    account: 'account-heading',
  };
  /* Where the keyboard focus goes when #account/<part> is opened. */
  var SECTION_FOCUS = {
    profile: 'profile-name',
    tiktok: 'tiktok-username',
    'youtube-chat': 'youtube-chat-video',
    youtube: 'youtube-key',
    token: 'token-heading',
  };
  /* The cards each tab of the account dialog shows. */
  var ACCOUNT_PANELS = {
    profile: ['account-profile'],
    token: ['account-token'],
  };
  /* What the server says went wrong with a credential, by its code. */
  var ACCOUNT_ERRORS = {
    key_empty: 'dashboard.account.error.key_empty',
    key_bad_format: 'dashboard.account.error.key_bad_format',
    key_rejected: 'dashboard.account.error.key_rejected',
    cannot_check: 'dashboard.account.error.cannot_check',
    not_saved: 'dashboard.account.error.not_saved',
    loopback_only: 'dashboard.account.error.loopback_only',
    bad_request: 'dashboard.account.error.bad_request',
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
  /* The rows of the chat commands form: { name, source } inputs. */
  var commandRefs = [];
  /* The commands form has edits that are not saved; a change from outside
   * must not wipe them. */
  var commandsDirty = false;
  var chatDrawn = null; // the newest chat line already in the page
  var requestsDrawn = null; // the history and blacklist the requests page shows, or null
  var commandsOnly = false;
  var chatSource = 'all'; // the chat page shows one source, or all
  var chatQuery = ''; // lower case; what a line must contain
  var chatPaused = false; // the moderator stopped the page from following the chat
  var chatUnread = 0; // lines added while the page was not following
  var frame = 0; // the animation frame that will draw chat lines that came in
  var minuteCounts = {}; // chat messages seen in each minute, for the chart of the overview
  var minutesSeeded = false;
  var ovNowRefs = null; // the parts of the small "now playing" card that change
  var view = 'overview';
  var lastView = 'overview'; // the page under the account dialog, which it returns to
  var draftAvatar = null; // the picture chosen but not saved; '' means "remove"; null means no change
  var painted = {}; // what each avatar shows now, so a picture is not set again for nothing
  var pane = 'queue'; // queue | notices
  var selectedId = null; // the waiting request the keyboard acts on
  var selectedIndex = 0;
  var clockOffset = 0; // browser clock minus server clock, in milliseconds
  var nowRefs = null; // the parts of the "now playing" card that change
  var waiting = []; // { node, item }: labels that count up every second
  var account = null; // what the server says is set (never the secrets), or null
  var menuOpen = false;
  var insideMenu = false; // the click being handled was on the menu or its button

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

  function markMenu(name) {
    core.MENU_VIEWS.forEach(function (candidate) {
      ['nav-', 'bnav-'].forEach(function (prefix) {
        $(prefix + candidate).setAttribute('aria-current', candidate === name ? 'page' : 'false');
      });
    });
  }

  /* Shows one page and marks it in both menus. The account is a dialog over
   * the page that was open, not a page of its own; it has no menu entry and is
   * reached from the avatar. `section` is the part of it to bring the focus
   * to, if any. */
  function showView(name, focusHeading, section) {
    closeMenu(false);
    if (name === 'account') {
      view = name;
      markMenu(lastView); // the page under the dialog, when the address opened it first
      openAccount(core.accountTab(location.hash), focusHeading, section);
      loadAccount();
      renderRequests();
      return;
    }
    view = name;
    lastView = name;
    closeAccount();
    core.VIEWS.forEach(function (candidate) {
      $('view-' + candidate).classList[candidate === name ? 'remove' : 'add']('hidden');
    });
    markMenu(name);
    renderRequests();
    // Lines that came in while the page was hidden were drawn with no size to
    // measure: show the newest.
    if (name === 'chat') $('chat-log').scrollTop = $('chat-log').scrollHeight;
    if (name === 'sources') loadAccount();
    if (name === 'overview') renderOverview();
    // An old address to a part of what is now this page opens the key's
    // details when it asks for the key.
    if (name === 'sources' && section === 'youtube') $('account-youtube').open = true;
    var target = $((section && SECTION_FOCUS[section]) || HEADINGS[name]);
    if (focusHeading && target && target.focus) target.focus();
  }

  // --- the account dialog ---

  /* Shows one tab of the account dialog and the cards that belong to it. */
  function selectAccountTab(name) {
    core.ACCOUNT_TABS.forEach(function (tab) {
      var selected = tab === name;
      var button = $('atab-' + tab);
      button.setAttribute('aria-selected', selected ? 'true' : 'false');
      button.setAttribute('tabindex', selected ? '0' : '-1');
      ACCOUNT_PANELS[tab].forEach(function (id) {
        $(id).classList[selected ? 'remove' : 'add']('hidden');
      });
    });
  }

  function openAccount(tab, focusTarget, section) {
    var dialog = $('view-account');
    dialog.classList.remove('hidden');
    if (dialog.showModal && !dialog.open) dialog.showModal();
    selectAccountTab(tab);
    resetProfile();
    var target = $((section && SECTION_FOCUS[section]) || HEADINGS.account);
    if (focusTarget && target && target.focus) target.focus();
  }

  /* Closes the dialog without leaving the page: the 'close' event below takes
   * the address back to the page under it. */
  function closeAccount() {
    var dialog = $('view-account');
    dialog.classList.add('hidden');
    if (dialog.close && dialog.open) dialog.close();
  }

  function dismissAccount() {
    var dialog = $('view-account');
    if (dialog.close && dialog.open) dialog.close();
    else if (view === 'account') location.hash = '#' + lastView;
  }

  $('view-account').addEventListener('close', function () {
    $('view-account').classList.add('hidden');
    if (view === 'account') location.hash = '#' + lastView;
  });
  $('account-close').addEventListener('click', dismissAccount);
  // A click on the dimmed page around the dialog (the dialog is the target
  // only there and on its own padding).
  $('view-account').addEventListener('click', function (event) {
    var dialog = $('view-account');
    if (event.target !== dialog || !dialog.getBoundingClientRect) return;
    var box = dialog.getBoundingClientRect();
    var outside =
      event.clientX < box.left || event.clientX > box.right || event.clientY < box.top || event.clientY > box.bottom;
    if (outside) dismissAccount();
  });
  core.ACCOUNT_TABS.forEach(function (tab, index) {
    $('atab-' + tab).addEventListener('click', function () {
      selectAccountTab(tab);
    });
    // The arrow keys move between the tabs, as in any tab list.
    $('atab-' + tab).addEventListener('keydown', function (event) {
      var step = event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0;
      if (!step) return;
      event.preventDefault();
      var next = core.ACCOUNT_TABS[(index + step + core.ACCOUNT_TABS.length) % core.ACCOUNT_TABS.length];
      selectAccountTab(next);
      $('atab-' + next).focus();
    });
  });

  // --- the tabs of the Rules and Overlay pages ---

  /* The tabs of a page: one group of cards shown at a time. `prefix` is the
   * start of the tab buttons' ids, `tabs` their names, `panels` the ids of the
   * cards of each. Arrow keys move between the tabs. Returns the function that
   * chooses one. */
  function pageTabs(prefix, tabs, panels) {
    function select(name) {
      tabs.forEach(function (tab) {
        var on = tab === name;
        var button = $(prefix + tab);
        button.setAttribute('aria-selected', on ? 'true' : 'false');
        button.setAttribute('tabindex', on ? '0' : '-1');
        panels[tab].forEach(function (id) {
          $(id).classList[on ? 'remove' : 'add']('hidden');
        });
      });
    }
    tabs.forEach(function (tab, index) {
      $(prefix + tab).addEventListener('click', function () {
        select(tab);
      });
      $(prefix + tab).addEventListener('keydown', function (event) {
        var step = event.key === 'ArrowRight' ? 1 : event.key === 'ArrowLeft' ? -1 : 0;
        if (!step) return;
        event.preventDefault();
        var next = tabs[(index + step + tabs.length) % tabs.length];
        select(next);
        $(prefix + next).focus();
      });
    });
    select(tabs[0]);
    return select;
  }

  // --- the profile: the name and the picture of the avatar ---

  /* Puts a picture in an avatar, or the first letter of the name when there is
   * none. Only when it differs from what it shows now. */
  function paintAvatar(id, name, picture) {
    var key = name + '\n' + picture;
    if (painted[id] === key) return;
    painted[id] = key;
    var node = $(id);
    if (picture) node.replaceChildren(h('img', { attrs: { src: picture, alt: '' } }));
    else node.textContent = core.initial(name);
  }

  /* The picture the avatar will have: the one chosen, else the saved one. */
  function profilePicture() {
    return draftAvatar !== null ? draftAvatar : core.profileOf(state.settings).avatar;
  }

  function renderProfile() {
    var typed = core.cleanDisplayName($('profile-name').value);
    var picture = profilePicture();
    paintAvatar('profile-preview', typed || t('dashboard.account.title'), picture);
    $('profile-photo-remove').classList[picture ? 'remove' : 'add']('hidden');
  }

  /* Shows what is saved, dropping a picture chosen and not saved. */
  function resetProfile() {
    draftAvatar = null;
    $('profile-name').value = core.profileOf(state.settings).name;
    renderProfile();
  }

  $('profile-name').addEventListener('input', renderProfile);

  /* Reads a photo as the small square picture that is kept: cut from the
   * middle, shrunk to AVATAR_SIZE and saved as JPEG. Resolves with the
   * data: address. (A blob: address would not pass the page's policy.) */
  function readAvatar(file) {
    return new Promise(function (resolve, reject) {
      var reader = new FileReader();
      reader.onerror = reject;
      reader.onload = function () {
        var picture = new Image();
        picture.onerror = reject;
        picture.onload = function () {
          var size = core.AVATAR_SIZE;
          var canvas = document.createElement('canvas');
          canvas.width = size;
          canvas.height = size;
          var context = canvas.getContext('2d');
          if (!context || !picture.naturalWidth || !picture.naturalHeight) {
            reject(new Error('unreadable'));
            return;
          }
          var crop = core.squareCrop(picture.naturalWidth, picture.naturalHeight);
          context.fillStyle = '#ffffff'; // behind a transparent picture
          context.fillRect(0, 0, size, size);
          context.drawImage(picture, crop.x, crop.y, crop.side, crop.side, 0, 0, size, size);
          resolve(canvas.toDataURL('image/jpeg', 0.85));
        };
        picture.src = String(reader.result);
      };
      reader.readAsDataURL(file);
    });
  }

  $('profile-photo').addEventListener('change', function () {
    var input = $('profile-photo');
    var file = input.files && input.files[0];
    if (!file) return;
    readAvatar(file).then(
      function (picture) {
        draftAvatar = picture;
        renderProfile();
      },
      function () {
        showError(t('dashboard.profile.photo_bad'));
      }
    );
    input.value = ''; // so that the same file can be chosen again
  });

  $('profile-photo-remove').addEventListener('click', function () {
    draftAvatar = '';
    renderProfile();
  });

  $('profile-form').addEventListener('submit', function (event) {
    event.preventDefault();
    var name = core.cleanDisplayName($('profile-name').value);
    var body = { display_name: name };
    if (draftAvatar !== null) body.avatar = draftAvatar;
    act('PATCH', '/api/dashboard', body, function (result) {
      adopt(result.data);
      draftAvatar = null;
      $('profile-name').value = name;
      renderProfile();
      flash(t('dashboard.profile.saved'));
    });
  });

  /* On a phone the queue page shows one of two panes at a time, the queue or
   * the notices; on a wide screen both are always shown, side by side. */
  function setPane(name) {
    pane = name;
    $('view-queue').setAttribute('data-pane', name);
    [
      ['tab-queue', name === 'queue'],
      ['tab-notices', name === 'notices'],
    ].forEach(function (entry) {
      $(entry[0]).setAttribute('aria-pressed', entry[1] ? 'true' : 'false');
    });
  }

  window.addEventListener('hashchange', function () {
    showView(core.parseView(location.hash), true, core.parseSection(location.hash));
  });

  [
    ['tab-queue', 'queue'],
    ['tab-notices', 'notices'],
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
    $('chat-search').setAttribute('placeholder', t('dashboard.chat.search'));
    $('chat-search').setAttribute('aria-label', t('dashboard.chat.search'));
    $('chat-sources').setAttribute('aria-label', t('dashboard.chat.sources'));
    $('account-close').setAttribute('aria-label', t('dashboard.account.close'));
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
    var role = t(core.roleKey(item.requested_by.role, item.requested_by.source));
    var seconds = core.waitingSeconds(item, nowMs());
    if (seconds === null) return role;
    return tf('dashboard.row.meta', { role: role, time: core.formatWaiting(seconds, timeTemplates()) });
  }

  /* What the viewer wrote after the link, on a line of its own; nothing when
   * they wrote nothing. */
  function messageLine(message) {
    if (!message) return null;
    return h('div', { className: 'row-message', text: '\u201C' + message + '\u201D', attrs: { title: message } });
  }

  /* One request in a list: picture, title, who asked and for how long. */
  function rowView(item, selected, actions) {
    var picture = thumb(item, 'thumb', 96, 54);
    if (item.duration_secs > 0) {
      picture.children.push(h('span', { className: 'duration', text: core.formatDuration(item.duration_secs) }));
    } else if (core.hasUnknownLength(item)) {
      // No length and no picture: the source's name says what this is.
      picture.children.push(h('span', { className: 'duration', text: item.channel }));
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
          h('div', { className: 'row-title' }, [
            item.song ? h('span', { className: 'song-tag', text: t('dashboard.item.song') }) : null,
            h('span', { text: item.title }),
          ]),
          messageLine(item.message),
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
    // One pill for each chat. A chat with nothing to read (nothing set, or
    // switched off on purpose) has none: the hint below says it once.
    core.CHAT_SOURCES.forEach(function (source) {
      var status = state.chats[source];
      var pill = $(CHAT_PILLS[source]);
      if (status && typeof status.text === 'string' && !isIdle(status)) {
        pill.textContent = status.text;
        pill.className = 'pill ' + (status.state === 'connected' ? 'state-ok' : 'state-bad');
      } else {
        pill.textContent = '';
        pill.className = 'pill';
      }
    });
    // With nothing to read anywhere, say so, with a way to fix it.
    var known = reported();
    var unset = known.length > 0 && known.every(isIdle);
    $('setup-hint').classList[unset ? 'remove' : 'add']('hidden');
    $('setup-hint-text').textContent = unset ? t('dashboard.setup.hint') : '';
    // The avatar is always there: it opens the account menu. It shows the
    // name and picture chosen on the Profile tab (never a chat's channel),
    // and without a name it says "Account".
    var channel = state.chat && typeof state.chat.channel === 'string' ? state.chat.channel : '';
    var profile = core.profileOf(state.settings);
    var shownName = profile.name || t('dashboard.account.title');
    $('channel-name').textContent = shownName;
    paintAvatar('channel-initial', shownName, profile.avatar);
    $('menu-channel').textContent = channel;
    $('menu-channel').classList[channel ? 'remove' : 'add']('hidden');
    $('account-button').setAttribute('aria-label', t('dashboard.account.menu_label') + ': ' + shownName);
    // Disconnect or connect the chat: only when there is a chat to switch.
    var switchable = known.some(function (status) {
      return !isIdle(status);
    });
    $('menu-disconnect').classList[switchable ? 'remove' : 'add']('hidden');
    $('menu-disconnect').textContent = t(
      isPaused() ? 'dashboard.account.menu.connect' : 'dashboard.account.menu.disconnect'
    );
  }

  /* The pill of each chat, by source. */
  var CHAT_PILLS = { tiktok: 'chat-status', youtube: 'chat-status-youtube' };

  /* The chats that have reported a status. */
  function reported() {
    return core.CHAT_SOURCES.map(function (source) {
      return state.chats[source];
    }).filter(Boolean);
  }

  /* Nothing to read: nothing set, or switched off on purpose. */
  function isIdle(status) {
    return status.state === 'not_configured' || status.state === 'disabled';
  }

  /* The chat was disconnected from the account menu. */
  function isPaused() {
    return reported().some(function (status) {
      return status.state === 'paused';
    });
  }

  /* The Account page's chat cards: each source's switch and status line, and
   * the YouTube live. Anything being typed is left alone. */
  function renderChatAccount() {
    var chat = (state.settings && state.settings.chat) || {};
    [
      ['tiktok', 'tiktok-enabled', 'tiktok-status'],
      ['youtube', 'youtube-chat-enabled', 'youtube-chat-status'],
    ].forEach(function (row) {
      var box = $(row[1]);
      if (chat[row[0]] && box !== document.activeElement) box.checked = chat[row[0]].enabled !== false;
      var status = state.chats[row[0]];
      var said = status && typeof status.text === 'string' ? status.text : '';
      // A chat that is being read also says how busy it is.
      if (said && status.state === 'connected') {
        var own = state.messages.filter(function (line) {
          return line.source === row[0];
        });
        said += ' \u00b7 ' + tf('dashboard.chat.rate', { count: core.chatRate(own, nowMs()) });
      }
      $(row[2]).textContent = said;
    });
    var video = $('youtube-chat-video');
    if (chat.youtube && video !== document.activeElement) video.value = String(chat.youtube.video);
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
    ['rules-chips', 'ov-rules'].forEach(function (id) {
      var list = $(id);
      list.replaceChildren.apply(
        list,
        chips.map(function (text) {
          return h('li', { className: 'chip', text: text });
        })
      );
    });
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
    var live = h('span', { className: 'label live' }, [
      h('span', { className: 'dot' }),
      h('span', { text: t('dashboard.now_playing.title') }),
    ]);
    var badge = h('span', { className: 'badge-paused hidden', text: t('dashboard.now.paused') });
    var pause = button('dashboard.action.pause', 'pause', togglePause);
    nowRefs = {
      id: item.id,
      progress: progress,
      elapsed: elapsed,
      remaining: remaining,
      next: next,
      live: live,
      badge: badge,
      pause: pause,
    };
    $('now').replaceChildren(
      h('div', { className: 'now-media' }, [
        h('span', { className: 'label', text: t('dashboard.now.preview') }),
        h('div', { className: picture.className }, picture.children),
      ]),
      h('div', { className: 'now-info' }, [
        h('div', { className: 'live-row' }, [live, badge]),
        h('div', { className: 'now-title', text: item.title }),
        item.message ? h('div', { className: 'now-message', text: '\u201C' + item.message + '\u201D' }) : null,
        h('div', { className: 'row-sub' }, [
          h('span', { text: item.channel }),
          h('span', { className: 'sep', text: ' · ' }),
          h('code', { text: item.video_id }),
          h('span', { className: 'sep', text: ' · ' }),
          h('span', {
            text:
              tf('dashboard.item.requested_by', { name: item.requested_by.name }) +
              ' (' +
              t(core.roleKey(item.requested_by.role, item.requested_by.source)) +
              ')',
          }),
        ]),
        h('div', { className: 'progress' }, [progress, h('div', { className: 'times' }, [elapsed, remaining])]),
        h('div', { className: 'actions' }, [
          pause,
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
    // A TikTok video has a length once its player has told the overlay; until
    // then there is a bar with nothing written under it.
    var position = core.playbackPosition(state);
    var duration = core.playbackDuration(state);
    nowRefs.progress.setAttribute('max', String(Math.max(1, duration)));
    nowRefs.progress.setAttribute('value', String(position));
    if (duration > 0) {
      nowRefs.elapsed.textContent = tf('dashboard.now.elapsed', {
        position: core.formatDuration(position),
        duration: core.formatDuration(duration),
      });
      nowRefs.remaining.textContent = tf('dashboard.now.remaining', {
        time: core.formatWaiting(Math.max(0, duration - position), timeTemplates()),
      });
    } else {
      nowRefs.elapsed.textContent = '';
      nowRefs.remaining.textContent = '';
    }
    // Paused: say so, and the button offers to go on.
    nowRefs.badge.classList[state.paused ? 'remove' : 'add']('hidden');
    nowRefs.live.className = state.paused ? 'label live is-paused' : 'label live';
    nowRefs.pause.textContent = t(state.paused ? 'dashboard.action.resume' : 'dashboard.action.pause');
    nowRefs.pause.setAttribute('aria-pressed', state.paused ? 'true' : 'false');
    var upcoming = state.queue.approved[0];
    nowRefs.next.textContent = upcoming
      ? tf('dashboard.now.next', { title: upcoming.title })
      : t('dashboard.now.next_none');
  }

  /* Pauses the video that is playing, or goes on if it is paused. */
  function togglePause() {
    act('PUT', '/api/playback/pause', { paused: !state.paused }, function (result) {
      adopt(result.data);
    });
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

  /* Adds one entry to the blacklist. With `item`, that request is rejected
   * too: it is being turned down for good. */
  function addToBlacklist(choice, item) {
    var lists = (state.settings && state.settings.blacklist) || {};
    var body = {};
    body[choice.key] = (lists[choice.key] || []).concat([choice.value]);
    act('PATCH', '/api/blacklist', body, function (result) {
      adopt(result.data);
      flash(t('dashboard.blacklist.added'));
      if (item) reject(item);
    });
  }

  /* The small buttons that blacklist a video, or its channel or account. */
  function blacklistButtons(item, rejectToo) {
    var lists = state.settings && state.settings.blacklist;
    return core.blacklistChoices(item, lists).map(function (choice) {
      var node = button(choice.listed ? 'dashboard.blacklist.listed' : choice.label, 'blacklist', function () {
        addToBlacklist(choice, rejectToo ? item : null);
      });
      node.className = 'small';
      node.setAttribute('data-key', choice.key);
      if (choice.listed) node.disabled = true;
      return node;
    });
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
    // The list scrolls by itself on a wide screen, and the arrow keys do not.
    var row = $('pending').children[selectedIndex];
    if (row && row.scrollIntoView) row.scrollIntoView({ block: 'nearest' });
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

  function renderList(id, list, rowFor, emptyKey) {
    var box = $(id);
    if (list.length === 0) {
      box.replaceChildren(h('li', { className: 'empty', text: t(emptyKey) }));
      return;
    }
    box.replaceChildren.apply(box, list.map(rowFor));
  }

  function renderQueues() {
    syncSelection();
    waiting = [];
    renderList('pending', state.queue.pending, function (item) {
      return rowView(
        item,
        item.id === selectedId,
        [
          button('dashboard.action.reject', 'reject', function () {
            reject(item);
          }),
          button('dashboard.action.approve', 'approve', function () {
            approve(item);
          }),
        ].concat(blacklistButtons(item, true))
      );
    }, 'dashboard.pending.empty');
    renderList('approved', state.queue.approved, function (item) {
      return rowView(
        item,
        false,
        [
          button('dashboard.action.reject', 'reject', function () {
            reject(item);
          }),
        ].concat(blacklistButtons(item, true))
      );
    }, 'dashboard.approved.empty');
  }

  /* The small button that copies a link to the video, to open it and see why
   * it does not play. None when there is no link to give. */
  function linkButton(entry) {
    var link = core.videoLink(entry);
    if (!link) return [];
    var node = button('dashboard.action.copy_link', 'copy', function () {
      copyText(link, 'dashboard.requests.link_copied');
    });
    node.className = 'small';
    return [node];
  }

  /* One request in the history: what it was, who sent it, what became of it,
   * and the buttons to blacklist it. */
  function historyRow(entry) {
    var picture = thumb(entry, 'thumb', 96, 54);
    if (entry.source === 'tiktok') {
      picture.children.push(h('span', { className: 'duration', text: t('dashboard.commands.source.tiktok') }));
    }
    var sub = [
      h('span', { className: 'status status-' + entry.status, text: t('dashboard.requests.status.' + entry.status) }),
    ];
    if (entry.channel) sub.push(h('span', { className: 'sep', text: ' · ' }), h('span', { text: entry.channel }));
    sub.push(h('span', { className: 'sep', text: ' · ' }), h('code', { text: entry.video_id }));
    return h('li', { className: 'row' }, [
      h('div', { className: picture.className }, picture.children),
      h('div', { className: 'info' }, [
        h('div', { className: 'row-title', text: entry.title || entry.video_id }),
        messageLine(entry.message),
        h('div', { className: 'row-sub' }, sub),
        entry.reason
          ? h('div', { className: 'row-sub reason', text: entry.reason, attrs: { title: entry.reason } })
          : null,
      ]),
      h('div', { className: 'who' }, [
        h('span', {
          className: 'avatar tint-' + core.tintIndex(entry.requested_by),
          text: core.initial(entry.requested_by),
          attrs: { 'aria-hidden': 'true' },
        }),
        h('div', { className: 'who-text' }, [
          h('span', { className: 'who-name', text: entry.requested_by }),
          h('span', { className: 'who-meta', text: core.formatWhen(entry.requested_at_ms, Date.now()) }),
        ]),
      ]),
      h('div', { className: 'actions' }, linkButton(entry).concat(blacklistButtons(entry, false))),
    ]);
  }

  /* The requests page, newest first. Only redrawn when the history or the
   * blacklist changed: it can hold hundreds of rows. */
  function renderRequests() {
    if (view !== 'requests') return;
    var blacklist = state.settings && state.settings.blacklist;
    if (requestsDrawn && requestsDrawn.history === state.history && requestsDrawn.blacklist === blacklist) return;
    requestsDrawn = { history: state.history, blacklist: blacklist };
    var box = $('requests');
    if (state.history.length === 0) {
      box.replaceChildren(h('li', { className: 'empty', text: t('dashboard.requests.empty') }));
      return;
    }
    box.replaceChildren.apply(box, state.history.slice().reverse().map(historyRow));
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
    if (!core.chatMatches(line, { commandsOnly: commandsOnly, source: chatSource, query: chatQuery })) return false;
    var main = [h('span', { className: 'chat-name name-' + core.tintIndex(line.name), text: line.name })];
    if (line.developer) {
      main.push(h('span', { className: 'chat-tick', attrs: { 'aria-hidden': 'true' } }));
      main.push(h('span', { className: 'chat-role', text: t('overlay.chat.badge.developer') }));
    } else if (line.role !== 'everyone') {
      main.push(h('span', { className: 'chat-role', text: t(core.roleKey(line.role, line.source)) }));
    }
    main.push(h('span', { className: line.command ? 'chat-text chat-cmd' : 'chat-text', text: line.text }));
    box.appendChild(
      h('li', { className: line.command ? 'chat-line chat-command' : line.gift ? 'chat-line chat-gift' : 'chat-line' }, [
        // The chat it came from: an icon in the style sheet, named for screen readers.
        h('span', {
          className: 'chat-source source-' + line.source,
          attrs: { role: 'img', 'aria-label': t('dashboard.chat.source.' + line.source), title: t('dashboard.chat.source.' + line.source) },
        }),
        h('span', { className: 'chat-time', text: core.formatClock(line.at_ms) }),
        h('div', { className: 'chat-main' }, main),
      ])
    );
    return true;
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
    var added = 0;
    for (var i = from; i < lines.length; i += 1) {
      if (appendChatLine(box, lines[i]) === true) added += 1;
    }
    chatDrawn = lines.length ? lines[lines.length - 1] : null;
    while (box.children.length > core.MAX_CHAT_LINES) box.removeChild(box.firstChild);
    $('chat-empty').classList[box.children.length ? 'add' : 'remove']('hidden');
    // Follows the newest line, unless the moderator paused or scrolled up to
    // read: then the new lines are only counted, on the jump button.
    if (!chatPaused && (atBottom || rebuild)) {
      box.scrollTop = box.scrollHeight;
      chatUnread = 0;
    } else if (rebuild) {
      chatUnread = 0;
    } else {
      chatUnread += added;
    }
    renderChatTools();
  }

  /* The pause button, the jump button and the rate beside the filters. */
  /* One more message in the minute it arrived in; minutes an hour old are forgotten. */
  function countMinute(at) {
    var minute = Math.floor((at > 0 ? at : nowMs()) / 60000);
    minuteCounts[minute] = (minuteCounts[minute] || 0) + 1;
    var oldest = Math.floor(nowMs() / 60000) - 60;
    Object.keys(minuteCounts).forEach(function (key) {
      if (Number(key) < oldest) delete minuteCounts[key];
    });
  }

  /* The overview: what the other pages own, in one look. */
  var OVERVIEW_MINUTES = 15;
  function renderOverview() {
    if (view !== 'overview') return;
    $('ov-waiting').textContent = String(state.queue.pending.length);
    $('ov-received').textContent = String(state.stats.received);
    $('ov-approved').textContent = String(state.stats.approved);
    $('ov-rejected').textContent = String(state.stats.rejected);
    $('ov-rate').textContent = String(core.chatRate(state.messages, nowMs()));

    var sources = $('ov-sources');
    sources.replaceChildren.apply(
      sources,
      ['youtube', 'tiktok'].map(function (source) {
        var status = state.chats[source];
        var idle = !status || typeof status.text !== 'string' || isIdle(status);
        var kids = [
          h('span', { className: 'dot ' + (!idle && status.state === 'connected' ? 'ok' : idle ? '' : 'bad') }),
          h('span', { className: 'ov-source-name', text: t('dashboard.chat.source.' + source) }),
          h('span', { className: 'muted', text: idle ? t('dashboard.overview.source_idle') : status.text }),
        ];
        if (idle) kids.push(h('a', { className: 'link', text: t('dashboard.nav.sources'), attrs: { href: '#sources' } }));
        return h('li', {}, kids);
      })
    );

    var bars = core.minuteBars(minuteCounts, nowMs(), OVERVIEW_MINUTES);
    var peak = Math.max.apply(null, bars.concat([1]));
    var chart = $('ov-chart');
    chart.replaceChildren.apply(
      chart,
      bars.map(function (count) {
        return h('i', { className: 'h' + Math.round((count / peak) * 10) });
      })
    );
    chart.setAttribute(
      'aria-label',
      tf('dashboard.overview.chart_label', { peak: Math.max.apply(null, bars), minutes: OVERVIEW_MINUTES })
    );
    renderOverviewNow();
  }

  /* The playing video in miniature; built again only for another video. */
  function renderOverviewNow() {
    var item = state.queue.now_playing;
    if (!item) {
      ovNowRefs = null;
      $('ov-now').replaceChildren(h('p', { className: 'empty', text: t('dashboard.now_playing.none') }));
      return;
    }
    if (!ovNowRefs || ovNowRefs.id !== item.id) {
      var picture = thumb(item, 'preview', 160, 90);
      var progress = h('progress', { attrs: { max: String(Math.max(1, item.duration_secs)), value: '0' } });
      var pause = button('dashboard.action.pause', 'pause', togglePause);
      ovNowRefs = { id: item.id, progress: progress, pause: pause };
      $('ov-now').replaceChildren(
        h('div', { className: 'ov-now' }, [
          h('div', { className: picture.className }, picture.children),
          h('div', { className: 'ov-now-info' }, [
            h('div', { className: 'now-title', text: item.title }),
            h('div', { className: 'row-sub', text: tf('dashboard.item.requested_by', { name: item.requested_by.name }) }),
            progress,
            h('div', { className: 'actions' }, [
              pause,
              button('dashboard.action.skip', 'skip', function () {
                act('POST', '/api/skip');
              }),
            ]),
          ]),
        ])
      );
    }
    ovNowRefs.progress.setAttribute('max', String(Math.max(1, core.playbackDuration(state))));
    ovNowRefs.progress.setAttribute('value', String(core.playbackPosition(state)));
    ovNowRefs.pause.textContent = t(state.paused ? 'dashboard.action.resume' : 'dashboard.action.pause');
    ovNowRefs.pause.setAttribute('aria-pressed', state.paused ? 'true' : 'false');
  }

  function renderChatTools() {
    $('chat-pause').textContent = t(chatPaused ? 'dashboard.chat.resume' : 'dashboard.chat.pause');
    $('chat-pause').setAttribute('aria-pressed', chatPaused ? 'true' : 'false');
    $('chat-jump').textContent = tf('dashboard.chat.jump', { count: chatUnread });
    $('chat-jump').classList[chatUnread > 0 ? 'remove' : 'add']('hidden');
    $('chat-rate').textContent = tf('dashboard.chat.rate', { count: core.chatRate(state.messages, nowMs()) });
  }

  function chatToBottom() {
    var box = $('chat-log');
    box.scrollTop = box.scrollHeight;
    chatUnread = 0;
    renderChatTools();
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
      var node = $('setting-' + field.key);
      // Not while it is being edited, and not what a server from before the
      // setting existed does not send.
      if (node === document.activeElement) return;
      if (moderation[field.key] === undefined && field.kind !== 'optional-number' && !(field.kind === 'select' && field.nullable)) return;
      showSetting(field, moderation[field.key]);
    });
    core.BLACKLIST_FIELDS.forEach(function (field) {
      var input = $('blacklist-' + field.key);
      if (input !== document.activeElement) {
        input.value = core.linesToText(state.settings.blacklist[field.key]);
      }
    });
    renderCommands();
    updateRulesBar();
  }

  /* The chat commands form shows the saved commands, unless the moderator is
   * in the middle of editing them. */
  function renderCommands() {
    var command = state.settings && state.settings.command;
    if (!command || !Array.isArray(command.commands) || command.commands.length === 0) return;
    var words = core.commandWords(command);
    $('commands-example').textContent = tf('dashboard.commands.example', { example: words[0] + ' dQw4w9WgXcQ' });
    if (!commandsDirty) showCommands();
  }

  function showCommands() {
    var command = state.settings && state.settings.command;
    if (!command || !Array.isArray(command.commands)) return;
    $('commands-prefix').value = String(command.prefix);
    drawCommandRows(command.commands);
  }

  function markCommandsDirty() {
    commandsDirty = true;
  }

  /* What is typed in the rows now. */
  function readCommandRows() {
    return commandRefs.map(function (row) {
      return { name: row.name.value, source: row.source.value };
    });
  }

  function drawCommandRows(entries) {
    var list = $('commands-rows');
    list.replaceChildren();
    commandRefs = entries.map(function (entry, index) {
      var name = h('input', {
        attrs: {
          id: 'command-name-' + index,
          type: 'text',
          autocomplete: 'off',
          spellcheck: 'false',
          maxlength: '40',
          'aria-label': t('dashboard.commands.name'),
        },
        on: { input: markCommandsDirty },
      });
      name.value = String(entry.name);
      var source = h(
        'select',
        {
          attrs: { id: 'command-source-' + index, 'aria-label': t('dashboard.commands.source') },
          on: { change: markCommandsDirty },
        },
        core.COMMAND_SOURCES.map(function (id) {
          return h('option', { text: t('dashboard.commands.source.' + id), attrs: { value: id } });
        })
      );
      source.value = String(entry.source);
      var remove = h('button', {
        text: t('dashboard.commands.remove'),
        attrs: { type: 'button', 'data-action': 'remove-command' },
        on: {
          click: function () {
            changeCommandRows(function (rows) {
              return rows.filter(function (row, at) {
                return at !== index;
              });
            });
          },
        },
      });
      // The last word cannot go: there must be a command.
      remove.disabled = entries.length <= 1;
      list.appendChild(h('div', { className: 'command-row' }, [name, source, remove]));
      return { name: name, source: source };
    });
  }

  /* Redraws the rows after `change` turned the typed rows into new ones. */
  function changeCommandRows(change) {
    var rows = change(readCommandRows());
    markCommandsDirty();
    drawCommandRows(rows);
  }

  function saveCommands() {
    var read = core.readCommands({ prefix: $('commands-prefix').value, commands: readCommandRows() });
    if (read.error) {
      var field = {
        prefix: 'dashboard.commands.prefix',
        name: 'dashboard.commands.name',
        source: 'dashboard.commands.source',
      }[read.error];
      showError(
        read.error === 'count'
          ? tf('dashboard.commands.error.count', { max: core.MAX_COMMANDS })
          : read.error === 'duplicate'
            ? t('dashboard.commands.error.duplicate')
            : tf('dashboard.error.invalid_value', { field: t(field) })
      );
      if (read.index !== undefined && commandRefs[read.index]) commandRefs[read.index].name.focus();
      return;
    }
    var current = state.settings && state.settings.command;
    var same =
      current &&
      JSON.stringify({
        prefix: current.prefix,
        commands: current.commands.map(function (entry) {
          return { name: entry.name, source: entry.source };
        }),
      }) === JSON.stringify(read.value);
    if (same) {
      // Typed like "!vt" or with spaces around: show it the way it is kept.
      clearError();
      commandsDirty = false;
      showCommands();
      flash(t('dashboard.commands.saved'));
      return;
    }
    act('PATCH', '/api/commands', read.value, function (result) {
      commandsDirty = false;
      adopt(result.data);
      flash(t('dashboard.commands.saved'));
    });
  }

  /* The address OBS needs, as shown (no token) and as copied (with it). */
  function renderOverlay() {
    var shown = function (overlay) {
      return core.overlayUrl(location.protocol, location.host, '', overlay).replace(/^https?:\/\//, '');
    };
    $('overlay-shown').textContent = shown();
    $('chat-panel-shown').textContent = shown('chat-panel');
    $('chat-float-shown').textContent = shown('chat-float');
    $('song-shown').textContent = shown('song');
    var mediashare = state.settings && state.settings.overlay && state.settings.overlay.mediashare;
    $('developer-notice').checked = !mediashare || mediashare.developer_notice !== false;
    themeInputs.forEach(function (entry) {
      entry.input.checked = entry.name === entry.picker.current(state.settings);
    });
  }

  /* One choice per look of an overlay; the change is saved at once and the
   * overlay in OBS follows without a reload. */
  var themeInputs = [];
  var THEME_PICKERS = [
    { box: 'themes', radio: 'mediashare', path: '/api/overlay/mediashare', names: core.OVERLAY_THEMES, current: core.overlayTheme },
    { box: 'chat-panel-themes', radio: 'chat-panel', path: '/api/overlay/chat/panel', names: core.CHAT_PANEL_THEMES, current: core.chatPanelTheme },
    { box: 'chat-float-themes', radio: 'chat-float', path: '/api/overlay/chat/float', names: core.CHAT_FLOAT_THEMES, current: core.chatFloatTheme },
  ];

  function buildThemePicker(picker) {
    picker.names.forEach(function (name) {
      var input = h('input', {
        attrs: { type: 'radio', name: picker.radio + '-theme', value: name },
        on: {
          change: function () {
            act('PATCH', picker.path, { theme: name }, function (result) {
              adopt(result.data);
            });
          },
        },
      });
      themeInputs.push({ name: name, picker: picker, input: input });
      $(picker.box).appendChild(
        h('label', { className: 'theme', attrs: { 'data-theme': name } }, [
          input,
          h('span', { className: 'swatch', text: 'Aa', attrs: { 'aria-hidden': 'true' } }),
          h('span', { className: 'theme-name', text: t('dashboard.overlay.theme.' + name) }),
        ])
      );
    });
  }

  function buildThemes() {
    THEME_PICKERS.forEach(buildThemePicker);
    renderOverlay();
  }

  function render() {
    renderStatus();
    renderSummary();
    renderCounts();
    renderToday();
    renderRules();
    renderNow();
    renderQueues();
    renderRequests();
    renderVolume();
    renderChat(false);
    renderNotices();
    renderSettings();
    renderChatAccount();
    renderOverlay();
    renderOverview();
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

  /* The control of one rule, with its label (see SETTINGS_FIELDS for the kinds). */
  function settingControl(spec) {
    var id = 'setting-' + spec.key;
    if (spec.kind === 'bool') {
      return h('label', { className: 'toggle' }, [
        h('input', { attrs: { id: id, name: spec.key, type: 'checkbox' } }),
        h('span', { text: t(spec.label) }),
      ]);
    }
    if (spec.kind === 'multi') {
      return h('fieldset', { className: 'field checks-field', attrs: { id: id } }, [
        h('legend', { className: 'field-label', text: t(spec.label) }),
        h(
          'div',
          { className: 'checks' },
          spec.options.map(function (option) {
            return h('label', { className: 'toggle' }, [
              h('input', { attrs: { id: id + '-' + option.value, type: 'checkbox', value: option.value } }),
              h('span', { text: t(option.label) }),
            ]);
          })
        ),
      ]);
    }
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
            attrs: {
              id: id,
              name: spec.key,
              type: 'number',
              min: '0',
              step: spec.unit === 'minutes' ? 'any' : '1',
              inputmode: spec.unit === 'minutes' ? 'decimal' : 'numeric',
            },
          });
    return field(id, spec.label, control);
  }

  /* What the control of a rule holds now, in the shape readSettings takes. */
  function settingInput(spec) {
    var node = $('setting-' + spec.key);
    if (spec.kind === 'bool') return node.checked === true;
    if (spec.kind === 'multi') {
      return spec.options
        .filter(function (option) {
          return $('setting-' + spec.key + '-' + option.value).checked === true;
        })
        .map(function (option) {
          return option.value;
        });
    }
    return spec.unit === 'minutes' ? core.minutesToSeconds(node.value) : node.value;
  }

  /* Shows a rule's saved value in its control. */
  function showSetting(spec, value) {
    var node = $('setting-' + spec.key);
    if (spec.kind === 'bool') {
      node.checked = value === true;
    } else if (spec.kind === 'multi') {
      var chosen = Array.isArray(value) ? value : [];
      spec.options.forEach(function (option) {
        $('setting-' + spec.key + '-' + option.value).checked = chosen.indexOf(option.value) >= 0;
      });
    } else if (spec.unit === 'minutes') {
      node.value = core.secondsToMinutes(value);
    } else {
      node.value = value === null || value === undefined ? '' : String(value);
    }
  }

  /* The controls of a rule: one, or one for each choice of a 'multi' one. */
  function settingNodes(spec) {
    return spec.kind === 'multi'
      ? spec.options.map(function (option) {
          return $('setting-' + spec.key + '-' + option.value);
        })
      : [$('setting-' + spec.key)];
  }

  function readRules() {
    var raw = {};
    core.SETTINGS_FIELDS.forEach(function (spec) {
      raw[spec.key] = settingInput(spec);
    });
    return core.readSettings(raw);
  }

  /* Whether the rules on the page differ from the saved ones. A value that
   * cannot be read counts: it is an edit that has not been saved. */
  function rulesDirty() {
    if (!state.settings || !state.settings.moderation) return false;
    var read = readRules();
    if (read.error) return true;
    return Object.keys(core.diffSettings(state.settings.moderation, read.value)).length > 0;
  }

  /* The save bar is there only while something is unsaved, and the preset the
   * rules match is marked. */
  function updateRulesBar() {
    if (!built) return;
    $('rules-savebar').classList[rulesDirty() ? 'remove' : 'add']('hidden');
    var read = readRules();
    var chosen = read.error ? '' : core.presetOf(read.value);
    Object.keys(core.PRESETS).forEach(function (name) {
      $('preset-' + name).setAttribute('aria-pressed', name === chosen ? 'true' : 'false');
    });
  }

  function applyPreset(name) {
    var values = core.PRESETS[name];
    core.SETTINGS_FIELDS.forEach(function (spec) {
      if (Object.prototype.hasOwnProperty.call(values, spec.key)) showSetting(spec, values[spec.key]);
    });
    updateRulesBar();
    flash(tf('dashboard.rules.preset_applied', { name: t('dashboard.rules.preset.' + name) }));
  }

  function discardRules() {
    lastSettings = null;
    renderSettings();
    updateRulesBar();
  }

  function buildForms() {
    if (built) return;
    built = true;
    buildThemes();

    var settingsForm = $('settings-form');
    // One group of cards per tab; the save bar below saves all of them.
    var rulesPanels = {};
    core.RULES_TABS.forEach(function (tab) {
      rulesPanels[tab] = h('div', { className: 'rules-panel', attrs: { id: 'rules-' + tab } });
      settingsForm.appendChild(rulesPanels[tab]);
      // Presets fill the rules in; nothing is saved until the bar is used.
      if (tab === 'general') {
        rulesPanels[tab].appendChild(
      h('div', { className: 'presets', attrs: { role: 'group', 'aria-label': t('dashboard.rules.presets') } },
        Object.keys(core.PRESETS).map(function (name) {
          return h(
            'button',
            { className: 'preset', attrs: { id: 'preset-' + name, type: 'button', 'aria-pressed': 'false' }, on: { click: function () { applyPreset(name); } } },
            [h('b', { text: t('dashboard.rules.preset.' + name) }), h('span', { text: t('dashboard.rules.preset.' + name + '_hint') })]
          );
        })
      )
        );
      }
      core.RULE_GROUPS.forEach(function (group) {
        var specs = core.SETTINGS_FIELDS.filter(function (spec) {
          return spec.tab === tab && spec.group === group;
        });
        if (specs.length === 0) return;
        rulesPanels[tab].appendChild(
          h('div', { className: 'card page-card' }, [h('h2', { text: t('dashboard.rules.group.' + group) })].concat(specs.map(settingControl)))
        );
      });
    });
    core.SETTINGS_FIELDS.forEach(function (spec) {
      settingNodes(spec).forEach(function (node) {
        node.addEventListener('input', updateRulesBar);
        node.addEventListener('change', updateRulesBar);
      });
    });
    $('rules-discard').addEventListener('click', discardRules);
    settingsForm.addEventListener('submit', function (event) {
      event.preventDefault();
      saveSettings();
    });

    $('commands-prefix').addEventListener('input', markCommandsDirty);
    $('commands-add').addEventListener('click', function () {
      changeCommandRows(function (rows) {
        return rows.concat([{ name: '', source: core.COMMAND_SOURCES[0] }]);
      });
      var last = commandRefs[commandRefs.length - 1];
      if (last) last.name.focus();
    });
    $('commands-form').addEventListener('submit', function (event) {
      event.preventDefault();
      saveCommands();
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

    pageTabs('rtab-', core.RULES_TABS, {
      general: ['rules-general', 'rules-commands'],
      youtube: ['rules-youtube'],
      tiktok: ['rules-tiktok'],
    });
    pageTabs('otab-', core.OVERLAY_TABS, {
      mediashare: ['overlay-mediashare'],
      song: ['overlay-song'],
      'chat-panel': ['overlay-chat-panel'],
      'chat-float': ['overlay-chat-float'],
    });
  }

  function saveSettings() {
    var raw = {};
    core.SETTINGS_FIELDS.forEach(function (spec) {
      raw[spec.key] = settingInput(spec);
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

  // The developer's notice on the mediashare overlay: on unless a streamer says no.
  $('developer-notice').addEventListener('change', function () {
    act('PATCH', '/api/overlay/mediashare', { developer_notice: $('developer-notice').checked }, function (result) {
      adopt(result.data);
    });
  });

  $('chat-commands-only').addEventListener('change', function () {
    commandsOnly = $('chat-commands-only').checked === true;
    renderChat(true);
  });
  $('chat-pause').addEventListener('click', function () {
    chatPaused = !chatPaused;
    if (chatPaused) renderChatTools();
    else chatToBottom();
  });
  $('chat-jump').addEventListener('click', function () {
    chatPaused = false;
    chatToBottom();
  });
  $('chat-log').addEventListener('scroll', function () {
    var box = $('chat-log');
    if (box.scrollHeight - box.scrollTop - box.clientHeight < 24 && chatUnread > 0) {
      chatUnread = 0;
      renderChatTools();
    }
  });
  ['all', 'youtube', 'tiktok'].forEach(function (source) {
    $('chat-src-' + source).addEventListener('click', function () {
      chatSource = source;
      ['all', 'youtube', 'tiktok'].forEach(function (other) {
        $('chat-src-' + other).setAttribute('aria-pressed', other === source ? 'true' : 'false');
      });
      renderChat(true);
    });
  });
  $('chat-search').addEventListener('input', function () {
    chatQuery = $('chat-search').value.trim().toLowerCase();
    renderChat(true);
  });

  /* The side panel of the queue page (this session, the rules) can be folded
   * away for a live; the choice is kept in this browser only. */
  function setRail(collapsed) {
    $('queue-grid').classList[collapsed ? 'add' : 'remove']('rail-collapsed');
    $('rail-toggle').textContent = t(collapsed ? 'dashboard.queue.show_panel' : 'dashboard.queue.hide_panel');
    $('rail-toggle').setAttribute('aria-pressed', collapsed ? 'true' : 'false');
    try {
      localStorage.setItem('spectre.rail', collapsed ? '1' : '0');
    } catch (error) {
      // a private window: the choice just is not kept
    }
  }
  $('rail-toggle').addEventListener('click', function () {
    setRail($('rail-toggle').getAttribute('aria-pressed') !== 'true');
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

  /* Switches the chat of `source` ('tiktok' or 'youtube') on or off. */
  function setChatEnabled(source, enabled) {
    act('PATCH', '/api/chat/' + source, { enabled: enabled }, function (result) {
      adopt(result.data);
      flash(t(enabled ? 'dashboard.chat.switched_on' : 'dashboard.chat.switched_off'));
    });
  }

  $('tiktok-enabled').addEventListener('change', function () {
    setChatEnabled('tiktok', $('tiktok-enabled').checked);
  });
  $('youtube-chat-enabled').addEventListener('change', function () {
    setChatEnabled('youtube', $('youtube-chat-enabled').checked);
  });

  /* The live is sent as typed (a link or an ID); the server checks it and
   * keeps the ID. */
  $('youtube-chat-form').addEventListener('submit', function (event) {
    event.preventDefault();
    act('PATCH', '/api/chat/youtube', { video: $('youtube-chat-video').value.trim() }, function (result) {
      adopt(result.data);
      flash(t('dashboard.youtube_chat.saved'));
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

  // --- a newer version ---

  var updateState = null; // what /api/update said, as core.updateOf reads it

  function renderUpdate() {
    var latest = updateState && updateState.latest;
    $('update-banner').classList[latest ? 'remove' : 'add']('hidden');
    $('update-text').textContent = latest ? tf('dashboard.update.available', { version: latest.version }) : '';
    if (latest) $('update-link').setAttribute('href', latest.url);
    $('update-version').textContent = updateState
      ? tf('dashboard.update.current', { version: updateState.current })
      : '';
    $('update-off').classList[updateState && !updateState.enabled ? 'remove' : 'add']('hidden');
  }

  function loadUpdate() {
    return api('GET', '/api/update').then(function (result) {
      if (!result.ok) return;
      updateState = core.updateOf(result.data) || updateState;
      renderUpdate();
    });
  }

  $('update-check').addEventListener('click', function () {
    var status = $('update-status');
    status.textContent = t('dashboard.update.checking');
    api('POST', '/api/update/check').then(function (result) {
      var read = result.ok ? core.updateOf(result.data) : null;
      if (!read) {
        status.textContent = result.status === 401 ? '' : t('dashboard.update.failed');
        if (result.status === 401) showLogin(false);
        return;
      }
      updateState = read;
      renderUpdate();
      status.textContent = read.latest
        ? tf('dashboard.update.available', { version: read.latest.version })
        : t('dashboard.update.up_to_date');
    });
  });

  // --- the account menu and page ---

  function openMenu() {
    menuOpen = true;
    $('account-menu').classList.remove('hidden');
    $('account-button').setAttribute('aria-expanded', 'true');
  }

  function closeMenu(returnFocus) {
    if (!menuOpen) return;
    menuOpen = false;
    $('account-menu').classList.add('hidden');
    $('account-button').setAttribute('aria-expanded', 'false');
    if (returnFocus) $('account-button').focus();
  }

  $('account-button').addEventListener('click', function () {
    insideMenu = true;
    if (menuOpen) closeMenu(false);
    else openMenu();
  });
  $('account-menu').addEventListener('click', function () {
    // A choice in the menu leads to another page (or part of this one).
    insideMenu = true;
    closeMenu(false);
  });
  /* Disconnects the chat, or connects it again, for as long as the program
   * runs. It is only for this run: after a restart the chat connects by itself. */
  $('menu-disconnect').addEventListener('click', function () {
    var wasPaused = isPaused();
    act('PUT', '/api/chat/connection', { connected: wasPaused }, function (result) {
      adopt(result.data);
      flash(t(wasPaused ? 'dashboard.chat.connecting' : 'dashboard.chat.disconnected'));
    });
  });
  document.addEventListener('click', function () {
    if (!insideMenu) closeMenu(false);
    insideMenu = false;
  });

  function accountFailure(result) {
    if (result.status === 401) {
      showLogin(false);
      return;
    }
    var code = result.data && typeof result.data.code === 'string' ? result.data.code : '';
    if (ACCOUNT_ERRORS[code]) showError(t(ACCOUNT_ERRORS[code]));
    else failure(result);
  }

  function renderAccount() {
    if (!account) return;
    var key = account.youtube_key;
    $('key-status').textContent = key.set
      ? key.last4
        ? tf('dashboard.account.youtube.set', { ending: key.last4 })
        : t('dashboard.account.youtube.set_hidden')
      : t('dashboard.account.youtube.unset');
    $('key-source').textContent =
      key.source === 'saved'
        ? t('dashboard.account.youtube.source.saved')
        : key.source === 'environment'
          ? t('dashboard.account.youtube.source.environment')
          : '';
    var editable = account.editable === true;
    $('key-locked').classList[editable ? 'add' : 'remove']('hidden');
    $('youtube-key').disabled = !editable;
    $('key-save').disabled = !editable;
    $('key-remove').classList[editable && key.source === 'saved' ? 'remove' : 'add']('hidden');
    $('token-status').textContent = account.dashboard_token.set
      ? t('dashboard.account.token.set')
      : t('dashboard.account.token.unset');
  }

  /* Takes what the server says about the account. Never holds a secret: the
   * server sends none. */
  function adoptAccount(data) {
    if (
      !data ||
      typeof data !== 'object' ||
      !data.youtube_key ||
      typeof data.youtube_key !== 'object' ||
      !data.dashboard_token ||
      typeof data.dashboard_token !== 'object'
    ) {
      return;
    }
    account = {
      youtube_key: {
        set: data.youtube_key.set === true,
        source: typeof data.youtube_key.source === 'string' ? data.youtube_key.source : 'none',
        last4: typeof data.youtube_key.last4 === 'string' ? data.youtube_key.last4 : null,
      },
      dashboard_token: { set: data.dashboard_token.set === true },
      editable: data.editable === true,
    };
    renderAccount();
  }

  function loadAccount() {
    return api('GET', '/api/account').then(function (result) {
      if (result.status === 401) showLogin(false);
      else if (result.ok) adoptAccount(result.data);
    });
  }

  $('key-form').addEventListener('submit', function (event) {
    event.preventDefault();
    var read = core.readYoutubeKey($('youtube-key').value);
    if (read.error) {
      showError(t(read.error === 'empty' ? ACCOUNT_ERRORS.key_empty : ACCOUNT_ERRORS.key_bad_format));
      return;
    }
    clearError();
    api('PUT', '/api/account/youtube-key', { key: read.value }).then(function (result) {
      if (!result.ok) {
        accountFailure(result);
        return;
      }
      // Gone from the page the moment it was accepted.
      $('youtube-key').value = '';
      adoptAccount(result.data);
      flash(
        t(
          result.data && result.data.checked === 'quota_exhausted'
            ? 'dashboard.account.youtube.saved_quota'
            : 'dashboard.account.youtube.saved'
        )
      );
    });
  });

  $('key-remove').addEventListener('click', function () {
    clearError();
    api('DELETE', '/api/account/youtube-key').then(function (result) {
      if (!result.ok) {
        accountFailure(result);
        return;
      }
      adoptAccount(result.data);
      flash(t('dashboard.account.youtube.removed'));
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

  /* Copies `text`, and says `copiedKey` when it worked. */
  function copyText(text, copiedKey) {
    var done = function () {
      clearError();
      flash(t(copiedKey));
    };
    var fallback = function () {
      if (copyByHand(text)) done();
      else showError(t('dashboard.overlay.copy_failed'));
    };
    if (typeof navigator !== 'undefined' && navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(done, fallback);
    } else {
      fallback();
    }
  }

  function copyOverlay() {
    copyText(core.overlayUrl(location.protocol, location.host, token), 'dashboard.overlay.copied');
  }
  function copySong() {
    copyText(core.overlayUrl(location.protocol, location.host, token, 'song'), 'dashboard.overlay.copied');
  }
  function copyChatPanel() {
    copyText(core.overlayUrl(location.protocol, location.host, token, 'chat-panel'), 'dashboard.overlay.copied');
  }
  function copyChatFloat() {
    copyText(core.overlayUrl(location.protocol, location.host, token, 'chat-float'), 'dashboard.overlay.copied');
  }

  $('copy-overlay-page').addEventListener('click', copyOverlay);
  $('copy-song').addEventListener('click', copySong);
  $('copy-chat-panel').addEventListener('click', copyChatPanel);
  $('copy-chat-float').addEventListener('click', copyChatFloat);

  // --- keyboard ---

  function typing(target) {
    var tag = target && target.tagName ? String(target.tagName).toLowerCase() : '';
    return tag === 'input' || tag === 'textarea' || tag === 'select' || (target && target.isContentEditable === true);
  }

  document.addEventListener('keydown', function (event) {
    if (menuOpen && event.key === 'Escape') {
      closeMenu(true);
      return;
    }
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
    } else if (command === 'pause' && state.queue.now_playing) {
      event.preventDefault();
      togglePause();
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
      var lastLine = state.messages.length ? state.messages[state.messages.length - 1] : null;
      state = core.reduce(state, message);
      var newLine = state.messages.length ? state.messages[state.messages.length - 1] : null;
      if (message && message.type === 'chat_message' && newLine && newLine !== lastLine) countMinute(newLine.at_ms);
      // A busy chat is drawn once per frame, not once per message.
      if (message && message.type === 'chat_message' && typeof requestAnimationFrame === 'function') {
        if (!frame) {
          frame = requestAnimationFrame(function () {
            frame = 0;
            render();
          });
        }
        return;
      }
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
    $('chat-rate').textContent = tf('dashboard.chat.rate', { count: core.chatRate(state.messages, nowMs()) });
    if (view === 'sources') renderChatAccount();
    if (view === 'overview') renderOverview();
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
        if (!minutesSeeded) {
          minutesSeeded = true;
          state.messages.forEach(function (line) {
            countMinute(line.at_ms);
          });
        }
        applyTexts();
        buildForms();
        showView(core.parseView(location.hash), false, core.parseSection(location.hash));
        setPane(pane);
        var folded = false;
        try {
          folded = localStorage.getItem('spectre.rail') === '1';
        } catch (error) {
          folded = false;
        }
        setRail(folded);
        render();
        loadUpdate();
        if (!started) {
          started = true;
          connect();
        }
      });
    });
  }

  boot(false);
})();
