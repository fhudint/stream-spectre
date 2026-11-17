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
  /* Most chat lines the page keeps, and the most the server may remember. */
  var MAX_CHAT_LINES = 200;
  var MAX_CHAT_HISTORY = 200;
  /* Most requests the page keeps: the same as the server's HISTORY_LIMIT. */
  var MAX_HISTORY = 500;
  var NOTICE_LEVELS = ['info', 'warning', 'error'];
  /* What can have become of a request, as the server names it. */
  var REQUEST_STATUSES = [
    'waiting', 'queued', 'playing', 'played', 'skipped', 'rejected', 'failed', 'refused', 'dropped',
  ];
  /* The IDs the server takes into the blacklist. */
  var YOUTUBE_ID = /^[A-Za-z0-9_-]{11}$/;
  var TIKTOK_ID = /^[0-9]{15,20}$/;
  var TIKTOK_ACCOUNT = /^[A-Za-z0-9_.]{1,64}$/;
  var TIKTOK_SHORT_CODE = /^[A-Za-z0-9]{6,12}$/;
  /* The pages of the dashboard, in the order of the menu. */
  var VIEWS = ['queue', 'requests', 'blacklist', 'rules', 'overlay', 'settings', 'account'];
  /* The ones with an entry in the side menu. The account page is reached from
   * the avatar instead. */
  var MENU_VIEWS = ['queue', 'requests', 'blacklist', 'rules', 'overlay', 'settings'];
  /* The parts of the account page: #account/tiktok and so on. */
  var ACCOUNT_SECTIONS = ['tiktok', 'youtube-chat', 'youtube', 'token'];
  /* The chats that are read, in the order they are shown. */
  var CHAT_SOURCES = ['tiktok', 'youtube'];
  /* The longest YouTube API key the server accepts. */
  var MAX_KEY_LEN = 200;
  /* The server's limits for the chat commands. */
  var MAX_COMMANDS = 8;
  var MAX_PREFIX_LEN = 4;
  var MAX_COMMAND_NAME_LEN = 32;
  var COMMAND_SOURCES = ['youtube', 'tiktok'];
  /* Colours for viewers' avatars and names: see .tint-N in dashboard.css. */
  var TINTS = 6;
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
      // An item from a server that predates TikTok is a YouTube one.
      source: item.source === 'tiktok' ? 'tiktok' : 'youtube',
      video_id: item.video_id,
      title: typeof item.title === 'string' ? item.title : '',
      channel: typeof item.channel === 'string' ? item.channel : '',
      duration_secs: Number.isFinite(item.duration_secs) ? item.duration_secs : 0,
      thumbnail_url: typeof item.thumbnail_url === 'string' ? item.thumbnail_url : '',
      requested_at_ms: wholeNumber(item.requested_at_ms),
      requested_by: {
        name: typeof by.name === 'string' ? by.name : '',
        role: typeof by.role === 'string' ? by.role : 'everyone',
      },
    };
  }

  /* A whole number of at least zero, otherwise 0. */
  function wholeNumber(value) {
    return typeof value === 'number' && isFinite(value) && value > 0 ? Math.floor(value) : 0;
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

  /* A chat line the page can safely show, or null. */
  function cleanLine(line) {
    if (!isObject(line) || typeof line.text !== 'string') return null;
    return {
      name: typeof line.name === 'string' ? line.name : '',
      role: typeof line.role === 'string' ? line.role : 'everyone',
      text: line.text,
      at_ms: wholeNumber(line.at_ms),
      command: line.command === true,
      skipped: Number.isInteger(line.skipped) && line.skipped > 0 ? line.skipped : 0,
    };
  }

  function cleanLines(list) {
    return Array.isArray(list) ? list.map(cleanLine).filter(Boolean).slice(-MAX_CHAT_LINES) : [];
  }

  /* A request from the history the page can safely show, or null. */
  function cleanEntry(entry) {
    if (!isObject(entry) || !Number.isInteger(entry.id) || typeof entry.video_id !== 'string') {
      return null;
    }
    var source = entry.source === 'tiktok' ? 'tiktok' : 'youtube';
    return {
      id: entry.id,
      source: source,
      video_id: entry.video_id,
      title: typeof entry.title === 'string' ? entry.title : '',
      channel: typeof entry.channel === 'string' ? entry.channel : '',
      requested_by: typeof entry.requested_by === 'string' ? entry.requested_by : '',
      requested_at_ms: wholeNumber(entry.requested_at_ms),
      status: REQUEST_STATUSES.indexOf(entry.status) >= 0 ? entry.status : 'refused',
      reason: typeof entry.reason === 'string' ? entry.reason : '',
      // The same picture the queue shows; only YouTube has one.
      thumbnail_url:
        source === 'youtube' && YOUTUBE_ID.test(entry.video_id)
          ? 'https://i.ytimg.com/vi/' + entry.video_id + '/mqdefault.jpg'
          : '',
    };
  }

  function cleanHistory(list) {
    return Array.isArray(list) ? list.map(cleanEntry).filter(Boolean).slice(-MAX_HISTORY) : [];
  }

  /* The history with `entry` in it: it replaces the one with the same id, or
   * goes last. */
  function withEntry(history, entry) {
    var at = history.findIndex(function (old) {
      return old.id === entry.id;
    });
    if (at >= 0) {
      var copy = history.slice();
      copy[at] = entry;
      return copy;
    }
    return history.concat([entry]).slice(-MAX_HISTORY);
  }

  function cleanStats(stats) {
    var s = isObject(stats) ? stats : {};
    return {
      since_ms: wholeNumber(s.since_ms),
      received: wholeNumber(s.received),
      approved: wholeNumber(s.approved),
      rejected: wholeNumber(s.rejected),
    };
  }

  /* The statuses a hello carries, by source. A server from before there were
   * two chats sends only `chat`, which is TikTok's. */
  function cleanChats(data) {
    var chats = { tiktok: null, youtube: null };
    if (isObject(data.chat)) chats.tiktok = data.chat;
    (Array.isArray(data.chats) ? data.chats : []).forEach(function (status) {
      if (isObject(status) && CHAT_SOURCES.indexOf(status.source) >= 0) chats[status.source] = status;
    });
    return chats;
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
      /* The status of each chat, or null before it has reported. `chat` is
       * the TikTok one, as it was before there were two. */
      chat: null,
      chats: { tiktok: null, youtube: null },
      messages: [],
      /* Every request, oldest first. */
      history: [],
      notices: [],
      stats: cleanStats(null),
      /* The video that is playing was paused by a moderator. */
      paused: false,
      /* { item_id, position_secs } of the video that is playing, or null. */
      playback: null,
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
        chat: cleanChats(data).tiktok,
        chats: cleanChats(data),
        messages: cleanLines(data.chat_log),
        history: cleanHistory(data.history),
        stats: cleanStats(data.stats),
        paused: data.playback_paused === true,
        playback: null,
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
        if (typeof data.state === 'string') {
          // An event without a source is from a server that only read TikTok.
          var source = data.source === 'youtube' ? 'youtube' : 'tiktok';
          next.chats = Object.assign({}, state.chats);
          next.chats[source] = data;
          if (source === 'tiktok') next.chat = data;
        }
        break;
      case 'chat_message':
        var line = cleanLine(data);
        if (line) next.messages = state.messages.concat([line]).slice(-MAX_CHAT_LINES);
        break;
      case 'stats_updated':
        next.stats = cleanStats(data);
        break;
      case 'history_entry':
        var entry = cleanEntry(data);
        if (entry) next.history = withEntry(state.history, entry);
        break;
      case 'playback_paused':
        next.paused = data.paused === true;
        break;
      case 'play':
      case 'skip':
      case 'stop':
        // Another video, or an empty screen: it is not paused any more.
        next.paused = false;
        break;
      case 'playback':
        if (Number.isInteger(data.item_id) && Number.isInteger(data.position_secs) && data.position_secs >= 0) {
          next.playback = {
            item_id: data.item_id,
            position_secs: data.position_secs,
            // 0 from a server that does not say; the item's own length counts then.
            duration_secs: Number.isInteger(data.duration_secs) && data.duration_secs > 0 ? data.duration_secs : 0,
          };
        }
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
        break; // queue_updated follows play, skip and stop and carries the queue
    }
    return next;
  }

  /* Whether the server has no length for the video, so a list has none to
   * show. That is so for every TikTok video; once it plays, its player tells. */
  function hasUnknownLength(item) {
    return !!item && item.source === 'tiktok';
  }

  /* How long the video that is playing is, in seconds, or 0 when not known
   * (yet): the length the server has for it, else what the overlay's player
   * told for this very video, which is how a TikTok video gets one. */
  function playbackDuration(state) {
    var playing = state.queue.now_playing;
    if (!playing) return 0;
    if (playing.duration_secs > 0) return playing.duration_secs;
    var playback = state.playback;
    return playback && playback.item_id === playing.id && playback.duration_secs > 0 ? playback.duration_secs : 0;
  }

  /* How far the video that is playing has got, in seconds. Zero until the
   * overlay has reported for this very video and its length is known, and
   * never past that length. */
  function playbackPosition(state) {
    var playing = state.queue.now_playing;
    var playback = state.playback;
    if (!playing || !playback || playback.item_id !== playing.id) return 0;
    var duration = playbackDuration(state);
    return duration > 0 ? Math.min(playback.position_secs, duration) : 0;
  }

  /* Whole seconds a request has waited, or null when its time is not known.
   * `nowMs` is the server's clock, so a moderator's own clock does not matter. */
  function waitingSeconds(item, nowMs) {
    if (!item || !item.requested_at_ms) return null;
    return Math.max(0, Math.floor((nowMs - item.requested_at_ms) / 1000));
  }

  /* How long the oldest of `items` has waited, or null. */
  function oldestWaiting(items, nowMs) {
    var oldest = null;
    items.forEach(function (item) {
      var seconds = waitingSeconds(item, nowMs);
      if (seconds !== null && (oldest === null || seconds > oldest)) oldest = seconds;
    });
    return oldest;
  }

  /* 252 -> "4 min 12 s" with templates from the language file:
   * { seconds, minutes_seconds, hours_minutes }. */
  function formatWaiting(totalSeconds, templates) {
    var seconds = Math.max(0, Math.floor(Number(totalSeconds) || 0));
    if (seconds < 60) return format(templates.seconds, { seconds: seconds });
    if (seconds < 3600) {
      return format(templates.minutes_seconds, { minutes: Math.floor(seconds / 60), seconds: seconds % 60 });
    }
    return format(templates.hours_minutes, {
      hours: Math.floor(seconds / 3600),
      minutes: Math.floor((seconds % 3600) / 60),
    });
  }

  /* A stable colour number (0 to TINTS - 1) for a viewer's name. */
  function tintIndex(name) {
    var hash = 5381;
    var text = String(name);
    for (var i = 0; i < text.length; i += 1) {
      hash = ((hash * 33) ^ text.charCodeAt(i)) >>> 0;
    }
    return hash % TINTS;
  }

  /* The first letter of a name, without a leading @, for the avatar. */
  function initial(name) {
    var letters = Array.from(String(name).replace(/^@+/, '').trim());
    return letters.length ? letters[0].toUpperCase() : '?';
  }

  /* Local time of day as HH:MM:SS, or '' when the time is not known. */
  function formatClock(ms) {
    if (!ms) return '';
    var date = new Date(ms);
    if (isNaN(date.getTime())) return '';
    var pad = function (n) {
      return n < 10 ? '0' + n : String(n);
    };
    return pad(date.getHours()) + ':' + pad(date.getMinutes()) + ':' + pad(date.getSeconds());
  }

  /* When a request came in: the time of day for today, otherwise the date
   * too. '' when the time is not known. `now` is the browser's clock. */
  function formatWhen(ms, now) {
    var clock = formatClock(ms);
    if (!clock) return '';
    var date = new Date(ms);
    if (date.toDateString() === new Date(now).toDateString()) return clock;
    var pad = function (n) {
      return n < 10 ? '0' + n : String(n);
    };
    return date.getFullYear() + '-' + pad(date.getMonth() + 1) + '-' + pad(date.getDate()) + ' ' + clock;
  }

  /* The short list of rules in force, as text. `text(key, values)` gives a
   * translated, filled-in text. */
  function ruleChips(settings, text) {
    if (!settings || !isObject(settings.moderation)) return [];
    var m = settings.moderation;
    var chips = [];
    var words = commandWords(settings.command);
    if (words.length > 0) chips.push(text('dashboard.rules.commands', { list: words.join(' ') }));
    chips.push(
      m.min_role === 'everyone'
        ? text('dashboard.rules.everyone', {})
        : text('dashboard.rules.role', { role: text('role.' + m.min_role, {}) })
    );
    if (m.mode === 'auto') chips.push(text('dashboard.rules.mode_auto', {}));
    if (m.min_gifter_level > 0) chips.push(text('dashboard.rules.gifter', { level: m.min_gifter_level }));
    if (m.min_fan_club_level > 0) chips.push(text('dashboard.rules.fan_club', { level: m.min_fan_club_level }));
    chips.push(
      m.cooldown_secs > 0
        ? text('dashboard.rules.cooldown', { seconds: m.cooldown_secs })
        : text('dashboard.rules.no_cooldown', {})
    );
    chips.push(text('dashboard.rules.max_duration', { duration: formatDuration(m.max_duration_secs) }));
    chips.push(text('dashboard.rules.max_queue', { count: m.max_queue_len }));
    var lists = isObject(settings.blacklist) ? settings.blacklist : {};
    var blocked = BLACKLIST_FIELDS.reduce(function (sum, field) {
      return sum + (Array.isArray(lists[field.key]) ? lists[field.key].length : 0);
    }, 0);
    if (blocked > 0) chips.push(text('dashboard.rules.blocked', { count: blocked }));
    return chips;
  }

  /* The name to blacklist for a video's channel: the YouTube channel, or the
   * TikTok account (a TikTok title is "@account" when the account is known).
   * Works on queue entries and requests alike. */
  function channelName(item) {
    if (item.source === 'tiktok') {
      return item.title.charAt(0) === '@' ? item.title.slice(1) : '';
    }
    return item.channel;
  }

  function isListed(list, value) {
    var wanted = String(value).trim().toLowerCase();
    return (list || []).some(function (entry) {
      return String(entry).trim().toLowerCase() === wanted;
    });
  }

  /* What a moderator can add to the blacklist for this video or request, as
   * { key, value, label, listed }: the video, when its ID is one the server
   * takes, and its channel or account, when known. `label` is a text key. */
  function blacklistChoices(item, blacklist) {
    var lists = isObject(blacklist) ? blacklist : {};
    var choices = [];
    var idPattern = item.source === 'tiktok' ? TIKTOK_ID : YOUTUBE_ID;
    if (idPattern.test(item.video_id)) {
      choices.push({
        key: 'video_ids',
        value: item.video_id,
        label: 'dashboard.action.blacklist_video',
        listed: (lists.video_ids || []).indexOf(item.video_id) >= 0,
      });
    }
    var channel = channelName(item).trim();
    if (channel) {
      choices.push({
        key: 'channels',
        value: channel,
        label:
          item.source === 'tiktok'
            ? 'dashboard.action.blacklist_account'
            : 'dashboard.action.blacklist_channel',
        listed: isListed(lists.channels, channel),
      });
    }
    return choices;
  }

  /* Where to look at a request's video in a browser, to see why it will not
   * play or to check it again, or '' when there is nothing to open: a YouTube
   * watch page, a TikTok video page (with the account when it is known, and
   * any name otherwise: TikTok shows the right video), or a TikTok short link
   * for a request that was turned away before the link was opened. */
  function videoLink(item) {
    if (item.source !== 'tiktok') {
      return YOUTUBE_ID.test(item.video_id) ? 'https://www.youtube.com/watch?v=' + item.video_id : '';
    }
    if (TIKTOK_ID.test(item.video_id)) {
      var account = channelName(item);
      return 'https://www.tiktok.com/@' + (TIKTOK_ACCOUNT.test(account) ? account : '_') + '/video/' + item.video_id;
    }
    return TIKTOK_SHORT_CODE.test(item.video_id) ? 'https://vt.tiktok.com/' + item.video_id + '/' : '';
  }

  /* Which page the address bar asks for; the queue when it asks for nothing
   * known (a `#token=...` fragment, for one). */
  function parseView(hash) {
    var name = String(hash || '').replace(/^#/, '').split('/')[0];
    return VIEWS.indexOf(name) >= 0 ? name : 'queue';
  }

  /* The part of the account page the address asks for ('#account/youtube'),
   * or '' for none or an unknown one. */
  function parseSection(hash) {
    var parts = String(hash || '').replace(/^#/, '').split('/');
    return parts.length === 2 && parts[0] === 'account' && ACCOUNT_SECTIONS.indexOf(parts[1]) >= 0
      ? parts[1]
      : '';
  }

  /* The YouTube API key typed into the form, as the server will take it: no
   * surrounding spaces, printable ASCII only (no spaces inside, no line
   * breaks), not empty, not absurdly long. Returns { value } or
   * { error: 'empty' | 'format' }. The key is never kept after it is sent. */
  function readYoutubeKey(input) {
    var key = String(input).trim();
    if (key === '') return { error: 'empty' };
    if (key.length > MAX_KEY_LEN || !/^[\x21-\x7e]+$/.test(key)) return { error: 'format' };
    return { value: key };
  }

  /* The ID in `ids` that is `delta` places from `current`, kept inside the
   * list; the first when `current` is not in it; null for an empty list. */
  function selectAfter(ids, current, delta) {
    if (ids.length === 0) return null;
    var at = ids.indexOf(current);
    if (at < 0) return ids[0];
    return ids[Math.min(ids.length - 1, Math.max(0, at + delta))];
  }

  /* What a key press asks for on the queue page, or null: up, down,
   * approve, reject, skip or pause. Presses with Ctrl, Alt or Meta are left to the
   * browser. */
  function keyCommand(event) {
    if (!event || event.ctrlKey || event.metaKey || event.altKey) return null;
    switch (event.key) {
      case 'ArrowUp':
        return 'up';
      case 'ArrowDown':
        return 'down';
      case 'a':
      case 'A':
        return 'approve';
      case 'r':
      case 'R':
        return 'reject';
      case 's':
      case 'S':
        return 'skip';
      case 'p':
      case 'P':
        return 'pause';
      default:
        return null;
    }
  }

  /* The overlay's address to give to OBS. A loopback IP is turned into
   * `localhost`: YouTube's player refuses every video on a page opened by an
   * IP address. The token, when there is one, goes in the query. */
  function overlayUrl(protocol, host, token) {
    var shown = String(host).replace(/^(127\.0\.0\.1|\[::1\])(?=:|$)/, 'localhost');
    return protocol + '//' + shown + '/overlay/' + (token ? '?token=' + encodeURIComponent(token) : '');
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

  /* The TikTok username typed into the form, as the server will use it: no
   * spaces, no leading @. Returns { value } (empty for "no username") or
   * { error: true }. The rules are the server's: letters, digits, '_' and
   * '.', at most 64 characters. */
  function readTiktokUsername(input) {
    var typed = String(input).trim();
    var name = typed.replace(/^@+/, '');
    if (name === '') return typed === '' ? { value: '' } : { error: true };
    if (name.length > 64 || !/^[A-Za-z0-9_.]+$/.test(name)) return { error: true };
    return { value: name };
  }

  /* The chat history size typed into the form: a whole number from 0 to the
   * server's limit, or null. Digits only, like the other number fields. */
  function readChatHistory(input) {
    var text = String(input).trim();
    var number = Number(text);
    if (!/^[0-9]+$/.test(text) || !Number.isSafeInteger(number) || number > MAX_CHAT_HISTORY) {
      return null;
    }
    return number;
  }

  /* The chat commands typed into the form, as the server will take them.
   * `raw` is { prefix, commands: [{ name, source }] } with text as typed. A
   * word may be typed with the prefix in front of it ("!vt"); the prefix is
   * dropped. Returns { value: { prefix, commands } } or
   * { error: 'prefix' | 'count' | 'name' | 'source' | 'duplicate', index }
   * (index: the row at fault). The server checks the same rules. */
  function readCommands(raw) {
    var prefix = String(raw.prefix).trim();
    var length = Array.from(prefix).length;
    if (length < 1 || length > MAX_PREFIX_LEN || /[\s\u0000-\u001f\u007f-\u009f]/.test(prefix)) {
      return { error: 'prefix' };
    }
    var rows = Array.isArray(raw.commands) ? raw.commands : [];
    if (rows.length < 1 || rows.length > MAX_COMMANDS) return { error: 'count' };
    var seen = Object.create(null);
    var commands = [];
    for (var i = 0; i < rows.length; i += 1) {
      var name = String(rows[i].name).trim();
      if (name.indexOf(prefix) === 0) name = name.slice(prefix.length);
      if (name.length < 1 || name.length > MAX_COMMAND_NAME_LEN || !/^[A-Za-z0-9_]+$/.test(name)) {
        return { error: 'name', index: i };
      }
      if (COMMAND_SOURCES.indexOf(rows[i].source) < 0) return { error: 'source', index: i };
      var key = name.toLowerCase();
      if (seen[key]) return { error: 'duplicate', index: i };
      seen[key] = true;
      commands.push({ name: name, source: rows[i].source });
    }
    return { value: { prefix: prefix, commands: commands } };
  }

  /* What a viewer types for a command, like "!vt". */
  function commandWords(command) {
    if (!isObject(command) || !Array.isArray(command.commands)) return [];
    return command.commands.map(function (entry) {
      return String(command.prefix) + String(entry.name);
    });
  }

  return {
    PROTOCOL_VERSION: PROTOCOL_VERSION,
    MAX_COMMANDS: MAX_COMMANDS,
    COMMAND_SOURCES: COMMAND_SOURCES,
    MAX_NOTICES: MAX_NOTICES,
    MAX_CHAT_LINES: MAX_CHAT_LINES,
    MAX_CHAT_HISTORY: MAX_CHAT_HISTORY,
    MAX_HISTORY: MAX_HISTORY,
    VIEWS: VIEWS,
    MENU_VIEWS: MENU_VIEWS,
    ACCOUNT_SECTIONS: ACCOUNT_SECTIONS,
    CHAT_SOURCES: CHAT_SOURCES,
    TINTS: TINTS,
    SETTINGS_FIELDS: SETTINGS_FIELDS,
    BLACKLIST_FIELDS: BLACKLIST_FIELDS,
    format: format,
    formatDuration: formatDuration,
    thumbnailUrl: thumbnailUrl,
    playbackPosition: playbackPosition,
    playbackDuration: playbackDuration,
    hasUnknownLength: hasUnknownLength,
    blacklistChoices: blacklistChoices,
    videoLink: videoLink,
    waitingSeconds: waitingSeconds,
    oldestWaiting: oldestWaiting,
    formatWaiting: formatWaiting,
    tintIndex: tintIndex,
    initial: initial,
    formatClock: formatClock,
    formatWhen: formatWhen,
    ruleChips: ruleChips,
    parseView: parseView,
    parseSection: parseSection,
    readYoutubeKey: readYoutubeKey,
    selectAfter: selectAfter,
    keyCommand: keyCommand,
    overlayUrl: overlayUrl,
    initialState: initialState,
    reduce: reduce,
    diffSettings: diffSettings,
    parseLines: parseLines,
    linesToText: linesToText,
    readSettings: readSettings,
    readChatHistory: readChatHistory,
    readTiktokUsername: readTiktokUsername,
    readCommands: readCommands,
    commandWords: commandWords,
  };
});
