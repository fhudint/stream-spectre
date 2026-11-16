'use strict';

// Runs static/dashboard/dashboard.js against a small stand-in for the browser
// built from the real index.html, with hand-made fetch and WebSocket.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const root = path.join(__dirname, '../..');
const core = require(path.join(root, 'static/dashboard/dashboard-core.js'));
const html = fs.readFileSync(path.join(root, 'static/dashboard/index.html'), 'utf8');
const source = fs.readFileSync(path.join(root, 'static/dashboard/dashboard.js'), 'utf8');
const english = JSON.parse(fs.readFileSync(path.join(root, 'locales/en.json'), 'utf8'));

class FakeNode {
  constructor(tag) {
    this.tag = tag;
    this.children = [];
    this.attrs = {};
    this.listeners = {};
    this.classes = new Set();
    this._text = '';
    this.value = '';
    this.max = '';
  }
  get className() {
    return [...this.classes].join(' ');
  }
  set className(value) {
    this.classes = new Set(String(value).split(/\s+/).filter(Boolean));
  }
  get classList() {
    return {
      add: (name) => this.classes.add(name),
      remove: (name) => this.classes.delete(name),
      contains: (name) => this.classes.has(name),
    };
  }
  get textContent() {
    return this.children.length ? this.children.map((c) => c.textContent).join('') : this._text;
  }
  set textContent(value) {
    this.children = [];
    this._text = String(value);
  }
  set innerHTML(_value) {
    throw new Error('innerHTML must never be assigned');
  }
  setAttribute(name, value) {
    this.attrs[name] = String(value);
  }
  getAttribute(name) {
    return name in this.attrs ? this.attrs[name] : null;
  }
  addEventListener(type, listener) {
    (this.listeners[type] ||= []).push(listener);
  }
  appendChild(child) {
    this.children.push(child);
    return child;
  }
  replaceChildren(...nodes) {
    this.children = nodes;
    this._text = '';
  }
  fire(type) {
    for (const listener of this.listeners[type] || []) listener({ preventDefault() {} });
  }
  get hidden() {
    return this.classes.has('hidden');
  }
}

function descendants(node) {
  return node.children.flatMap((child) => [child, ...descendants(child)]);
}
const buttons = (node, action) =>
  descendants(node).filter((n) => n.tag === 'button' && n.attrs['data-action'] === action);

const ITEM_ID = 'dQw4w9WgXcQ';
const item = (id, extra = {}) => ({
  id,
  video_id: ITEM_ID,
  title: `Title ${id}`,
  channel: 'Some Channel',
  duration_secs: 213,
  thumbnail_url: `https://i.ytimg.com/vi/${ITEM_ID}/mqdefault.jpg`,
  requested_by: { name: `Viewer ${id}`, role: 'follower' },
  ...extra,
});

const snapshot = (extra = {}) => ({
  server_version: '0.1.0',
  language: 'en',
  seq: 5,
  settings: {
    moderation: {
      mode: 'manual', min_role: 'follower', min_gifter_level: 0, min_fan_club_level: 0,
      cooldown_secs: 60, max_duration_secs: 600, max_queue_len: 20, volume: 50, max_volume: 80,
    },
    blacklist: { video_ids: ['aaaaaaaaaaa'], channels: [], title_words: [] },
  },
  volume: 50,
  queue: { pending: [item(7), item(8)], approved: [item(9)], now_playing: item(3) },
  chat: { state: 'connected', channel: 'streamer', text: 'Connected to TikTok chat @streamer' },
  ...extra,
});

const envelope = (seq, type, data) => ({ v: 1, seq, ts: 0, type, data });
const settle = async () => {
  for (let i = 0; i < 12; i += 1) await new Promise((resolve) => setImmediate(resolve));
};

function load({ hash = '', stored = null, routes = {}, protocol = 'http:', storageBroken = false } = {}) {
  const log = { requests: [], sockets: [], timeouts: [], history: [] };

  const elements = {};
  const i18nNodes = [];
  for (const match of html.matchAll(/<(\w+)([^>]*)>/g)) {
    const attrs = match[2];
    const id = /\bid="([^"]+)"/.exec(attrs);
    const i18n = /\bdata-i18n="([^"]+)"/.exec(attrs);
    if (!id && !i18n) continue;
    const node = new FakeNode(match[1]);
    const cls = /\bclass="([^"]*)"/.exec(attrs);
    if (cls) node.className = cls[1];
    if (i18n) {
      node.setAttribute('data-i18n', i18n[1]);
      i18nNodes.push(node);
    }
    if (id) elements[id[1]] = node;
  }

  // Like a real document: static elements, plus anything the script attached.
  const lookup = (id) => {
    if (elements[id]) return elements[id];
    for (const node of Object.values(elements)) {
      const found = descendants(node).find((n) => n.attrs.id === id);
      if (found) return found;
    }
    return undefined;
  };
  const byId = new Proxy(elements, { get: (target, id) => lookup(String(id)) });

  const defaultRoutes = {
    'GET /api/state': () => ({ status: 200, body: snapshot() }),
    'GET /api/i18n': () => ({ status: 200, body: english }),
  };
  const table = { ...defaultRoutes, ...routes };

  class FakeWebSocket {
    static OPEN = 1;
    constructor(url) {
      this.url = url;
      this.readyState = 0;
      log.sockets.push(this);
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

  const storage = new Map(stored ? [['stream-spectre-token', stored]] : []);
  const document = {
    getElementById: (id) => lookup(id),
    createElement: (tag) => new FakeNode(tag),
    querySelectorAll: (selector) => (selector === '[data-i18n]' ? i18nNodes : []),
    documentElement: new FakeNode('html'),
    title: '',
    activeElement: null,
  };

  const sandbox = {
    DashboardCore: core,
    URLSearchParams, JSON, Math, Object, String, Number, Promise, encodeURIComponent, isFinite,
    document,
    location: { hash, pathname: '/dashboard/', search: '', protocol, host: '127.0.0.1:8787' },
    history: { replaceState: (...args) => log.history.push(args) },
    WebSocket: FakeWebSocket,
    fetch: (url, init = {}) => {
      const method = init.method || 'GET';
      const headers = { ...(init.headers || {}) };
      const body = init.body ? JSON.parse(init.body) : undefined;
      const request = { method, path: url, headers, body };
      log.requests.push(request);
      const route = table[`${method} ${url}`];
      const answer = typeof route === 'function' ? route(request) : route;
      const status = answer ? answer.status : 404;
      return Promise.resolve({
        ok: status >= 200 && status < 300,
        status,
        text: () => Promise.resolve(answer && answer.body !== undefined ? JSON.stringify(answer.body) : ''),
      });
    },
    setTimeout: (fn, delay) => {
      log.timeouts.push({ fn, delay });
      return log.timeouts.length;
    },
    clearTimeout: () => {},
  };
  sandbox.window = sandbox;
  sandbox.window.sessionStorage = storageBroken
    ? { getItem() { throw new Error('blocked'); }, setItem() { throw new Error('blocked'); } }
    : { getItem: (k) => (storage.has(k) ? storage.get(k) : null), setItem: (k, v) => storage.set(k, v) };
  vm.runInNewContext(source, sandbox);

  return { log, elements: byId, document, storage, table, i18nNodes, socket: () => log.sockets.at(-1) };
}

async function ready(options) {
  const page = load(options);
  await settle();
  page.socket().open();
  return page;
}

// --- starting up ---

test('it loads the state and texts, builds the page and connects', async () => {
  const page = load();
  await settle();
  assert.deepEqual(page.log.requests.slice(0, 2).map((r) => `${r.method} ${r.path}`), ['GET /api/state', 'GET /api/i18n']);
  assert.ok(page.log.requests.every((r) => !('Authorization' in r.headers)));
  assert.ok(page.elements.login.hidden);
  assert.ok(!page.elements.app.hidden);
  assert.equal(page.socket().url, 'ws://127.0.0.1:8787/ws/dashboard');
  assert.equal(page.elements['pending-heading'].textContent, 'Awaiting approval');
  assert.equal(page.document.title, 'Moderation Dashboard');
  assert.equal(page.document.documentElement.lang, 'en');
  assert.equal(page.elements['connection'].textContent, 'Disconnected');
  page.socket().open();
  assert.equal(page.elements['connection'].textContent, 'Connected');
  assert.ok(page.elements['connection'].classes.has('state-ok'));
  assert.equal(page.elements['chat-status'].textContent, 'Connected to TikTok chat @streamer');
});

test('every translated label of the page gets its text', async () => {
  const page = load();
  await settle();
  assert.ok(page.i18nNodes.length >= 10);
  for (const node of page.i18nNodes) {
    assert.equal(node.textContent, english[node.getAttribute('data-i18n')], node.getAttribute('data-i18n'));
  }
});

test('a token in the address bar is kept, removed from the address, and sent', async () => {
  const page = load({ hash: '#token=abc%26d' });
  await settle();
  assert.equal(page.storage.get('stream-spectre-token'), 'abc&d');
  assert.deepEqual(JSON.parse(JSON.stringify(page.log.history)), [[null, '', '/dashboard/']]);
  assert.ok(page.log.requests.every((r) => r.headers.Authorization === 'Bearer abc&d'));
  assert.equal(page.socket().url, 'ws://127.0.0.1:8787/ws/dashboard?token=abc%26d');
});

test('a token stored earlier in the session is used', async () => {
  const page = load({ stored: 'remembered' });
  await settle();
  assert.equal(page.log.requests[0].headers.Authorization, 'Bearer remembered');
});

test('the page works when storage is blocked', async () => {
  const page = load({ hash: '#token=abc', storageBroken: true });
  await settle();
  assert.equal(page.log.requests[0].headers.Authorization, 'Bearer abc');
  assert.ok(!page.elements.app.hidden);
});

test('without access the sign-in form is shown, and a wrong token is refused', async () => {
  const guard = (request) => (request.headers.Authorization === 'Bearer good'
    ? { status: 200, body: snapshot() }
    : { status: 401, body: { error: 'missing or wrong token' } });
  const page = load({ routes: { 'GET /api/state': guard } });
  await settle();
  assert.ok(!page.elements.login.hidden);
  assert.ok(page.elements.app.hidden);
  assert.equal(page.log.sockets.length, 0);
  assert.equal(page.elements['login-error'].textContent, '');

  page.elements['token-input'].value = '  bad ';
  page.elements['login-form'].fire('submit');
  await settle();
  assert.ok(!page.elements.login.hidden);
  assert.equal(page.elements['login-error'].textContent, 'Invalid token');
  assert.equal(page.log.sockets.length, 0);

  page.elements['token-input'].value = 'good';
  page.elements['login-form'].fire('submit');
  await settle();
  assert.ok(page.elements.login.hidden);
  assert.ok(!page.elements.app.hidden);
  assert.equal(page.storage.get('stream-spectre-token'), 'good');
  assert.equal(page.socket().url, 'ws://127.0.0.1:8787/ws/dashboard?token=good');
});

test('an empty sign-in does nothing', async () => {
  const page = load({ routes: { 'GET /api/state': () => ({ status: 401, body: null }) } });
  await settle();
  const before = page.log.requests.length;
  page.elements['token-input'].value = '   ';
  page.elements['login-form'].fire('submit');
  await settle();
  assert.equal(page.log.requests.length, before);
});

test('a server that is down shows an error and tries again', async () => {
  const page = load({ routes: { 'GET /api/state': () => ({ status: 503, body: null }) } });
  await settle();
  assert.ok(!page.elements.error.hidden);
  assert.match(page.elements.error.textContent, /HTTP 503/);
  assert.equal(page.log.timeouts.at(-1).delay, 3000);
});

// --- the queue ---

test('the queue is drawn with titles, requesters and action buttons', async () => {
  const page = await ready();
  const pending = page.elements.pending.children;
  assert.equal(pending.length, 2);
  const text = pending[0].textContent;
  assert.match(text, /Title 7/);
  assert.match(text, /Requested by Viewer 7/);
  assert.match(text, /Channel: Some Channel/);
  assert.match(text, /Duration: 3:33/);
  assert.equal(buttons(pending[0], 'approve').length, 1);
  assert.equal(buttons(pending[0], 'reject').length, 1);
  assert.equal(page.elements.approved.children.length, 1);
  assert.equal(buttons(page.elements.approved, 'approve').length, 0);
  assert.equal(buttons(page.elements.approved, 'reject').length, 1);
  assert.match(page.elements.now.textContent, /Title 3/);
  assert.equal(buttons(page.elements.now, 'skip').length, 1);
});

test('empty lists say so', async () => {
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body: snapshot({ queue: { pending: [], approved: [], now_playing: null } }) }) } });
  assert.equal(page.elements.pending.children[0].textContent, 'The queue is empty');
  assert.equal(page.elements.approved.children[0].textContent, 'The queue is empty');
  assert.equal(page.elements.now.textContent, 'Nothing is playing');
});

test('approve, reject and skip call the API with the item id', async () => {
  const page = await ready({ stored: 'tok' });
  page.table['POST /api/queue/7/approve'] = { status: 204 };
  page.table['POST /api/queue/8/reject'] = { status: 204 };
  page.table['POST /api/queue/9/reject'] = { status: 204 };
  page.table['POST /api/skip'] = { status: 200, body: { skipped: true } };

  buttons(page.elements.pending.children[0], 'approve')[0].fire('click');
  buttons(page.elements.pending.children[1], 'reject')[0].fire('click');
  buttons(page.elements.approved.children[0], 'reject')[0].fire('click');
  buttons(page.elements.now, 'skip')[0].fire('click');
  await settle();

  const posts = page.log.requests.filter((r) => r.method === 'POST');
  assert.deepEqual(posts.map((r) => r.path), ['/api/queue/7/approve', '/api/queue/8/reject', '/api/queue/9/reject', '/api/skip']);
  assert.ok(posts.every((r) => r.headers.Authorization === 'Bearer tok'));
  assert.ok(page.elements.error.hidden);
});

test('a refused action shows the server message', async () => {
  const page = await ready();
  page.table['POST /api/queue/7/approve'] = { status: 409, body: { error: 'entry 7 is not waiting for approval' } };
  buttons(page.elements.pending.children[0], 'approve')[0].fire('click');
  await settle();
  assert.ok(!page.elements.error.hidden);
  assert.equal(page.elements.error.textContent, 'Request failed: entry 7 is not waiting for approval');
  // The next action clears it.
  page.table['POST /api/skip'] = { status: 200, body: { skipped: false } };
  buttons(page.elements.now, 'skip')[0].fire('click');
  assert.ok(page.elements.error.hidden);
});

test('a network failure during an action is reported', async () => {
  const page = await ready();
  page.table['POST /api/skip'] = undefined; // fake fetch answers 404 with no body
  buttons(page.elements.now, 'skip')[0].fire('click');
  await settle();
  assert.equal(page.elements.error.textContent, 'Request failed: HTTP 404');
});

test('an action refused for the token shows the sign-in form', async () => {
  const page = await ready();
  page.table['POST /api/skip'] = { status: 401, body: { error: 'missing or wrong token' } };
  buttons(page.elements.now, 'skip')[0].fire('click');
  await settle();
  assert.ok(!page.elements.login.hidden);
  assert.ok(page.elements.app.hidden);
});

test('text from viewers is shown as text and foreign images are not loaded', async () => {
  const evil = '<img src=x onerror=alert(1)>';
  const bad = item(7, { title: evil, channel: evil, requested_by: { name: evil, role: 'follower' }, thumbnail_url: 'https://evil.example/x.jpg' });
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body: snapshot({ queue: { pending: [bad], approved: [], now_playing: null } }) }) } });
  const card = page.elements.pending.children[0];
  assert.ok(card.textContent.includes(evil));
  assert.equal(descendants(card).filter((n) => n.tag === 'img').length, 0);
});

test('valid thumbnails are shown without a referrer', async () => {
  const page = await ready();
  const images = descendants(page.elements.pending.children[0]).filter((n) => n.tag === 'img');
  assert.equal(images.length, 1);
  assert.equal(images[0].attrs.src, `https://i.ytimg.com/vi/${ITEM_ID}/mqdefault.jpg`);
  assert.equal(images[0].attrs.referrerpolicy, 'no-referrer');
  assert.equal(images[0].attrs.alt, '');
});

// --- live updates ---

test('live events redraw the page', async () => {
  const page = await ready();
  page.socket().receive(envelope(6, 'queue_updated', { pending: [item(20)], approved: [], now_playing: null }));
  assert.equal(page.elements.pending.children.length, 1);
  assert.match(page.elements.pending.children[0].textContent, /Title 20/);
  assert.equal(page.elements.now.textContent, 'Nothing is playing');

  page.socket().receive(envelope(7, 'chat_status', { state: 'reconnecting', channel: 'streamer', text: 'Reconnecting to chat…' }));
  assert.equal(page.elements['chat-status'].textContent, 'Reconnecting to chat…');
  assert.ok(page.elements['chat-status'].classes.has('state-bad'));

  page.socket().receive(envelope(8, 'volume', { volume: 30 }));
  assert.equal(page.elements.volume.value, '30');
  assert.equal(page.elements['volume-value'].textContent, '30%');
});

test('events already covered by the snapshot are ignored', async () => {
  const page = await ready();
  page.socket().receive(envelope(5, 'queue_updated', { pending: [], approved: [], now_playing: null }));
  page.socket().receive(envelope(4, 'volume', { volume: 1 }));
  assert.equal(page.elements.pending.children.length, 2);
  assert.equal(page.elements.volume.value, '50');
});

test('a new hello replaces the whole view', async () => {
  const page = await ready();
  page.socket().receive(envelope(50, 'hello', snapshot({ seq: 50, queue: { pending: [], approved: [], now_playing: item(99) } })));
  assert.equal(page.elements.pending.children[0].textContent, 'The queue is empty');
  assert.match(page.elements.now.textContent, /Title 99/);
});

test('notices are listed newest first with the viewer they concern', async () => {
  const page = await ready();
  assert.ok(!page.elements['notices-empty'].hidden);
  page.socket().receive(envelope(6, 'notice', { level: 'warning', code: 'reject.not_found', text: 'Video not found', sender: 'Budi' }));
  page.socket().receive(envelope(7, 'notice', { level: 'error', code: 'x', text: 'Something broke', sender: null }));
  const [newest, older] = page.elements.notices.children;
  assert.equal(newest.textContent, 'Something broke');
  assert.ok(newest.classes.has('notice-error'));
  assert.equal(older.textContent, 'Budi: Video not found');
  assert.ok(older.classes.has('notice-warning'));
  assert.ok(page.elements['notices-empty'].hidden);
});

test('malformed socket messages are ignored', async () => {
  const page = await ready();
  for (const junk of ['not json', '', '[]', 'null', '{"v":1}']) {
    assert.doesNotThrow(() => page.socket().receive(junk), junk);
  }
  assert.equal(page.elements.pending.children.length, 2);
});

test('after a dropped connection it checks access, then reconnects with a growing delay', async () => {
  const page = await ready();
  const delays = [];
  for (let i = 0; i < 4; i += 1) {
    page.socket().close();
    assert.equal(page.elements.connection.textContent, 'Reconnecting…');
    await settle();
    delays.push(page.log.timeouts.at(-1).delay);
    page.log.timeouts.at(-1).fn();
  }
  assert.deepEqual(delays, [1000, 2000, 4000, 8000]);
  assert.equal(page.log.sockets.length, 5);
  assert.ok(page.log.requests.filter((r) => r.path === '/api/state').length >= 5);
});

test('a refused token after a drop shows the sign-in form instead of retrying', async () => {
  const page = await ready();
  const timeouts = page.log.timeouts.length;
  page.table['GET /api/state'] = { status: 401, body: null };
  page.socket().close();
  await settle();
  assert.ok(!page.elements.login.hidden);
  assert.equal(page.log.timeouts.length, timeouts);
});

// --- forms ---

test('the settings form lists every setting with the current values', async () => {
  const page = await ready();
  for (const field of core.SETTINGS_FIELDS) {
    assert.ok(page.elements[`setting-${field.key}`], field.key);
  }
  const m = snapshot().settings.moderation;
  assert.equal(page.elements['setting-mode'].value, 'manual');
  assert.equal(page.elements['setting-min_role'].value, 'follower');
  assert.equal(page.elements['setting-cooldown_secs'].value, String(m.cooldown_secs));
  assert.equal(page.elements['setting-max_volume'].value, '80');
  const mode = descendants(page.elements['settings-form']).find((n) => n.attrs.id === 'setting-mode');
  assert.deepEqual(mode.children.map((o) => o.textContent), [english['dashboard.settings.mode.manual'], english['dashboard.settings.mode.auto']]);
});

test('saving settings sends only what changed', async () => {
  const page = await ready();
  const updated = snapshot();
  updated.settings.moderation.mode = 'auto';
  updated.settings.moderation.cooldown_secs = 30;
  page.table['PATCH /api/settings'] = { status: 200, body: updated };

  page.elements['setting-mode'].value = 'auto';
  page.elements['setting-cooldown_secs'].value = '30';
  page.elements['settings-form'].fire('submit');
  await settle();

  const patch = page.log.requests.find((r) => r.method === 'PATCH');
  assert.deepEqual(patch.body, { mode: 'auto', cooldown_secs: 30 });
  assert.equal(page.elements.flash.textContent, 'Settings saved');
  assert.ok(!page.elements.flash.hidden);
  assert.ok(page.elements.error.hidden);
});

test('saving unchanged settings sends nothing', async () => {
  const page = await ready();
  page.elements['settings-form'].fire('submit');
  await settle();
  assert.equal(page.log.requests.filter((r) => r.method === 'PATCH').length, 0);
  assert.equal(page.elements.flash.textContent, 'Settings saved');
});

test('an unacceptable number is refused before anything is sent', async () => {
  const page = await ready();
  page.elements['setting-cooldown_secs'].value = '1e2';
  page.elements['settings-form'].fire('submit');
  await settle();
  assert.equal(page.log.requests.filter((r) => r.method === 'PATCH').length, 0);
  assert.equal(page.elements.error.textContent, 'Invalid value for Per-user cooldown (seconds)');
});

test('the server refusing settings is shown and nothing is saved', async () => {
  const page = await ready();
  page.table['PATCH /api/settings'] = { status: 400, body: { error: 'invalid config value `moderation.max_queue_len`: must be greater than 0' } };
  page.elements['setting-max_queue_len'].value = '0';
  page.elements['settings-form'].fire('submit');
  await settle();
  assert.match(page.elements.error.textContent, /must be greater than 0/);
  assert.ok(page.elements.flash.hidden);
});

test('a field being edited is not overwritten by a live update', async () => {
  const page = await ready();
  const next = snapshot().settings;
  next.moderation.cooldown_secs = 15;
  next.moderation.max_queue_len = 7;

  page.document.activeElement = page.elements['setting-cooldown_secs'];
  page.elements['setting-cooldown_secs'].value = '99';
  page.socket().receive(envelope(6, 'settings_updated', next));
  assert.equal(page.elements['setting-cooldown_secs'].value, '99');
  assert.equal(page.elements['setting-max_queue_len'].value, '7');
});

test('the blacklist form shows the lists and saves them cleaned up', async () => {
  const page = await ready();
  assert.equal(page.elements['blacklist-video_ids'].value, 'aaaaaaaaaaa');
  assert.equal(page.elements['blacklist-channels'].value, '');
  const updated = snapshot();
  updated.settings.blacklist.channels = ['Bad', 'Other'];
  page.table['PATCH /api/blacklist'] = { status: 200, body: updated };

  page.elements['blacklist-channels'].value = 'Bad  \n\nBad\n Other ';
  page.elements['blacklist-form'].fire('submit');
  await settle();

  const patch = page.log.requests.find((r) => r.path === '/api/blacklist');
  assert.deepEqual(patch.body, { video_ids: ['aaaaaaaaaaa'], channels: ['Bad', 'Other'], title_words: [] });
  assert.equal(page.elements.flash.textContent, 'Blacklist saved');
  assert.equal(page.elements['blacklist-channels'].value, 'Bad\nOther');
});

test('the volume slider follows the maximum and sends what was chosen', async () => {
  const page = await ready();
  assert.equal(page.elements.volume.max, '80');
  assert.equal(page.elements.volume.value, '50');
  page.table['POST /api/volume'] = { status: 200, body: { volume: 30 } };

  page.elements.volume.value = '33';
  page.elements.volume.fire('input');
  assert.equal(page.elements['volume-value'].textContent, '33%');
  page.elements.volume.fire('change');
  await settle();

  const post = page.log.requests.find((r) => r.path === '/api/volume');
  assert.deepEqual(post.body, { volume: 33 });
  assert.equal(page.elements.volume.value, '30', 'shows the volume the server settled on');
  assert.equal(page.elements['volume-value'].textContent, '30%');
});

// --- keeping the page and its texts in step ---

test('every id the script looks up exists in the page', () => {
  const ids = new Set([...html.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]));
  const wanted = [...source.matchAll(/\$\('([^']+)'\)/g)].map((m) => m[1]);
  assert.ok(wanted.length > 15);
  for (const id of wanted) assert.ok(ids.has(id), `missing #${id} in index.html`);
});

test('every text the page uses exists in both languages', () => {
  const indonesian = JSON.parse(fs.readFileSync(path.join(root, 'locales/id.json'), 'utf8'));
  const keys = new Set();
  for (const text of [source, fs.readFileSync(path.join(root, 'static/dashboard/dashboard-core.js'), 'utf8')]) {
    for (const m of text.matchAll(/'((?:dashboard|role)\.[a-z_.]*[a-z_])'/g)) keys.add(m[1]);
  }
  for (const m of html.matchAll(/data-i18n="([^"]+)"/g)) keys.add(m[1]);
  for (const state of ['connected', 'disconnected', 'reconnecting']) keys.add(`dashboard.connection.${state}`);
  assert.ok(keys.size > 30, `found only ${keys.size} keys`);
  for (const key of keys) {
    assert.ok(key in english, `en.json lacks ${key}`);
    assert.ok(key in indonesian, `id.json lacks ${key}`);
  }
});
