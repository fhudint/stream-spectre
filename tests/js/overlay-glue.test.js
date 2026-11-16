'use strict';

// Runs static/overlay/overlay.js against hand-made stand-ins for the browser
// (document, WebSocket, fetch, timers) and the YouTube player.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const core = require('../../static/overlay/overlay-core.js');
const source = fs.readFileSync(path.join(__dirname, '../../static/overlay/overlay.js'), 'utf8');

const ID = 'dQw4w9WgXcQ';

class FakeElement {
  constructor(id, classes = []) {
    this.id = id;
    this.classes = new Set(classes);
    this._text = '';
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
  set innerHTML(_value) {
    throw new Error('innerHTML must never be assigned');
  }
}

function load({ search = '', protocol = 'http:', host = '127.0.0.1:8787', fetchBody = {}, fetchOk = true } = {}) {
  const log = { sockets: [], timeouts: [], intervals: [], fetches: [], scripts: [], players: [], position: 0 };

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
      this.onmessage({ data: typeof message === 'string' ? message : JSON.stringify(message) });
    }
    close() {
      this.readyState = 3;
      this.onclose();
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
    loadVideoById(videoId) {
      this.calls.push(['loadVideoById', videoId]);
    }
    stopVideo() {
      this.calls.push(['stopVideo']);
    }
  }

  const elements = {
    root: new FakeElement('root', ['hidden']),
    requester: new FakeElement('requester'),
    title: new FakeElement('title'),
  };

  const sandbox = {
    OverlayCore: core,
    URLSearchParams,
    JSON,
    Math,
    Object,
    String,
    Promise,
    encodeURIComponent,
    location: { search, protocol, host, origin: `${protocol}//${host}` },
    WebSocket: FakeWebSocket,
    document: {
      getElementById: (id) => elements[id],
      createElement: (tag) => ({ tag }),
      head: { appendChild: (el) => log.scripts.push(el) },
    },
    fetch: (url, options) => {
      log.fetches.push({ url, options });
      return Promise.resolve({ ok: fetchOk, json: () => Promise.resolve(fetchBody) });
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

  const socket = () => log.sockets.at(-1);
  const apiReady = () => sandbox.onYouTubeIframeAPIReady();
  return { log, elements, sandbox, socket, apiReady, player: () => log.players.at(-1) };
}

const envelope = (seq, type, data) => ({ v: 1, seq, ts: 0, type, data });
const item = (id = 1) => ({ id, video_id: ID, title: 'A <b>title</b>', requested_by: { name: 'Budi', role: 'follower' } });
// Objects made inside the vm context have another prototype; compare by value.
const plain = (value) => JSON.parse(JSON.stringify(value));
const flush = () => new Promise((resolve) => setImmediate(resolve));

test('it connects to the overlay socket and says it is ready', () => {
  const page = load();
  assert.equal(page.log.sockets.length, 1);
  assert.equal(page.socket().url, 'ws://127.0.0.1:8787/ws/overlay');
  page.socket().open();
  assert.deepEqual(page.socket().sent, [{ type: 'player_ready' }]);
});

test('https pages use wss and the token goes in the query, encoded', () => {
  const page = load({ search: '?token=a%26b%3D', protocol: 'https:', host: 'example.com' });
  assert.equal(page.socket().url, 'wss://example.com/ws/overlay?token=a%26b%3D');
});

test('translated texts are fetched, with the token as a bearer header', async () => {
  const page = load({ search: '?token=secret-token', fetchBody: { 'overlay.requested_by': 'Diminta oleh {name}' } });
  assert.equal(page.log.fetches[0].url, '/api/i18n');
  assert.deepEqual(plain(page.log.fetches[0].options.headers), { Authorization: 'Bearer secret-token' });
  await flush();
  page.apiReady();
  page.player().options.events.onReady();
  page.socket().open();
  page.socket().receive(envelope(1, 'play', { item: item(), volume: 40 }));
  assert.equal(page.elements.requester.textContent, 'Diminta oleh Budi');
});

test('without a token no auth header is sent and the query is empty', () => {
  const page = load();
  assert.deepEqual(plain(page.log.fetches[0].options.headers), {});
  assert.ok(!page.socket().url.includes('?'));
});

test('a failed translation request does not break the overlay', async () => {
  const page = load({ fetchOk: false });
  await flush();
  page.apiReady();
  page.player().options.events.onReady();
  page.socket().open();
  page.socket().receive(envelope(1, 'play', { item: item(), volume: 40 }));
  assert.equal(page.elements.requester.textContent, 'Requested by Budi');
});

test('it reconnects with a growing delay and starts over after a success', () => {
  const page = load();
  const delays = [];
  for (let i = 0; i < 6; i += 1) {
    page.socket().close();
    delays.push(page.log.timeouts.at(-1).delay);
    page.log.timeouts.at(-1).fn();
  }
  assert.deepEqual(delays, [1000, 2000, 4000, 8000, 10000, 10000]);
  assert.equal(page.log.sockets.length, 7);

  page.socket().open();
  page.socket().close();
  assert.equal(page.log.timeouts.at(-1).delay, 1000);
});

test('the YouTube player script is loaded and the player is created with safe options', () => {
  const page = load();
  assert.equal(page.log.scripts.length, 1);
  assert.equal(page.log.scripts[0].src, 'https://www.youtube.com/iframe_api');
  page.apiReady();
  const { options, id } = page.player();
  assert.equal(id, 'player');
  assert.equal(options.width, '100%');
  assert.equal(options.height, '100%');
  assert.equal(options.playerVars.autoplay, 1);
  assert.equal(options.playerVars.controls, 0);
  assert.equal(options.playerVars.origin, 'http://127.0.0.1:8787');
});

test('a play event loads the video and shows the caption as plain text', () => {
  const page = load();
  page.apiReady();
  page.player().options.events.onReady();
  page.socket().open();
  page.socket().receive(envelope(1, 'play', { item: item(), volume: 40 }));

  assert.deepEqual(page.player().calls.slice(-2), [['setVolume', 40], ['loadVideoById', ID]]);
  assert.equal(page.elements.title.textContent, 'A <b>title</b>');
  assert.ok(!page.elements.root.classList.contains('hidden'));
});

test('a play event that arrives before the player is ready starts when it is', () => {
  const page = load();
  page.socket().open();
  page.socket().receive(envelope(1, 'play', { item: item(), volume: 70 }));
  page.apiReady();
  assert.deepEqual(page.player().calls, []);
  page.player().options.events.onReady();
  assert.deepEqual(page.player().calls.slice(-2), [['setVolume', 70], ['loadVideoById', ID]]);
});

test('stop hides the overlay and stops the video', () => {
  const page = load();
  page.apiReady();
  page.player().options.events.onReady();
  page.socket().open();
  page.socket().receive(envelope(1, 'play', { item: item(), volume: 40 }));
  page.socket().receive(envelope(2, 'stop', {}));
  assert.deepEqual(page.player().calls.at(-1), ['stopVideo']);
  assert.ok(page.elements.root.classList.contains('hidden'));
  assert.equal(page.elements.requester.textContent, '');
  assert.equal(page.elements.title.textContent, '');
});

test('the end of the video is sent to the server', () => {
  const page = load();
  page.apiReady();
  page.player().options.events.onReady();
  page.socket().open();
  page.socket().receive(envelope(1, 'play', { item: item(7), volume: 40 }));

  page.player().options.events.onStateChange({ data: 1 }); // playing: nothing to report
  assert.deepEqual(page.socket().sent, [{ type: 'player_ready' }]);
  page.player().options.events.onStateChange({ data: 0 });
  assert.deepEqual(page.socket().sent.at(-1), { type: 'ended', item_id: 7 });
});

test('a player error is sent to the server with its code', () => {
  const page = load();
  page.apiReady();
  page.player().options.events.onReady();
  page.socket().open();
  page.socket().receive(envelope(1, 'play', { item: item(7), volume: 40 }));
  page.player().options.events.onError({ data: 150 });
  assert.deepEqual(page.socket().sent.at(-1), { type: 'player_error', item_id: 7, code: '150' });
});

test('if the YouTube script cannot load, the video is reported as an error', () => {
  const page = load();
  page.socket().open();
  page.socket().receive(envelope(1, 'play', { item: item(7), volume: 40 }));
  page.log.scripts[0].onerror();
  assert.deepEqual(page.socket().sent.at(-1), { type: 'player_error', item_id: 7, code: 'api_unavailable' });

  // Later videos fail at once instead of hanging.
  page.socket().receive(envelope(2, 'play', { item: item(8), volume: 40 }));
  assert.deepEqual(page.socket().sent.at(-1), { type: 'player_error', item_id: 8, code: 'api_unavailable' });
});

test('malformed socket messages are ignored', () => {
  const page = load();
  page.socket().open();
  for (const junk of ['not json', '', '[]', 'null', '{"v":1}']) {
    assert.doesNotThrow(() => page.socket().receive(junk), junk);
  }
  assert.ok(page.elements.root.classList.contains('hidden'));
});

test('a report made while the socket is down goes out after the reconnect', () => {
  const page = load();
  page.apiReady();
  page.player().options.events.onReady();
  page.socket().open();
  page.socket().receive(envelope(1, 'play', { item: item(7), volume: 40 }));

  const first = page.socket();
  first.close();
  page.player().options.events.onStateChange({ data: 0 }); // ended while offline
  page.log.timeouts.at(-1).fn();
  const second = page.socket();
  assert.notEqual(second, first);
  second.open();
  assert.deepEqual(second.sent, [{ type: 'player_ready' }, { type: 'ended', item_id: 7 }]);
});

test('progress is reported once a second, but only while the video is playing', () => {
  const page = load();
  assert.deepEqual(page.log.intervals.map((i) => i.delay), [1000]);
  const tick = page.log.intervals[0].fn;
  page.apiReady();
  page.player().options.events.onReady();
  page.socket().open();
  page.socket().receive(envelope(1, 'play', { item: item(7), volume: 40 }));
  const progress = () => page.socket().sent.filter((m) => m.type === 'progress');

  // Loading: the player has not started yet.
  page.log.position = 0;
  tick();
  assert.deepEqual(progress(), []);

  page.player().options.events.onStateChange({ data: 1 }); // playing
  page.log.position = 4.7;
  tick();
  assert.deepEqual(plain(progress()), [{ type: 'progress', item_id: 7, position_secs: 4 }]);

  page.player().options.events.onStateChange({ data: 2 }); // paused or buffering
  page.log.position = 9;
  tick();
  assert.equal(progress().length, 1);

  page.player().options.events.onStateChange({ data: 1 });
  page.socket().receive(envelope(2, 'stop', {}));
  tick();
  assert.equal(progress().length, 1, 'nothing after the screen is cleared');
});
