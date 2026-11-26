'use strict';

// Runs static/overlay/song/song.js against hand-made stand-ins for the browser
// and the YouTube player.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const root = path.join(__dirname, '../..');
const core = require(path.join(root, 'static/overlay/mediashare/overlay-core.js'));
const source = fs.readFileSync(path.join(root, 'static/overlay/song/song.js'), 'utf8');
const html = fs.readFileSync(path.join(root, 'static/overlay/song/index.html'), 'utf8');
const css = fs.readFileSync(path.join(root, 'static/overlay/song/song.css'), 'utf8');

const ID = 'dQw4w9WgXcQ';

class FakeElement {
  constructor(id, classes = []) {
    this.id = id;
    this.classes = new Set(classes);
    this._text = '';
    this.style = {};
    this.attributes = {};
    this.classList = {
      add: (name) => this.classes.add(name),
      remove: (name) => this.classes.delete(name),
      contains: (name) => this.classes.has(name),
    };
  }
  get textContent() {
    return this._text;
  }
  set textContent(value) {
    this._text = String(value);
  }
  setAttribute(name, value) {
    this.attributes[name] = String(value);
  }
  set innerHTML(_value) {
    throw new Error('innerHTML must never be assigned');
  }
}

function load({ search = '', fetchBody = {} } = {}) {
  const log = { sockets: [], timeouts: [], intervals: [], fetches: [], scripts: [], players: [], position: 0, duration: 200 };

  class FakeWebSocket {
    static OPEN = 1;
    constructor(url) {
      this.url = url;
      this.readyState = 0;
      this.sent = [];
      log.sockets.push(this);
    }
    send(data) {
      this.sent.push(JSON.parse(data));
    }
    open() {
      this.readyState = 1;
      this.onopen();
    }
    receive(message) {
      this.onmessage({ data: JSON.stringify(message) });
    }
  }

  class FakePlayer {
    constructor(id, options) {
      this.id = id;
      this.options = options;
      this.calls = [];
      log.players.push(this);
    }
    setVolume(volume) {
      this.calls.push(['setVolume', volume]);
    }
    getCurrentTime() {
      return log.position;
    }
    getDuration() {
      return log.duration;
    }
    loadVideoById(id) {
      this.calls.push(['loadVideoById', id]);
    }
    stopVideo() {
      this.calls.push(['stopVideo']);
    }
    pauseVideo() {
      this.calls.push(['pauseVideo']);
    }
    playVideo() {
      this.calls.push(['playVideo']);
    }
  }

  const elements = { root: new FakeElement('root', ['hidden']), 'bar-fill': new FakeElement('bar-fill') };
  for (const id of ['kicker', 'title', 'channel', 'requested', 'message', 'time', 'left', 'paused']) elements[id] = new FakeElement(id);

  const sandbox = {
    OverlayCore: core,
    URLSearchParams,
    JSON,
    Math,
    Object,
    String,
    Promise,
    encodeURIComponent,
    location: { search, protocol: 'http:', host: 'localhost:8787', origin: 'http://localhost:8787' },
    WebSocket: FakeWebSocket,
    document: {
      getElementById: (id) => elements[id],
      createElement: (tag) => new FakeElement(tag),
      head: { appendChild: (el) => log.scripts.push(el) },
    },
    fetch: (url, options) => {
      log.fetches.push({ url, options });
      return Promise.resolve({ ok: true, json: () => Promise.resolve(fetchBody) });
    },
    setTimeout: (fn, delay) => {
      log.timeouts.push({ fn, delay });
      return log.timeouts.length;
    },
    setInterval: (fn, delay) => {
      log.intervals.push({ fn, delay });
      return log.intervals.length;
    },
    YT: { Player: FakePlayer, PlayerState: { ENDED: 0, PLAYING: 1 } },
  };
  sandbox.window = sandbox;
  vm.runInNewContext(source, sandbox);

  return {
    log,
    elements,
    sandbox,
    socket: () => log.sockets.at(-1),
    ready() {
      sandbox.onYouTubeIframeAPIReady();
      log.players.at(-1).options.events.onReady();
      return log.players.at(-1);
    },
  };
}

const envelope = (seq, type, data) => ({ v: 1, seq, ts: 0, type, data });
const song = (id = 1, extra = {}) => ({
  id,
  source: 'youtube',
  video_id: ID,
  title: 'A <b>song</b>',
  channel: 'Some Artist',
  message: '',
  song: true,
  requested_by: { name: 'Budi', role: 'follower', source: 'tiktok' },
  ...extra,
});
const flush = () => new Promise((resolve) => setImmediate(resolve));

test('it connects to the song socket, with the token encoded in the query', () => {
  const page = load({ search: '?token=a b' });
  assert.equal(page.socket().url, 'ws://localhost:8787/ws/overlay/song?token=a%20b');
  assert.equal(load().socket().url, 'ws://localhost:8787/ws/overlay/song');
});

test('a song starts in the player and shows its title, artist, who asked and the message, as plain text', () => {
  const page = load();
  const player = page.ready();
  page.socket().open();
  page.socket().receive(envelope(1, 'play', { item: song(1, { message: 'for <i>you</i>' }), volume: 40 }));

  assert.deepEqual(player.calls.slice(-2), [['setVolume', 40], ['loadVideoById', ID]]);
  assert.equal(page.elements.title.textContent, 'A <b>song</b>', 'the song title is the main line');
  assert.equal(page.elements.channel.textContent, 'Some Artist');
  assert.equal(page.elements.requested.textContent, 'Requested by Budi');
  assert.equal(page.elements.message.textContent, '“for <i>you</i>”');
  assert.equal(page.elements.kicker.textContent, 'Song request');
  assert.ok(!page.elements.root.classes.has('hidden'));
});

test('a song that is over is reported, and stop clears the card', () => {
  const page = load();
  const player = page.ready();
  page.socket().open();
  page.socket().receive(envelope(1, 'play', { item: song(7), volume: 50 }));
  player.options.events.onStateChange({ data: 0 });
  assert.deepEqual(page.socket().sent.at(-1), { type: 'ended', item_id: 7 });

  page.socket().receive(envelope(2, 'stop', {}));
  assert.ok(page.elements.root.classes.has('hidden'));
  assert.equal(page.elements.title.textContent, '');
  assert.equal(player.calls.at(-1)[0], 'stopVideo');
});

test('a pause shows its notice and stops the song, and resuming plays it again', () => {
  const page = load();
  const player = page.ready();
  page.socket().open();
  page.socket().receive(envelope(1, 'play', { item: song(7), volume: 50 }));
  page.socket().receive(envelope(2, 'playback_paused', { item_id: 7, paused: true }));
  assert.equal(player.calls.at(-1)[0], 'pauseVideo');
  assert.equal(page.elements.paused.textContent, 'Paused');
  assert.ok(page.elements.root.classes.has('paused'));
  page.socket().receive(envelope(3, 'playback_paused', { item_id: 7, paused: false }));
  assert.equal(player.calls.at(-1)[0], 'playVideo');
  assert.equal(page.elements.paused.textContent, '');
});

test('a player that cannot play the song is reported as an error', () => {
  const page = load();
  const player = page.ready();
  page.socket().open();
  page.socket().receive(envelope(1, 'play', { item: song(5), volume: 50 }));
  player.options.events.onError({ data: 150 });
  assert.deepEqual(page.socket().sent.at(-1), { type: 'player_error', item_id: 5, code: '150' });
});

test('the player is created visible, with the whole cover to itself, and the page is a clean one', () => {
  const page = load();
  const player = page.ready();
  assert.equal(player.options.width, '100%');
  assert.equal(player.options.height, '100%');
  assert.equal(player.options.playerVars.controls, 0);
  // YouTube wants the player visible and at least 200 x 200.
  assert.match(css, /--cover:\s*max\(200px,/);
  assert.match(css, /#cover\s*\{[^}]*width:\s*var\(--cover\)/);
  assert.ok(!/<script(?![^>]*\bsrc=)[^>]*>/.test(html) && !/\sstyle="/.test(html), 'no inline script or style');
  assert.ok(html.includes('../mediashare/overlay-core.js') && html.includes('song.js'));
  assert.ok(!/innerHTML|outerHTML|insertAdjacentHTML|document\.write/.test(source));
});

test('the song look comes from its own settings and events, and ?theme= wins for one source', () => {
  const page = load();
  page.socket().open();
  page.socket().receive(envelope(1, 'hello', { volume: 50, settings: { overlay: { mediashare: { theme: 'pixel' }, song: { theme: 'terminal' } } }, queue: { now_playing: null } }));
  assert.equal(page.elements.root.attributes['data-theme'], 'terminal', 'not the mediashare look');
  page.socket().receive(envelope(2, 'overlay_theme', { overlay: 'song', theme: 'candy' }));
  assert.equal(page.elements.root.attributes['data-theme'], 'candy');
  page.socket().receive(envelope(3, 'overlay_theme', { overlay: 'mediashare', theme: 'comic' }));
  assert.equal(page.elements.root.attributes['data-theme'], 'candy');

  const forced = load({ search: '?theme=receipt' });
  forced.socket().open();
  forced.socket().receive(envelope(1, 'overlay_theme', { overlay: 'song', theme: 'paper' }));
  assert.equal(forced.elements.root.attributes['data-theme'], 'receipt');
  const junk = load({ search: '?theme=neon' });
  junk.socket().open();
  junk.socket().receive(envelope(1, 'overlay_theme', { overlay: 'song', theme: 'paper' }));
  assert.equal(junk.elements.root.attributes['data-theme'], 'paper');
});

test('every look is drawn, and none of them touches the size of the cover', () => {
  for (const name of core.THEMES.filter((n) => n !== 'obsidian')) {
    assert.ok(css.includes('#root[data-theme="' + name + '"]'), name);
  }
  assert.ok(html.includes('data-theme="obsidian"'));
  // The player stays at least 200 x 200 in every look: no rule of a look sets its size.
  const themed = css.slice(css.indexOf('/* ---- obsidian'));
  assert.ok(!/--cover:|#cover\s*\{[^}]*(width|height)/.test(themed));
  assert.ok(!/display:\s*none/.test(themed.replace(/#paused|\[data-theme[^{]*#message:empty/g, '')), 'nothing is hidden by a look');
});

test('translated texts are fetched, with the token as a bearer header, and used', async () => {
  const page = load({ search: '?token=t', fetchBody: { 'overlay.song.kicker': 'Lagu pilihan penonton', 'overlay.requested_by': 'Dari {name}' } });
  assert.deepEqual(JSON.parse(JSON.stringify(page.log.fetches[0])), { url: '/api/i18n', options: { headers: { Authorization: 'Bearer t' } } });
  await flush();
  const player = page.ready();
  page.socket().open();
  page.socket().receive(envelope(1, 'play', { item: song(), volume: 50 }));
  assert.equal(page.elements.kicker.textContent, 'Lagu pilihan penonton');
  assert.equal(page.elements.requested.textContent, 'Dari Budi');
  assert.ok(player);
});
