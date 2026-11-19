/* Chat overlay logic with no browser dependencies, so it can be tested with
 * Node. It turns server events (see docs/websocket-events.md) into the list
 * of lines to show, and tells a `ui` to draw them. chat.js connects it to the
 * real WebSocket and page.
 */
(function (root, factory) {
  'use strict';
  var api = factory();
  if (typeof module === 'object' && module.exports) {
    module.exports = api;
  } else {
    root.ChatCore = api;
  }
})(typeof self !== 'undefined' ? self : this, function () {
  'use strict';

  /* The looks of the overlay; each is a block of styles in chat.css and a
   * name of ChatPanelTheme or ChatFloatTheme in the server. The panel
   * overlay has a background behind the lines, the float one is transparent. */
  var PANEL_THEMES = ['obsidian', 'paper', 'brutal', 'terminal', 'candy', 'pixel', 'receipt', 'messages', 'ticker'];
  var FLOAT_THEMES = ['outline', 'comic', 'notes'];
  var THEMES = PANEL_THEMES.concat(FLOAT_THEMES);
  /* How many lines a look keeps on screen: what fits in a source of the
   * usual size (360 x 640), or what a bar can hold. */
  var LIMITS = {
    obsidian: 7,
    paper: 7,
    brutal: 6,
    terminal: 7,
    candy: 5,
    pixel: 6,
    comic: 5,
    outline: 7,
    receipt: 6,
    messages: 6,
    ticker: 4,
    notes: 5,
  };
  /* Colours for viewers' names: see .tint-N in chat.css. */
  var TINTS = 7;
  /* The most lines the page ever holds, whatever the look. */
  var MAX_LINES = 12;
  /* Longest name or text drawn; the server's own limits are looser. */
  var MAX_NAME = 40;
  var MAX_TEXT = 300;
  var ROTATIONS = [-1.2, 1, -0.8, 1.4, -1.5, 0.9, -1];

  /* The looks of one overlay: 'panel' (with a background) or 'float'
   * (transparent). Anything else is the panel's. */
  function themesOf(kind) {
    return kind === 'float' ? FLOAT_THEMES : PANEL_THEMES;
  }

  /* Whether `value` is a look of the overlay `kind`; of either when none is given. */
  function isTheme(value, kind) {
    return typeof value === 'string' && (kind ? themesOf(kind) : THEMES).indexOf(value) >= 0;
  }

  function limitOf(theme) {
    return isTheme(theme) ? LIMITS[theme] : LIMITS[THEMES[0]];
  }

  /* The same name always gets the same colour. */
  function tintIndex(name) {
    var hash = 0;
    for (var i = 0; i < name.length; i += 1) hash = (hash * 31 + name.charCodeAt(i)) >>> 0;
    return hash % TINTS;
  }

  function initial(name) {
    var letters = Array.from(String(name).replace(/^@+/, '').trim());
    return letters.length ? letters[0].toUpperCase() : '?';
  }

  function shorten(text, max) {
    var letters = Array.from(String(text));
    return letters.length > max ? letters.slice(0, max - 1).join('') + '…' : letters.join('');
  }

  function pad(n) {
    return n < 10 ? '0' + n : String(n);
  }

  /* Local time of day as HH:MM:SS, or '' when the time is not known. */
  function clock(ms) {
    if (typeof ms !== 'number' || !ms) return '';
    var date = new Date(ms);
    if (isNaN(date.getTime())) return '';
    return pad(date.getHours()) + ':' + pad(date.getMinutes()) + ':' + pad(date.getSeconds());
  }

  /* The word beside a name for a viewer who pays or moderates; '' for the
   * rest. `texts` has the translated words (the overlay works without them). */
  function badgeOf(role, source, texts) {
    var say = function (key, fallback) {
      return (texts && texts[key]) || fallback;
    };
    if (role === 'moderator') return say('overlay.chat.badge.moderator', 'Mod');
    if (role === 'subscriber') {
      return source === 'youtube'
        ? say('overlay.chat.badge.member', 'Member')
        : say('overlay.chat.badge.subscriber', 'Sub');
    }
    return '';
  }

  /* One line of the chat as the page draws it, or null for something that is
   * not a line (an event with no name or no text). Names and texts are only
   * ever put on the page as text. */
  function lineOf(data, serial, texts) {
    if (!data || typeof data !== 'object') return null;
    var name = typeof data.name === 'string' ? data.name.replace(/^@+/, '').trim() : '';
    var text = typeof data.text === 'string' ? data.text.trim() : '';
    if (!name || !text) return null;
    var source = data.source === 'youtube' ? 'youtube' : 'tiktok';
    var command = data.command === true;
    var tint = tintIndex(name);
    return {
      id: serial,
      source: source,
      who: '@' + shorten(name, MAX_NAME),
      initial: initial(name),
      tint: tint,
      rotation: ROTATIONS[serial % ROTATIONS.length],
      badge: badgeOf(data.role, source, texts),
      text: shorten(text, MAX_TEXT),
      command: command,
      // A gift sent in the live: a line like the others, in the colour of the look.
      gift: data.gift === true,
      status: command ? (texts && texts['overlay.chat.command']) || 'Video request' : '',
      time: clock(data.at_ms),
    };
  }

  /* `ui` has setTheme(name) and render(lines, newestId). `texts()` returns
   * the translated words. `kind` is 'panel' or 'float': which overlay this is,
   * so which looks it has and which of the server's events are its own. */
  function createController(deps) {
    var ui = deps.ui;
    var kind = deps.kind === 'float' ? 'float' : 'panel';
    var theme = themesOf(kind)[0];
    var lines = [];
    var serial = 0;
    var snapshotSeq = 0;

    function keep() {
      var most = Math.min(limitOf(theme), MAX_LINES);
      if (lines.length > most) lines = lines.slice(lines.length - most);
    }

    function add(data) {
      serial += 1;
      var line = lineOf(data, serial, deps.texts ? deps.texts() : {});
      if (!line) return null;
      lines.push(line);
      keep();
      return line;
    }

    // ?theme=name shows that look whatever the dashboard chose, for one source.
    var forced = isTheme(deps.forcedTheme, kind) ? deps.forcedTheme : '';

    /* Whether the look was taken: one this overlay does not have is not. */
    function setTheme(name) {
      if (!forced && !isTheme(name, kind)) return false;
      theme = forced || name;
      ui.setTheme(theme);
      keep();
      return true;
    }

    function handle(message) {
      if (!message || typeof message !== 'object' || message.v !== 1) return;
      var data = message.data;
      switch (message.type) {
        case 'hello': {
          if (!data || typeof data !== 'object') return;
          snapshotSeq = typeof message.seq === 'number' ? message.seq : 0;
          lines = [];
          var chat = data.settings && data.settings.overlay && data.settings.overlay['chat_' + kind];
          setTheme(chat && chat.theme);
          (Array.isArray(data.chat_log) ? data.chat_log : []).forEach(add);
          ui.render(lines, 0);
          break;
        }
        case 'chat_message': {
          if (typeof message.seq === 'number' && message.seq <= snapshotSeq) return;
          var line = add(data);
          if (line) ui.render(lines, line.id);
          break;
        }
        case 'overlay_theme':
          if (data && data.overlay === 'chat_' + kind && setTheme(data.theme)) ui.render(lines, 0);
          break;
        default:
          break;
      }
    }

    return {
      handle: handle,
      state: function () {
        return { theme: theme, lines: lines.slice() };
      },
    };
  }

  return {
    THEMES: THEMES,
    PANEL_THEMES: PANEL_THEMES,
    FLOAT_THEMES: FLOAT_THEMES,
    themesOf: themesOf,
    LIMITS: LIMITS,
    TINTS: TINTS,
    MAX_LINES: MAX_LINES,
    isTheme: isTheme,
    limitOf: limitOf,
    tintIndex: tintIndex,
    initial: initial,
    shorten: shorten,
    clock: clock,
    badgeOf: badgeOf,
    lineOf: lineOf,
    createController: createController,
  };
});
