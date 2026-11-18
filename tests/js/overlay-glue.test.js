'use strict';

// Runs static/overlay/mediashare/overlay.js against hand-made stand-ins for the browser
// (document, WebSocket, fetch, timers) and the YouTube player.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const core = require('../../static/overlay/mediashare/overlay-core.js');
const source = fs.readFileSync(path.join(__dirname, '../../static/overlay/mediashare/overlay.js'), 'utf8');

const ID = 'dQw4w9WgXcQ';

class FakeElement {
  constructor(id, classes = []) {
    this.id = id;
    this.tag = id;
    this.classes = new Set(classes);
    this.children = [];
    this.attrs = {};
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
  setAttribute(name, value) {
    this.attrs[name] = String(value);
  }
  appendChild(child) {
    this.children.push(child);
    return child;
  }
  replaceChildren(...nodes) {
    this.children = nodes;
  }
  set innerHTML(_value) {
    throw new Error('innerHTML must never be assigned');
  }
}

function load({ search = '', protocol = 'http:', host = '127.0.0.1:8787', fetchBody = {}, fetchOk = true } = {}) {
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
    getDuration() {
      return log.duration;
    }
    loadVideoById(videoId) {
      this.calls.push(['loadVideoById', videoId]);
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

  const elements = {
    root: new FakeElement('root', ['hidden']),
    kicker: new FakeElement('kicker'),
    left: new FakeElement('left'),
    requested: new FakeElement('requested'),
    name: new FakeElement('name'),
    role: new FakeElement('role'),
    channel: new FakeElement('channel'),
    time: new FakeElement('time'),
    avatar: new FakeElement('avatar'),
    title: new FakeElement('title'),
    stage: new FakeElement('stage'),
    tiktok: new FakeElement('tiktok'),
    'bar-fill': new FakeElement('bar-fill'),
    'paused-label': new FakeElement('paused-label'),
  };
  elements['bar-fill'].style = {};

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
      createElement: (tag) => {
        const element = new FakeElement(tag);
        element.tag = tag;
        if (tag === 'iframe') {
          // What the page sends to TikTok's player, with the origin it names.
          element.contentWindow = { posted: [], postMessage(data, origin) { this.posted.push({ data: plain(data), origin }); } };
        }
        return element;
      },
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
    clearTimeout: (id) => {
      if (log.timeouts[id - 1]) log.timeouts[id - 1].cleared = true;
    },
    setInterval: (fn, delay) => {
      log.intervals.push({ fn, delay });
      return log.intervals.length;
    },
    YT: { Player: FakePlayer, PlayerState: { ENDED: 0, PLAYING: 1 } },
  };
  sandbox.window = sandbox;
  const windowListeners = {};
  sandbox.addEventListener = (type, listener) => (windowListeners[type] ||= []).push(listener);
  vm.runInNewContext(source, sandbox);

  const socket = () => log.sockets.at(-1);
  const apiReady = () => sandbox.onYouTubeIframeAPIReady();
  // A message to the page from another window, as the browser delivers it.
  const message = (event) => (windowListeners.message || []).forEach((listener) => listener(event));
  return { log, elements, sandbox, socket, apiReady, message, player: () => log.players.at(-1) };
}

const envelope = (seq, type, data) => ({ v: 1, seq, ts: 0, type, data });
const item = (id = 1) => ({ id, video_id: ID, title: 'A <b>title</b>', requested_by: { name: 'Budi', role: 'follower' } });
// Objects made inside the vm context have another prototype; compare by value.
const plain = (value) => JSON.parse(JSON.stringify(value));
const flush = () => new Promise((resolve) => setImmediate(resolve));

test('it connects to the overlay socket and says it is ready', () => {
  const page = load();
  assert.equal(page.log.sockets.length, 1);
  assert.equal(page.socket().url, 'ws://127.0.0.1:8787/ws/overlay/mediashare');
  page.socket().open();
  assert.deepEqual(page.socket().sent, [{ type: 'player_ready' }]);
});

test('https pages use wss and the token goes in the query, encoded', () => {
  const page = load({ search: '?token=a%26b%3D', protocol: 'https:', host: 'example.com' });
  assert.equal(page.socket().url, 'wss://example.com/ws/overlay/mediashare?token=a%26b%3D');
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
  assert.equal(page.elements.requested.textContent, 'Diminta oleh Budi');
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
  assert.equal(page.elements.requested.textContent, 'Requested by Budi');
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
  for (const id of ['kicker', 'left', 'requested', 'name', 'role', 'title', 'channel', 'time', 'avatar']) {
    assert.equal(page.elements[id].textContent, '', `#${id} is emptied`);
  }
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
  assert.deepEqual(page.log.intervals.map((i) => i.delay), [1000, 250], "the second one moves the line under the video");
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
  assert.deepEqual(plain(progress()), [{ type: 'progress', item_id: 7, position_secs: 4, duration_secs: 200 }]);

  page.player().options.events.onStateChange({ data: 2 }); // paused or buffering
  page.log.position = 9;
  tick();
  assert.equal(progress().length, 1);

  page.player().options.events.onStateChange({ data: 1 });
  page.socket().receive(envelope(2, 'stop', {}));
  tick();
  assert.equal(progress().length, 1, 'nothing after the screen is cleared');
});

test('a pause from the server pauses the player, and a resume plays it again', () => {
  const page = load();
  page.apiReady();
  page.player().options.events.onReady();
  page.socket().open();
  page.socket().receive(envelope(1, 'play', { item: item(7), volume: 40 }));
  page.player().options.events.onStateChange({ data: 1 });
  page.player().calls.length = 0;

  page.socket().receive(envelope(2, 'playback_paused', { item_id: 7, paused: true }));
  assert.deepEqual(page.player().calls, [['pauseVideo']]);
  page.socket().receive(envelope(3, 'playback_paused', { item_id: 7, paused: false }));
  assert.deepEqual(page.player().calls.at(-1), ['playVideo']);
});

test('a video that is paused while it loads is stopped as soon as it starts', () => {
  const page = load();
  page.apiReady();
  page.socket().open();
  // The player is not ready yet: the pause is wanted before the video even loads.
  page.socket().receive({ v: 1, seq: 1, ts: 0, type: 'hello', data: { volume: 40, playback_paused: true, queue: { pending: [], approved: [], now_playing: item(7) } } });
  page.player().options.events.onReady();
  page.player().calls.length = 0;
  page.player().options.events.onStateChange({ data: 1 }); // autoplay begins
  assert.deepEqual(page.player().calls, [['pauseVideo']]);
});

test('the next video plays even after a pause, and stopping forgets the pause', () => {
  const page = load();
  page.apiReady();
  page.player().options.events.onReady();
  page.socket().open();
  page.socket().receive(envelope(1, 'play', { item: item(7), volume: 40 }));
  page.socket().receive(envelope(2, 'playback_paused', { item_id: 7, paused: true }));
  page.socket().receive(envelope(3, 'play', { item: item(8), volume: 40 }));
  page.player().calls.length = 0;
  page.player().options.events.onStateChange({ data: 1 });
  assert.deepEqual(page.player().calls, [], 'the new video is left playing');

  page.socket().receive(envelope(4, 'playback_paused', { item_id: 8, paused: true }));
  page.socket().receive(envelope(5, 'stop', {}));
  page.socket().receive(envelope(6, 'play', { item: item(9), volume: 40 }));
  page.player().calls.length = 0;
  page.player().options.events.onStateChange({ data: 1 });
  assert.deepEqual(page.player().calls, []);
});

test('no progress is reported while the video is paused', () => {
  const page = load();
  const tick = page.log.intervals[0].fn;
  page.apiReady();
  page.player().options.events.onReady();
  page.socket().open();
  page.socket().receive(envelope(1, 'play', { item: item(7), volume: 40 }));
  page.player().options.events.onStateChange({ data: 1 });
  page.log.position = 12;
  tick();
  const progress = () => page.socket().sent.filter((m) => m.type === 'progress').length;
  assert.equal(progress(), 1);
  page.socket().receive(envelope(2, 'playback_paused', { item_id: 7, paused: true }));
  page.player().options.events.onStateChange({ data: 2 }); // the player reports PAUSED
  tick();
  assert.equal(progress(), 1);
});

// --- TikTok videos ---

const TT = '7700124541145599252';
const tiktokItem = (id = 1, extra = {}) => ({ ...item(id), source: 'tiktok', video_id: TT, title: '@cherlynn052', ...extra });
const frames = (page) => page.elements.tiktok.children;
const readyPage = () => {
  const page = load();
  page.apiReady();
  page.player().options.events.onReady();
  page.socket().open();
  return page;
};

test('a TikTok video is shown in TikTok\'s own frame, and YouTube is stopped', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: item(1), volume: 40 }));
  page.socket().receive(envelope(2, 'play', { item: tiktokItem(2), volume: 40 }));

  assert.equal(frames(page).length, 1);
  const frame = frames(page)[0];
  assert.equal(frame.tag, 'iframe');
  assert.equal(frame.src, `https://www.tiktok.com/player/v1/${TT}?autoplay=1&music_info=1&description=1`);
  assert.match(frame.attrs.allow, /autoplay/);
  assert.ok(page.elements.stage.classes.has('tiktok'));
  assert.ok(!page.elements.root.classes.has('hidden'));
  assert.equal(page.elements.title.textContent, '@cherlynn052');
  assert.equal(page.elements.requested.textContent, 'Requested by Budi');
  // The YouTube video that was playing is stopped, and nothing new is loaded in it.
  const calls = page.player().calls.map((c) => c[0]);
  assert.equal(calls.filter((c) => c === 'loadVideoById').length, 1, 'only the first, YouTube, video');
  assert.equal(calls.at(-1), 'stopVideo');
});

test('a TikTok video can start before the YouTube player is ready', () => {
  const page = load();
  page.socket().open();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(1), volume: 40 }));
  assert.equal(frames(page).length, 1);
  // The player becoming ready later must not start anything.
  page.apiReady();
  page.player().options.events.onReady();
  assert.ok(!page.player().calls.some((c) => c[0] === 'loadVideoById'));
});

test('the next YouTube video takes the screen back and removes the frame', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(1), volume: 40 }));
  page.socket().receive(envelope(2, 'play', { item: item(2), volume: 40 }));
  assert.equal(frames(page).length, 0);
  assert.ok(!page.elements.stage.classes.has('tiktok'));
  assert.deepEqual(plain(page.player().calls.at(-1)), ['loadVideoById', ID]);
});

test('a second TikTok video replaces the first frame instead of adding one', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(1), volume: 40 }));
  page.socket().receive(envelope(2, 'play', { item: tiktokItem(2, { video_id: '7219950637247728923' }), volume: 40 }));
  assert.equal(frames(page).length, 1);
  assert.match(frames(page)[0].src, /7219950637247728923/);
});

test('skip and stop take the TikTok frame away', () => {
  for (const [type, data] of [['skip', { item_id: 1, reason: 'moderator' }], ['stop', {}]]) {
    const page = readyPage();
    page.socket().receive(envelope(1, 'play', { item: tiktokItem(1), volume: 40 }));
    page.socket().receive(envelope(2, type, data));
    assert.equal(frames(page).length, 0, type);
    assert.ok(!page.elements.stage.classes.has('tiktok'), type);
    assert.ok(page.elements.root.classes.has('hidden'), type);
  }
});

test('a TikTok video is paused and resumed in its own player, not in YouTube\'s', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(1), volume: 40 }));
  const before = page.player().calls.length;
  page.socket().receive(envelope(2, 'playback_paused', { item_id: 1, paused: true }));
  page.socket().receive(envelope(3, 'playback_paused', { item_id: 1, paused: false }));
  const touched = plain(page.player().calls.slice(before)).map((c) => c[0]);
  assert.ok(!touched.includes('pauseVideo') && !touched.includes('playVideo'), touched.join());
  assert.deepEqual(posted(page).map((p) => p.data.type), ['pause', 'play']);
  assert.ok(posted(page).every((p) => p.origin === TT_ORIGIN && p.data['x-tiktok-player'] === true));
  assert.equal(frames(page).length, 1, 'still there');
});

test('a TikTok video reports its position and its length once its player has said them', () => {
  const page = readyPage();
  page.log.position = 12; // YouTube's clock is not the one that counts
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(1), volume: 40 }));
  const sent = page.socket().sent.length;
  page.log.intervals[0].fn();
  assert.equal(page.socket().sent.length, sent, 'nothing to report before the player speaks');

  fromPlayer(page, 'onCurrentTime', { currentTime: 5.4, duration: 15.04 });
  page.log.intervals[0].fn();
  assert.deepEqual(page.socket().sent.at(-1), { type: 'progress', item_id: 1, position_secs: 5, duration_secs: 16 });

  // A YouTube video says the length its player has too; the server uses it only when it has none.
  page.socket().receive(envelope(2, 'play', { item: item(2), volume: 40 }));
  page.log.position = 7;
  page.player().options.events.onStateChange({ data: 1 });
  page.log.intervals[0].fn();
  assert.deepEqual(page.socket().sent.at(-1), { type: 'progress', item_id: 2, position_secs: 7, duration_secs: 200 });
  page.log.duration = 0; // its player does not know it yet
  page.log.intervals[0].fn();
  assert.deepEqual(page.socket().sent.at(-1), { type: 'progress', item_id: 2, position_secs: 7 });
});

test('a TikTok video paused before its player is ready is paused as soon as it plays', () => {
  const page = readyPage();
  // What the page told the player about playing; the sound commands are left out.
  const told = () => posted(page).map((p) => p.data.type).filter((type) => type === 'pause' || type === 'play');
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(1), volume: 40 }));
  page.socket().receive(envelope(2, 'playback_paused', { item_id: 1, paused: true }));
  assert.deepEqual(told(), ['pause'], 'sent at once, though the player drops it');
  fromPlayer(page, 'onStateChange', 3); // buffering: nothing yet
  assert.deepEqual(told(), ['pause']);
  fromPlayer(page, 'onStateChange', 1); // it plays after all
  assert.deepEqual(told(), ['pause', 'pause']);

  // Resumed, it is let play.
  page.socket().receive(envelope(3, 'playback_paused', { item_id: 1, paused: false }));
  fromPlayer(page, 'onStateChange', 1);
  assert.deepEqual(told(), ['pause', 'pause', 'play']);
});

test('the pause a moderator asks for is not mistaken for the browser refusing sound', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(1), volume: 50 }));
  fromPlayer(page, 'onCurrentTime', { currentTime: 0.2, duration: 15 }); // sound asked for
  page.socket().receive(envelope(2, 'playback_paused', { item_id: 1, paused: true }));
  fromPlayer(page, 'onStateChange', 2); // the player says it paused, a moment after asking
  const types = posted(page).map((p) => p.data.type);
  assert.ok(!types.includes('mute'), types.join());
  assert.ok(!types.slice(types.indexOf('pause') + 1).includes('play'), 'and it is not set playing again');
});

test('a reload in the middle of a TikTok video shows it again', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'hello', { volume: 40, playback_paused: false, queue: { now_playing: tiktokItem(5) } }));
  assert.equal(frames(page).length, 1);
  assert.ok(page.elements.stage.classes.has('tiktok'));
});

test('an ID that is not a TikTok ID never reaches the frame', () => {
  const page = readyPage();
  for (const [index, bad] of ['dQw4w9WgXcQ', '77001245411455992"onload="x', '../x', ''].entries()) {
    page.socket().receive(envelope(index + 1, 'play', { item: tiktokItem(index + 1, { video_id: bad }), volume: 40 }));
    assert.equal(frames(page).length, 0, bad);
  }
  const errors = page.socket().sent.filter((m) => m.type === 'player_error');
  assert.equal(errors.length, 4);
  assert.ok(errors.every((m) => m.code === 'invalid'));
});

test('the frame is never given markup and carries no text of the viewers', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(1, { title: '<img src=x onerror=alert(1)>' }), volume: 40 }));
  assert.equal(page.elements.title.textContent, '<img src=x onerror=alert(1)>');
  assert.ok(!frames(page)[0].src.includes('onerror'));
});

// --- TikTok's sound ---

const TT_ORIGIN = 'https://www.tiktok.com';
const fromPlayer = (page, type, value, extra = {}) =>
  page.message({ origin: TT_ORIGIN, source: frames(page)[0].contentWindow, data: { 'x-tiktok-player': true, type, value }, ...extra });
const posted = (page) => frames(page)[0].contentWindow.posted;

test('the TikTok player is asked for sound, with the volume, once it speaks', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(1), volume: 35 }));
  assert.deepEqual(posted(page), [], 'nothing before the player says something');
  fromPlayer(page, 'onCurrentTime', { currentTime: 0.2, duration: 23 });
  assert.deepEqual(posted(page), [
    { data: { 'x-tiktok-player': true, type: 'setVolume', value: 35 }, origin: TT_ORIGIN },
    { data: { 'x-tiktok-player': true, type: 'unMute' }, origin: TT_ORIGIN },
  ]);
});

test('commands go only to TikTok\'s origin, never to any origin', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(1), volume: 35 }));
  fromPlayer(page, 'onCurrentTime', {});
  assert.ok(posted(page).length > 0);
  assert.ok(posted(page).every((p) => p.origin === TT_ORIGIN));
});

test('when the browser refuses sound the video is muted and set playing again', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(1), volume: 50 }));
  fromPlayer(page, 'onCurrentTime', {});
  fromPlayer(page, 'onMute', false);
  fromPlayer(page, 'onStateChange', 2);
  assert.deepEqual(posted(page).slice(2).map((p) => p.data.type), ['mute', 'play']);
  const count = posted(page).length;
  fromPlayer(page, 'onMute', true);
  fromPlayer(page, 'onCurrentTime', {});
  assert.equal(posted(page).length, count, 'it stays silent and is left alone');
});

test('a message from anywhere but the TikTok frame on screen is ignored', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(1), volume: 50 }));
  const data = { 'x-tiktok-player': true, type: 'onCurrentTime', value: {} };
  page.message({ origin: 'https://evil.example', source: frames(page)[0].contentWindow, data });
  page.message({ origin: TT_ORIGIN, source: {}, data });
  page.message({ origin: TT_ORIGIN, source: null, data });
  page.message({ origin: TT_ORIGIN, data });
  assert.deepEqual(posted(page), []);
});

test('with no TikTok frame on screen its messages are ignored', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(1), volume: 50 }));
  const old = frames(page)[0].contentWindow;
  page.socket().receive(envelope(2, 'play', { item: item(2), volume: 50 }));
  page.message({ origin: TT_ORIGIN, source: old, data: { 'x-tiktok-player': true, type: 'onCurrentTime', value: {} } });
  assert.deepEqual(old.posted, []);
});

test('a later TikTok video starts its sound over, and an old frame cannot answer for it', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(1), volume: 50 }));
  const first = frames(page)[0].contentWindow;
  fromPlayer(page, 'onCurrentTime', {});
  page.socket().receive(envelope(2, 'play', { item: tiktokItem(2, { video_id: '7219950637247728923' }), volume: 50 }));
  const second = frames(page)[0].contentWindow;
  assert.notEqual(first, second);
  page.message({ origin: TT_ORIGIN, source: first, data: { 'x-tiktok-player': true, type: 'onCurrentTime', value: {} } });
  assert.deepEqual(second.posted, [], 'the old frame is gone');
  fromPlayer(page, 'onCurrentTime', {});
  assert.deepEqual(second.posted.map((p) => p.data.type), ['setVolume', 'unMute']);
});

test('the volume from the dashboard reaches the TikTok player once it is unmuted', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(1), volume: 50 }));
  page.socket().receive(envelope(2, 'volume', { volume: 10 }));
  assert.deepEqual(posted(page), [], 'not unmuted yet, so the player is left alone');
  fromPlayer(page, 'onCurrentTime', {});
  fromPlayer(page, 'onMute', false);
  const before = posted(page).length;
  page.socket().receive(envelope(3, 'volume', { volume: 70 }));
  assert.deepEqual(posted(page).slice(before).map((p) => p.data), [{ 'x-tiktok-player': true, type: 'setVolume', value: 70 }]);
});

test('a volume change while YouTube plays does not touch TikTok', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(1), volume: 50 }));
  const old = frames(page)[0].contentWindow;
  page.socket().receive(envelope(2, 'play', { item: item(2), volume: 50 }));
  page.socket().receive(envelope(3, 'volume', { volume: 70 }));
  assert.deepEqual(old.posted, []);
});

test('a video after a refused one gets its own try at sound', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(1), volume: 50 }));
  fromPlayer(page, 'onCurrentTime', {});
  fromPlayer(page, 'onStateChange', 2); // refused: muted on purpose
  assert.deepEqual(posted(page).slice(2).map((p) => p.data.type), ['mute', 'play']);

  page.socket().receive(envelope(2, 'play', { item: tiktokItem(2, { video_id: '7219950637247728923' }), volume: 50 }));
  fromPlayer(page, 'onCurrentTime', {});
  assert.deepEqual(posted(page).map((p) => p.data.type), ['setVolume', 'unMute']);
});

// --- the end of a TikTok video, and videos that cannot be played ---

const reports = (page) => page.socket().sent.filter((m) => m.type === 'ended' || m.type === 'player_error');

test('when TikTok\'s player says the video is over, the server is told', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(4), volume: 50 }));
  fromPlayer(page, 'onStateChange', 1);
  assert.deepEqual(reports(page), []);
  fromPlayer(page, 'onStateChange', 0);
  assert.deepEqual(reports(page), [{ type: 'ended', item_id: 4 }]);
  // Said again, or followed by an error, it is still reported once.
  fromPlayer(page, 'onStateChange', 0);
  fromPlayer(page, 'onPlayerError', { errorCode: 1001 });
  assert.equal(reports(page).length, 1);
});

// --- the "Paused" notice ---

test('a paused video says so over the picture, and a new video, a skip or a stop takes it away', () => {
  const page = readyPage();
  const paused = () => page.elements.root.classList.contains('paused');
  page.socket().receive(envelope(1, 'play', { item: item(1), volume: 50 }));
  assert.ok(!paused());
  page.socket().receive(envelope(2, 'playback_paused', { item_id: 1, paused: true }));
  assert.ok(paused());
  assert.equal(page.elements['paused-label'].textContent, 'Paused', 'the English text when there are no others');
  page.socket().receive(envelope(3, 'playback_paused', { item_id: 1, paused: false }));
  assert.ok(!paused());
  assert.equal(page.elements['paused-label'].textContent, '');

  page.socket().receive(envelope(4, 'playback_paused', { item_id: 1, paused: true }));
  page.socket().receive(envelope(5, 'play', { item: item(2), volume: 50 }));
  assert.ok(!paused(), 'the next video plays');
  page.socket().receive(envelope(6, 'playback_paused', { item_id: 2, paused: true }));
  page.socket().receive(envelope(7, 'stop', {}));
  assert.ok(!paused(), 'nothing is left on screen to be paused');
  assert.equal(page.elements['paused-label'].textContent, '');
});

// --- the line under the video ---

const barStep = (page) => {
  page.log.intervals.find((i) => i.delay === 250).fn();
  return page.elements['bar-fill'].style.transform;
};

test('the line under a YouTube video follows how much of it is done, also while paused', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: item(1), volume: 50 }));
  assert.equal(barStep(page), 'scaleX(0.0000)', 'nothing yet');
  page.log.position = 50;
  assert.equal(barStep(page), 'scaleX(0.2500)');
  page.socket().receive(envelope(2, 'playback_paused', { item_id: 1, paused: true }));
  assert.equal(barStep(page), 'scaleX(0.2500)', 'a pause does not empty it');
  page.log.duration = 0; // the length is not known yet
  assert.equal(barStep(page), 'scaleX(0.0000)');
});

test('the line under a TikTok video follows what its player says, and starts over with the next one', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(4), volume: 50 }));
  assert.equal(barStep(page), 'scaleX(0.0000)');
  fromPlayer(page, 'onCurrentTime', { currentTime: 5, duration: 20 });
  assert.equal(barStep(page), 'scaleX(0.2500)', 'not the YouTube player position, which is 0');
  fromPlayer(page, 'onCurrentTime', { currentTime: 25, duration: 20 });
  assert.equal(barStep(page), 'scaleX(1.0000)', 'never past the end');
  fromPlayer(page, 'onCurrentTime', 'nonsense');
  assert.equal(barStep(page), 'scaleX(1.0000)', 'what is not a time changes nothing');

  page.socket().receive(envelope(2, 'play', { item: tiktokItem(5, { video_id: '7219950637247728923' }), volume: 50 }));
  assert.equal(barStep(page), 'scaleX(0.0000)', 'the next video starts empty');
});

// --- a TikTok video that never starts ---

const startTimers = (page) => page.log.timeouts.filter((t) => t.delay === 15000);

test('a TikTok video that never starts is reported as failed after 15 seconds', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(4), volume: 50 }));
  const [timer, ...others] = startTimers(page);
  assert.equal(others.length, 0);
  fromPlayer(page, 'onPlayerReady', {}); // ready is not playing
  assert.ok(!timer.cleared);
  assert.deepEqual(reports(page), []);

  timer.fn();
  assert.deepEqual(reports(page), [{ type: 'player_error', item_id: 4, code: 'timeout' }]);
});

test('a TikTok video that plays, or fails, or is replaced, is not timed out', () => {
  for (const signal of [['onStateChange', 1], ['onCurrentTime', {}], ['onPlayerError', { errorCode: 1001 }], ['onStateChange', 0]]) {
    const page = readyPage();
    page.socket().receive(envelope(1, 'play', { item: tiktokItem(4), volume: 50 }));
    fromPlayer(page, ...signal);
    assert.ok(startTimers(page)[0].cleared, signal.join(' '));
  }
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(4), volume: 50 }));
  page.socket().receive(envelope(2, 'play', { item: tiktokItem(5, { video_id: '7219950637247728923' }), volume: 50 }));
  const [first, second] = startTimers(page);
  assert.ok(first.cleared && !second.cleared, 'a new video starts its own count');
  page.socket().receive(envelope(3, 'skip', { item_id: 5, reason: 'moderator' }));
  assert.ok(second.cleared, 'skipping takes the count away');
});

test('YouTube videos have no start timer', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: item(1), volume: 50 }));
  assert.equal(startTimers(page).length, 0);
});

test('the pause TikTok sends just before the end does not stop the video from ending', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(4), volume: 50 }));
  fromPlayer(page, 'onCurrentTime', { currentTime: 0.2, duration: 23.7 });
  fromPlayer(page, 'onCurrentTime', { currentTime: 23.7, duration: 23.7 });
  fromPlayer(page, 'onStateChange', 2);
  fromPlayer(page, 'onStateChange', 0);
  assert.deepEqual(reports(page), [{ type: 'ended', item_id: 4 }]);
  assert.ok(!posted(page).some((p) => p.data.type === 'mute' || p.data.type === 'play'), 'not muted and not played again');
});

test('a video TikTok cannot play is reported as an error with its code, once', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(9), volume: 50 }));
  fromPlayer(page, 'onError', 1);
  assert.deepEqual(reports(page), [], 'the short form says nothing about why');
  fromPlayer(page, 'onPlayerError', { errorCode: 1001, errorType: 'INVALID_VIDEO' });
  fromPlayer(page, 'onPlayerError', { errorCode: 1001, errorType: 'INVALID_VIDEO' });
  assert.deepEqual(reports(page), [{ type: 'player_error', item_id: 9, code: '1001' }]);
});

test('an end or an error from anything but the TikTok frame on screen is ignored', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(4), volume: 50 }));
  const ended = { 'x-tiktok-player': true, type: 'onStateChange', value: 0 };
  const failed = { 'x-tiktok-player': true, type: 'onPlayerError', value: { errorCode: 1001 } };
  for (const data of [ended, failed]) {
    page.message({ origin: 'https://evil.example', source: frames(page)[0].contentWindow, data });
    page.message({ origin: TT_ORIGIN, source: {}, data });
    page.message({ origin: TT_ORIGIN, data });
  }
  assert.deepEqual(reports(page), []);
});

test('the end of an old TikTok video cannot end the next one', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(4), volume: 50 }));
  const old = frames(page)[0].contentWindow;
  page.socket().receive(envelope(2, 'play', { item: tiktokItem(5, { video_id: '7219950637247728923' }), volume: 50 }));
  page.message({ origin: TT_ORIGIN, source: old, data: { 'x-tiktok-player': true, type: 'onStateChange', value: 0 } });
  assert.deepEqual(reports(page), []);
  fromPlayer(page, 'onStateChange', 0);
  assert.deepEqual(reports(page), [{ type: 'ended', item_id: 5 }]);
});

test('a video that skipped by a moderator is not reported as ended afterwards', () => {
  const page = readyPage();
  page.socket().receive(envelope(1, 'play', { item: tiktokItem(4), volume: 50 }));
  const win = frames(page)[0].contentWindow;
  page.socket().receive(envelope(2, 'skip', { item_id: 4, reason: 'moderator' }));
  page.message({ origin: TT_ORIGIN, source: win, data: { 'x-tiktok-player': true, type: 'onStateChange', value: 0 } });
  assert.deepEqual(reports(page), []);
});


// --- the looks of the overlay ---

const themeOf = (page) => page.elements.root.attrs['data-theme'];

function playing(options, extra = {}) {
  const page = load(options);
  page.apiReady();
  page.player().options.events.onReady();
  page.socket().open();
  page.socket().receive(envelope(1, 'play', { item: { ...item(), ...extra }, volume: 40 }));
  return page;
}

test('the look comes from the server: in the hello and when it changes', () => {
  const page = load();
  page.socket().open();
  page.socket().receive({ ...envelope(1, 'hello', { volume: 50, queue: { now_playing: null }, settings: { overlay: { mediashare: { theme: 'pixel' } } } }) });
  assert.equal(themeOf(page), 'pixel');
  page.socket().receive(envelope(2, 'overlay_theme', { overlay: 'mediashare', theme: 'comic' }));
  assert.equal(themeOf(page), 'comic');
  page.socket().receive(envelope(3, 'overlay_theme', { overlay: 'mediashare', theme: 'neon<script>' }));
  assert.equal(themeOf(page), 'comic', 'a name the page has no look for changes nothing');
});

test('?theme= in the address wins over the server, but only for a look that exists', () => {
  const forced = load({ search: '?token=t&theme=receipt' });
  forced.socket().open();
  forced.socket().receive(envelope(1, 'hello', { volume: 50, queue: { now_playing: null }, settings: { overlay: { mediashare: { theme: 'pixel' } } } }));
  assert.equal(themeOf(forced), 'receipt');
  forced.socket().receive(envelope(2, 'overlay_theme', { overlay: 'mediashare', theme: 'outline' }));
  assert.equal(themeOf(forced), 'receipt');

  const wrong = load({ search: '?theme=nope' });
  wrong.socket().open();
  wrong.socket().receive(envelope(1, 'overlay_theme', { overlay: 'mediashare', theme: 'outline' }));
  assert.equal(themeOf(wrong), 'outline');
});

test('the card gets the name, the role, the channel and a letter for the avatar, as plain text', () => {
  const page = playing(
    { fetchBody: { 'role.follower': 'Pengikut', 'overlay.label.from': 'Dari', 'overlay.kicker': 'Permintaan' } },
    { channel: 'Chan <i>x</i>', requested_by: { name: '@budi', role: 'follower' } }
  );
  assert.equal(page.elements.name.textContent, '@budi');
  assert.equal(page.elements.avatar.textContent, 'B');
  assert.equal(page.elements.channel.textContent, 'Chan <i>x</i>');
  assert.equal(page.elements.role.textContent, '', 'texts are not loaded yet in this test');
  assert.equal(page.elements.kicker.textContent, 'Request from chat');
  assert.equal(page.elements.name.attrs['data-label'], 'From');
});

test('the translated texts are used once they are loaded', async () => {
  const page = load({ fetchBody: { 'role.follower': 'Pengikut', 'overlay.label.from': 'Dari', 'overlay.kicker': 'Permintaan' } });
  await flush();
  page.apiReady();
  page.player().options.events.onReady();
  page.socket().open();
  page.socket().receive(envelope(1, 'play', { item: item(), volume: 40 }));
  assert.equal(page.elements.role.textContent, 'Pengikut');
  assert.equal(page.elements.kicker.textContent, 'Permintaan');
  assert.equal(page.elements.name.attrs['data-label'], 'Dari');
});

test('the clock shows the time and what is left, and nothing while the length is not known', () => {
  const page = playing();
  const step = () => page.log.intervals.find((i) => i.delay === 250).fn();
  page.log.position = 23;
  page.log.duration = 45;
  step();
  assert.equal(page.elements.time.textContent, '0:23 / 0:45');
  assert.equal(page.elements.left.textContent, '0:22 left');
  page.log.duration = 0;
  step();
  assert.equal(page.elements.left.textContent, '');
});

test('a message from the viewer is shown as plain text in place of the title', () => {
  const page = playing({}, { message: 'halo <b>ka</b>', channel: 'Chan' });
  assert.equal(page.elements.title.textContent, 'halo <b>ka</b>');
  assert.equal(page.elements.channel.textContent, 'A <b>title</b>');
});
