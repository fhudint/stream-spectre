/* Dashboard logic with no browser dependencies, so it can be tested with Node.
 *
 * It keeps the dashboard's view of the server (queue, settings, volume, chat
 * status, notices) up to date from the events described in
 * docs/websocket-events.md, and holds the small helpers the page needs.
 * dashboard.js connects it to the real page, REST API and WebSocket.
 */
(function (root, factory) {
  'use strict';
  var api = factory();
  if (typeof module === 'object' && module.exports) {
    module.exports = api;
  } else {
    root.DashboardCore = api;
  }
})(typeof self !== 'undefined' ? self : this, function () {
  'use strict';

  var PROTOCOL_VERSION = 1;
  var MAX_NOTICES = 20;
  var NOTICE_LEVELS = ['info', 'warning', 'error'];
  var THUMBNAIL = /^https:\/\/i\.ytimg\.com\/vi\/[A-Za-z0-9_-]{11}\/[A-Za-z0-9_]+\.jpg$/;

  /* The moderation settings the form edits (volume has its own slider). */
  var SETTINGS_FIELDS = [
    {
      key: 'mode',
      kind: 'select',
      label: 'dashboard.settings.mode',
      options: [
        { value: 'manual', label: 'dashboard.settings.mode.manual' },
        { value: 'auto', label: 'dashboard.settings.mode.auto' },
      ],
    },
    {
      key: 'min_role',
      kind: 'select',
      label: 'dashboard.settings.min_role',
      options: [
        { value: 'everyone', label: 'role.everyone' },
        { value: 'follower', label: 'role.follower' },
        { value: 'subscriber', label: 'role.subscriber' },
        { value: 'moderator', label: 'role.moderator' },
      ],
    },
    { key: 'min_gifter_level', kind: 'number', label: 'dashboard.settings.min_gifter_level' },
    { key: 'min_fan_club_level', kind: 'number', label: 'dashboard.settings.min_fan_club_level' },
    { key: 'cooldown_secs', kind: 'number', label: 'dashboard.settings.cooldown_secs' },
    { key: 'max_duration_secs', kind: 'number', label: 'dashboard.settings.max_duration_secs' },
    { key: 'max_queue_len', kind: 'number', label: 'dashboard.settings.max_queue_len' },
    { key: 'max_volume', kind: 'number', label: 'dashboard.settings.max_volume' },
  ];

  /* The three blacklist lists, in the order they are shown. */
  var BLACKLIST_FIELDS = [
    { key: 'video_ids', label: 'dashboard.blacklist.video_ids' },
    { key: 'channels', label: 'dashboard.blacklist.channels' },
    { key: 'title_words', label: 'dashboard.blacklist.title_words' },
  ];

  function isObject(value) {
    return value !== null && typeof value === 'object' && !Array.isArray(value);
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

  /* 185 -> "3:05", 3723 -> "1:02:03". */
  function formatDuration(totalSeconds) {
    var seconds = Math.max(0, Math.floor(Number(totalSeconds) || 0));
    var hours = Math.floor(seconds / 3600);
    var minutes = Math.floor((seconds % 3600) / 60);
    var rest = seconds % 60;
    var pad = function (n) {
      return n < 10 ? '0' + n : String(n);
    };
    return hours > 0 ? hours + ':' + pad(minutes) + ':' + pad(rest) : minutes + ':' + pad(rest);
  }

  /* A queue entry the page can safely show, or null. */
  function cleanItem(item) {
    if (!isObject(item) || !Number.isInteger(item.id) || typeof item.video_id !== 'string') {
      return null;
    }
    var by = isObject(item.requested_by) ? item.requested_by : {};
    return {
      id: item.id,
      video_id: item.video_id,
      title: typeof item.title === 'string' ? item.title : '',
      channel: typeof item.channel === 'string' ? item.channel : '',
      duration_secs: Number.isFinite(item.duration_secs) ? item.duration_secs : 0,
      thumbnail_url: typeof item.thumbnail_url === 'string' ? item.thumbnail_url : '',
      requested_by: {
        name: typeof by.name === 'string' ? by.name : '',
        role: typeof by.role === 'string' ? by.role : 'everyone',
      },
    };
  }

  function cleanList(list) {
    return Array.isArray(list) ? list.map(cleanItem).filter(Boolean) : [];
  }

  function cleanQueue(queue) {
    var q = isObject(queue) ? queue : {};
    return {
      pending: cleanList(q.pending),
      approved: cleanList(q.approved),
      now_playing: cleanItem(q.now_playing),
    };
  }

  /* Thumbnails come from the server; only show YouTube's own image URLs. */
  function thumbnailUrl(item) {
    return item && typeof item.thumbnail_url === 'string' && THUMBNAIL.test(item.thumbnail_url)
      ? item.thumbnail_url
      : null;
  }

  function clampVolume(value, fallback) {
    if (typeof value !== 'number' || !isFinite(value)) return fallback;
    return Math.min(100, Math.max(0, Math.round(value)));
  }

  function initialState() {
    return {
      seq: 0,
      language: null,
      settings: null,
      volume: 50,
      queue: { pending: [], approved: [], now_playing: null },
      chat: null,
      notices: [],
    };
  }

  /* Returns the state after applying one server message; the same object when
   * the message changes nothing. `hello` replaces everything the server owns;
   * other events are applied only if newer than the last one seen. */
  function reduce(state, message) {
    if (!isObject(message) || message.v !== PROTOCOL_VERSION) return state;
    var data = isObject(message.data) ? message.data : {};

    if (message.type === 'hello') {
      return Object.assign({}, state, {
        seq: Number.isInteger(message.seq) ? message.seq : state.seq,
        language: typeof data.language === 'string' ? data.language : state.language,
        settings: isObject(data.settings) ? data.settings : state.settings,
        volume: clampVolume(data.volume, state.volume),
        queue: cleanQueue(data.queue),
        chat: isObject(data.chat) ? data.chat : null,
      });
    }

    if (!Number.isInteger(message.seq) || message.seq <= state.seq) return state;
    var next = Object.assign({}, state, { seq: message.seq });
    switch (message.type) {
      case 'queue_updated':
        next.queue = cleanQueue(data);
        break;
      case 'settings_updated':
        if (isObject(data.moderation) && isObject(data.blacklist)) next.settings = data;
        break;
      case 'volume':
        next.volume = clampVolume(data.volume, state.volume);
        break;
      case 'chat_status':
        next.chat = isObject(data) ? data : state.chat;
        break;
      case 'notice':
        next.notices = [
          {
            level: NOTICE_LEVELS.indexOf(data.level) >= 0 ? data.level : 'info',
            code: typeof data.code === 'string' ? data.code : '',
            text: typeof data.text === 'string' ? data.text : '',
            sender: typeof data.sender === 'string' ? data.sender : null,
          },
        ]
          .concat(state.notices)
          .slice(0, MAX_NOTICES);
        break;
      default:
        break; // play, skip, stop: queue_updated follows and carries the result
    }
    return next;
  }

  /* The fields of `desired` that differ from `current`. */
  function diffSettings(current, desired) {
    var changes = {};
    Object.keys(desired).forEach(function (key) {
      if (!current || current[key] !== desired[key]) changes[key] = desired[key];
    });
    return changes;
  }

  /* One entry per line: trimmed, no blanks, no repeats. */
  function parseLines(text) {
    var seen = Object.create(null);
    var out = [];
    String(text)
      .split(/\r?\n/)
      .forEach(function (line) {
        var entry = line.trim();
        if (entry && !seen[entry]) {
          seen[entry] = true;
          out.push(entry);
        }
      });
    return out;
  }

  function linesToText(list) {
    return Array.isArray(list) ? list.join('\n') : '';
  }

  /* Turns the form's raw values into settings. Returns { value } or
   * { error: <field key> } for the first value that is not acceptable. */
  function readSettings(raw) {
    var value = {};
    for (var i = 0; i < SETTINGS_FIELDS.length; i += 1) {
      var field = SETTINGS_FIELDS[i];
      var input = raw[field.key];
      if (field.kind === 'select') {
        var allowed = field.options.some(function (option) {
          return option.value === input;
        });
        if (!allowed) return { error: field.key };
        value[field.key] = input;
      } else {
        // Digits only: Number() would also accept "1e2" and "0x10".
        var text = String(input).trim();
        var number = Number(text);
        if (!/^[0-9]+$/.test(text) || !Number.isSafeInteger(number)) return { error: field.key };
        value[field.key] = number;
      }
    }
    return { value: value };
  }

  return {
    PROTOCOL_VERSION: PROTOCOL_VERSION,
    MAX_NOTICES: MAX_NOTICES,
    SETTINGS_FIELDS: SETTINGS_FIELDS,
    BLACKLIST_FIELDS: BLACKLIST_FIELDS,
    format: format,
    formatDuration: formatDuration,
    thumbnailUrl: thumbnailUrl,
    initialState: initialState,
    reduce: reduce,
    diffSettings: diffSettings,
    parseLines: parseLines,
    linesToText: linesToText,
    readSettings: readSettings,
  };
});
