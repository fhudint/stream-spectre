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
    this.checked = false;
    this.scrollTop = 0;
    this.scrollHeight = 0;
    this.clientHeight = 0;
  }
  get firstChild() {
    return this.children[0] || null;
  }
  removeChild(child) {
    this.children = this.children.filter((c) => c !== child);
    return child;
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
  fire(type, event = {}) {
    for (const listener of this.listeners[type] || []) listener({ preventDefault() {}, ...event });
  }
  focus() {
    this.focused = true;
  }
  select() {
    this.selected = true;
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
    dashboard: { chat_history: 20 },
    chat: { tiktok: { username: 'streamer' } },
  },
  chat_log: [],
  volume: 50,
  queue: { pending: [item(7), item(8)], approved: [item(9)], now_playing: item(3) },
  chat: { state: 'connected', channel: 'streamer', text: 'Connected to TikTok chat @streamer' },
  ...extra,
});

const envelope = (seq, type, data) => ({ v: 1, seq, ts: 0, type, data });
const settle = async () => {
  for (let i = 0; i < 12; i += 1) await new Promise((resolve) => setImmediate(resolve));
};

function load({
  hash = '',
  stored = null,
  routes = {},
  protocol = 'http:',
  storageBroken = false,
  clipboard = true,
  execCommandResult = true,
  clock = 1_000_000,
} = {}) {
  let now = clock;
  const log = { requests: [], sockets: [], timeouts: [], intervals: [], history: [], copied: [], documentListeners: {}, windowListeners: {} };

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
    addEventListener: (type, listener) => (log.documentListeners[type] ||= []).push(listener),
    body: { appendChild: (node) => node, removeChild: (node) => node },
    execCommand: (name) => {
      log.copied.push(name);
      return execCommandResult;
    },
    documentElement: new FakeNode('html'),
    title: '',
    activeElement: null,
  };

  const location = { hash, pathname: '/dashboard/', search: '', protocol, host: '127.0.0.1:8787' };
  const sandbox = {
    DashboardCore: core,
    URLSearchParams, JSON, Math, Object, String, Number, Promise, encodeURIComponent, isFinite,
    Date: { now: () => now },
    document,
    location,
    navigator: clipboard
      ? { clipboard: { writeText: (text) => { log.clipboard = text; return Promise.resolve(); } } }
      : {},
    setInterval: (fn, delay) => {
      log.intervals.push({ fn, delay });
      return log.intervals.length;
    },
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
  sandbox.addEventListener = (type, listener) => (log.windowListeners[type] ||= []).push(listener);
  sandbox.window.sessionStorage = storageBroken
    ? { getItem() { throw new Error('blocked'); }, setItem() { throw new Error('blocked'); } }
    : { getItem: (k) => (storage.has(k) ? storage.get(k) : null), setItem: (k, v) => storage.set(k, v) };
  vm.runInNewContext(source, sandbox);

  return {
    log, elements: byId, document, storage, table, i18nNodes, location,
    socket: () => log.sockets.at(-1),
    setClock: (value) => { now = value; },
    go: (nextHash) => {
      location.hash = nextHash;
      for (const listener of log.windowListeners.hashchange || []) listener({});
    },
    press: (key, extra = {}) => {
      let prevented = false;
      for (const listener of log.documentListeners.keydown || []) {
        listener({ key, target: { tagName: 'BODY' }, preventDefault: () => { prevented = true; }, ...extra });
      }
      return prevented;
    },
  };
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
  assert.match(text, /Viewer 7/);
  assert.match(text, /Follower/);
  assert.match(text, /Some Channel/);
  assert.match(text, new RegExp(ITEM_ID));
  assert.match(text, /3:33/);
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

// --- chat ---

const line = (text, extra = {}) => ({ name: 'Budi', role: 'follower', text, command: false, skipped: 0, ...extra });
const chatItems = (page) => page.elements['chat-log'].children;

// A line as one string: "Name (Role): text"; the skipped-lines note as it is.
function readable(li) {
  if (li.classes.has('chat-skipped')) return li.textContent;
  const part = (name) => descendants(li).find((n) => n.classes.has(name));
  const role = part('chat-role');
  return `${part('chat-name').textContent}${role ? ` (${role.textContent})` : ''}: ${part('chat-text').textContent}`;
}

test('the chat shows the history the server sends, commands marked', async () => {
  const log = [line('hello there'), line('!v dQw4w9WgXcQ', { command: true, role: 'everyone', name: 'Sari' })];
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body: snapshot({ chat_log: log }) }) } });
  const items = chatItems(page);
  assert.deepEqual(items.map(readable), ['Budi (Follower): hello there', 'Sari: !v dQw4w9WgXcQ']);
  assert.ok(!items[0].classes.has('chat-command'));
  assert.ok(items[1].classes.has('chat-command'));
  assert.ok(page.elements['chat-empty'].hidden);
});

test('an empty chat says so', async () => {
  const page = await ready();
  assert.equal(chatItems(page).length, 0);
  assert.ok(!page.elements['chat-empty'].hidden);
  page.socket().receive(envelope(6, 'chat_message', line('first')));
  assert.ok(page.elements['chat-empty'].hidden);
});

test('a new message is added without redrawing the lines already there', async () => {
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body: snapshot({ chat_log: [line('one')] }) }) } });
  const first = chatItems(page)[0];
  page.socket().receive(envelope(6, 'chat_message', line('two')));
  page.socket().receive(envelope(7, 'volume', { volume: 20 }));
  page.socket().receive(envelope(8, 'chat_message', line('three', { skipped: 4 })));
  const items = chatItems(page);
  assert.equal(items[0], first, 'the first line is the very same element');
  assert.deepEqual(items.map(readable), [
    'Budi (Follower): one',
    'Budi (Follower): two',
    '4 messages not shown (chat too fast)',
    'Budi (Follower): three',
  ]);
  assert.ok(items[2].classes.has('chat-skipped'));
});

test('text and names from chat are shown as text', async () => {
  const page = await ready();
  const nasty = '<img src=x onerror=alert(1)>';
  page.socket().receive(envelope(6, 'chat_message', line(nasty, { name: '<b>Eve</b>' })));
  const item = chatItems(page)[0];
  assert.equal(readable(item), `<b>Eve</b> (Follower): ${nasty}`);
  assert.ok(descendants(item).every((n) => !['img', 'b', 'script'].includes(n.tag)));
});

test('the filter shows only video requests and brings the rest back', async () => {
  const page = await ready();
  page.socket().receive(envelope(6, 'chat_message', line('just chatting')));
  page.socket().receive(envelope(7, 'chat_message', line('!v dQw4w9WgXcQ', { command: true })));
  page.socket().receive(envelope(8, 'chat_message', line('lol')));
  assert.equal(chatItems(page).length, 3);

  page.elements['chat-commands-only'].checked = true;
  page.elements['chat-commands-only'].fire('change');
  assert.deepEqual(chatItems(page).map(readable), ['Budi (Follower): !v dQw4w9WgXcQ']);

  page.socket().receive(envelope(9, 'chat_message', line('more chatting')));
  page.socket().receive(envelope(10, 'chat_message', line('!v jNQXAC9IVRw', { command: true })));
  assert.equal(chatItems(page).length, 2, 'new lines obey the filter too');

  page.elements['chat-commands-only'].checked = false;
  page.elements['chat-commands-only'].fire('change');
  assert.equal(chatItems(page).length, 5);
});

test('the lines are capped, dropping the oldest', async () => {
  const page = await ready();
  for (let i = 0; i < core.MAX_CHAT_LINES + 10; i += 1) {
    page.socket().receive(envelope(6 + i, 'chat_message', line(`m${i}`)));
  }
  const items = chatItems(page);
  assert.equal(items.length, core.MAX_CHAT_LINES);
  assert.equal(readable(items[0]), 'Budi (Follower): m10');
  assert.equal(readable(items.at(-1)), `Budi (Follower): m${core.MAX_CHAT_LINES + 9}`);
});

test('the view follows new lines, unless the moderator scrolled up to read', async () => {
  const page = await ready();
  const box = page.elements['chat-log'];
  box.scrollHeight = 1000;
  box.clientHeight = 300;

  box.scrollTop = 700; // at the bottom
  page.socket().receive(envelope(6, 'chat_message', line('a')));
  assert.equal(box.scrollTop, 1000, 'moved to the newest line');

  box.scrollTop = 100; // reading older lines
  page.socket().receive(envelope(7, 'chat_message', line('b')));
  assert.equal(box.scrollTop, 100, 'left alone');
});

test('a new hello replaces the lines, and so does a dropped connection coming back', async () => {
  const page = await ready();
  page.socket().receive(envelope(6, 'chat_message', line('live one')));
  page.socket().receive(envelope(7, 'chat_message', line('live two')));
  assert.equal(chatItems(page).length, 2);

  const fresh = snapshot({ seq: 20, chat_log: [line('from the server')] });
  page.socket().receive(envelope(20, 'hello', fresh));
  assert.deepEqual(chatItems(page).map(readable), ['Budi (Follower): from the server']);
});

test('saving other settings does not cut the chat down to the remembered lines', async () => {
  const page = await ready();
  for (let i = 0; i < 5; i += 1) page.socket().receive(envelope(6 + i, 'chat_message', line(`m${i}`)));
  const updated = snapshot({ chat_log: [line('only this one')] });
  updated.settings.blacklist.channels = ['Bad'];
  page.table['PATCH /api/blacklist'] = { status: 200, body: updated };

  page.elements['blacklist-channels'].value = 'Bad';
  page.elements['blacklist-form'].fire('submit');
  await settle();

  assert.equal(page.elements['blacklist-channels'].value, 'Bad');
  assert.equal(chatItems(page).length, 5);
  assert.equal(readable(chatItems(page)[0]), 'Budi (Follower): m0');
});

test('the history size is shown, and saving sends only a changed, valid number', async () => {
  const page = await ready();
  assert.equal(page.elements['chat-history'].value, '20');

  page.elements['chat-form'].fire('submit');
  await settle();
  assert.ok(!page.log.requests.some((r) => r.path === '/api/dashboard'), 'unchanged: nothing sent');
  assert.equal(page.elements.flash.textContent, 'Chat setting saved');

  for (const bad of ['', 'many', '-1', '1e2', '201', '1.5']) {
    page.elements['chat-history'].value = bad;
    page.elements['chat-form'].fire('submit');
    await settle();
    assert.ok(!page.log.requests.some((r) => r.path === '/api/dashboard'), `${bad}: nothing sent`);
    assert.match(page.elements.error.textContent, /Invalid value for Messages kept/, bad);
  }

  const updated = snapshot();
  updated.settings.dashboard.chat_history = 5;
  page.table['PATCH /api/dashboard'] = { status: 200, body: updated };
  page.elements['chat-history'].value = ' 5 ';
  page.elements['chat-form'].fire('submit');
  await settle();
  const patch = page.log.requests.find((r) => r.path === '/api/dashboard');
  assert.equal(patch.method, 'PATCH');
  assert.deepEqual(patch.body, { chat_history: 5 });
  assert.equal(page.elements['chat-history'].value, '5');
});

test('the server refusing the history size is shown', async () => {
  const page = await ready();
  page.table['PATCH /api/dashboard'] = { status: 400, body: { error: 'dashboard.chat_history must be at most 200' } };
  page.elements['chat-history'].value = '30';
  page.elements['chat-form'].fire('submit');
  await settle();
  assert.match(page.elements.error.textContent, /must be at most 200/);
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

// --- the pages of the menu ---

const VIEWS = ['queue', 'blacklist', 'rules', 'overlay', 'settings'];
const shown = (page) => VIEWS.filter((v) => !page.elements[`view-${v}`].hidden);
const current = (page, prefix) => VIEWS.filter((v) => page.elements[`${prefix}-${v}`].attrs['aria-current'] === 'page');

test('the queue page is shown first, and both menus mark it', async () => {
  const page = await ready();
  assert.deepEqual(shown(page), ['queue']);
  assert.deepEqual(current(page, 'nav'), ['queue']);
  assert.deepEqual(current(page, 'bnav'), ['queue']);
  assert.ok(!page.elements['queue-heading'].focused, 'opening the page does not steal the focus');
});

test('the address bar chooses the page, also when it changes later', async () => {
  const page = await ready({ hash: '#blacklist' });
  assert.deepEqual(shown(page), ['blacklist']);
  assert.deepEqual(current(page, 'nav'), ['blacklist']);

  page.go('#overlay');
  assert.deepEqual(shown(page), ['overlay']);
  assert.deepEqual(current(page, 'bnav'), ['overlay']);
  assert.ok(page.elements['overlay-heading'].focused, 'the keyboard focus follows to the new page');

  page.go('#rules');
  assert.deepEqual(shown(page), ['rules']);
  assert.ok(page.elements['settings-heading'].focused);
  page.go('#settings');
  assert.ok(page.elements['chat-settings-heading'].focused);
  page.go('#nothing-like-this');
  assert.deepEqual(shown(page), ['queue']);
  assert.ok(page.elements['queue-heading'].focused);
});

test('a token in the address is not taken for a page', async () => {
  const page = await ready({ hash: '#token=abc' });
  assert.deepEqual(shown(page), ['queue']);
});

test('the phone tabs choose what the queue page shows, and both tab bars agree', async () => {
  const page = await ready();
  const pressed = () => ['tab-queue', 'tab-chat', 'tab-notices', 'rail-chat', 'rail-notices']
    .filter((id) => page.elements[id].attrs['aria-pressed'] === 'true');
  assert.equal(page.elements['view-queue'].attrs['data-pane'], 'queue');
  assert.deepEqual(pressed(), ['tab-queue', 'rail-chat'], 'a wide screen shows the chat beside the queue');

  page.elements['tab-chat'].fire('click');
  assert.equal(page.elements['view-queue'].attrs['data-pane'], 'chat');
  assert.deepEqual(pressed(), ['tab-chat', 'rail-chat']);

  page.elements['rail-notices'].fire('click');
  assert.equal(page.elements['view-queue'].attrs['data-pane'], 'notices');
  assert.deepEqual(pressed(), ['tab-notices', 'rail-notices']);

  page.elements['tab-queue'].fire('click');
  assert.equal(page.elements['view-queue'].attrs['data-pane'], 'queue');
});

// --- the queue page: summary, counters, rules, channel ---

test('the summary counts the waiting requests and names the longest wait', async () => {
  const waited = (seconds) => item(7, { requested_at_ms: 1_000_000 - seconds * 1000 });
  const body = snapshot({ queue: { pending: [waited(60), waited(252)], approved: [], now_playing: null } });
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body }) } });
  assert.equal(page.elements['queue-summary'].textContent, '2 waiting for approval, the oldest for 4 min 12 s.');
  assert.equal(page.elements['nav-queue-count'].textContent, '2');
  assert.equal(page.elements['tab-queue-count'].textContent, '2');
  assert.equal(page.elements['queue-count'].textContent, '2');

  const none = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body: snapshot({ queue: { pending: [], approved: [], now_playing: null } }) }) } });
  assert.equal(none.elements['queue-summary'].textContent, 'No requests are waiting.');
  assert.equal(none.elements['nav-queue-count'].textContent, '0');

  const unknown = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body: snapshot({ queue: { pending: [item(7)], approved: [], now_playing: null } }) }) } });
  assert.equal(unknown.elements['queue-summary'].textContent, '1 waiting for approval.', 'no time known, none claimed');
});

test('waiting times count up each second and follow the server clock, not the browser clock', async () => {
  const body = snapshot({ queue: { pending: [item(7, { requested_at_ms: 5_000_000 })], approved: [], now_playing: null } });
  const page = await ready({ clock: 123, routes: { 'GET /api/state': () => ({ status: 200, body }) } });
  const label = () => page.elements.pending.children[0].textContent;

  // The browser clock is far off; the server says it is 5 000 060 000.
  page.socket().receive({ v: 1, seq: 6, ts: 5_060_000, type: 'volume', data: { volume: 50 } });
  assert.match(label(), /Follower · waiting 1 min 0 s/);
  assert.match(page.elements['queue-summary'].textContent, /the oldest for 1 min 0 s/);

  page.setClock(123 + 5_000);
  page.log.intervals[0].fn();
  assert.match(label(), /waiting 1 min 5 s/);
  assert.match(page.elements['queue-summary'].textContent, /1 min 5 s/);
  assert.deepEqual(page.log.intervals.map((i) => i.delay), [1000]);
});

test('the session counters come from the server and the waiting count from the queue', async () => {
  const body = snapshot({ stats: { since_ms: new Date(2026, 0, 5, 19, 2, 40).getTime(), received: 30, approved: 15, rejected: 9 } });
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body }) } });
  const read = () => ['received', 'approved', 'rejected', 'waiting'].map((k) => page.elements[`stat-${k}`].textContent);
  assert.deepEqual(read(), ['30', '15', '9', '2']);
  assert.equal(page.elements['today-since'].textContent, 'Since the program started at 19:02');

  page.socket().receive(envelope(6, 'stats_updated', { since_ms: 1, received: 31, approved: 15, rejected: 9 }));
  page.socket().receive(envelope(7, 'queue_updated', { pending: [], approved: [], now_playing: null }));
  assert.deepEqual(read(), ['31', '15', '9', '0']);
});

test('without a start time the session line is left empty', async () => {
  const page = await ready();
  assert.equal(page.elements['today-since'].textContent, '');
});

test('the active rules are listed from the settings and follow changes', async () => {
  const page = await ready();
  const chips = () => page.elements['rules-chips'].children.map((c) => c.textContent);
  assert.deepEqual(chips(), ['Follower and above', '60 s cooldown', 'Max. 10:00', 'Queue of 20', '1 blocked']);

  const next = snapshot().settings;
  next.moderation.mode = 'auto';
  next.moderation.min_role = 'everyone';
  next.moderation.cooldown_secs = 0;
  next.blacklist = { video_ids: [], channels: [], title_words: [] };
  page.socket().receive(envelope(6, 'settings_updated', next));
  assert.deepEqual(chips(), ['Anyone may submit', 'Automatic approval', 'No cooldown', 'Max. 10:00', 'Queue of 20']);
});

test('the channel is shown in the top bar, with its first letter', async () => {
  const page = await ready();
  assert.equal(page.elements['channel-name'].textContent, 'streamer');
  assert.equal(page.elements['channel-initial'].textContent, 'S');
});

// --- the card for the video that is playing ---

test('the playing card shows the picture, who asked, and how far the video has got', async () => {
  const page = await ready();
  const card = page.elements.now;
  const text = card.textContent;
  assert.match(text, /Viewer 3 requested this video/);
  assert.match(text, /Title 3/);
  assert.match(text, /Some Channel/);
  assert.match(text, /Requested by Viewer 3 \(Follower\)/);
  assert.match(text, /0:00 of 3:33/);
  assert.match(text, /3 min 33 s left/);
  const bar = descendants(card).find((n) => n.tag === 'progress');
  assert.equal(bar.attrs.max, '213');
  assert.equal(bar.attrs.value, '0');
  const images = descendants(card).filter((n) => n.tag === 'img');
  assert.equal(images.length, 1);
  assert.equal(images[0].attrs.referrerpolicy, 'no-referrer');

  page.socket().receive(envelope(6, 'playback', { item_id: 3, position_secs: 65 }));
  assert.equal(bar.attrs.value, '65', 'the same bar is updated, not rebuilt');
  assert.match(card.textContent, /1:05 of 3:33/);
  assert.match(card.textContent, /2 min 28 s left/);
  assert.equal(descendants(card).find((n) => n.tag === 'img'), images[0], 'the picture is not loaded again');
});

test('a report for another video does not move the bar, and a new video starts at zero', async () => {
  const page = await ready();
  page.socket().receive(envelope(6, 'playback', { item_id: 99, position_secs: 100 }));
  const bar = () => descendants(page.elements.now).find((n) => n.tag === 'progress');
  assert.equal(bar().attrs.value, '0');

  page.socket().receive(envelope(7, 'playback', { item_id: 3, position_secs: 100 }));
  assert.equal(bar().attrs.value, '100');
  page.socket().receive(envelope(8, 'queue_updated', { pending: [], approved: [], now_playing: item(4, { duration_secs: 60 }) }));
  assert.equal(bar().attrs.value, '0');
  assert.equal(bar().attrs.max, '60');
  assert.match(page.elements.now.textContent, /Title 4/);
});

test('a position that is past the end is shown as the end', async () => {
  const page = await ready();
  page.socket().receive(envelope(6, 'playback', { item_id: 3, position_secs: 99999 }));
  assert.match(page.elements.now.textContent, /3:33 of 3:33/);
  assert.match(page.elements.now.textContent, /0 s left/);
});

test('saving a setting does not make the bar jump back to zero', async () => {
  const page = await ready();
  page.socket().receive(envelope(6, 'playback', { item_id: 3, position_secs: 80 }));
  const updated = snapshot();
  updated.settings.blacklist.channels = ['Bad'];
  page.table['PATCH /api/blacklist'] = { status: 200, body: updated };
  page.elements['blacklist-channels'].value = 'Bad';
  page.elements['blacklist-form'].fire('submit');
  await settle();
  assert.match(page.elements.now.textContent, /1:20 of 3:33/);
});

test('the next line names the first approved video, or says there is none', async () => {
  const page = await ready();
  assert.match(page.elements.now.textContent, /Next: Title 9/);
  page.socket().receive(envelope(6, 'queue_updated', { pending: [], approved: [], now_playing: item(3) }));
  assert.match(page.elements.now.textContent, /Next: nothing approved yet/);
});

test('nothing playing shows a plain message and drops the progress', async () => {
  const page = await ready();
  page.socket().receive(envelope(6, 'queue_updated', { pending: [], approved: [], now_playing: null }));
  assert.equal(page.elements.now.textContent, 'Nothing is playing');
  assert.equal(descendants(page.elements.now).filter((n) => n.tag === 'progress').length, 0);
});

// --- the keyboard ---

const posts = (page) => page.log.requests.filter((r) => r.method === 'POST').map((r) => r.path);
const selectedTitle = (page) => {
  const rows = page.elements.pending.children.filter((li) => li.classes.has('selected'));
  assert.ok(rows.length <= 1, 'at most one row is selected');
  return rows.length ? rows[0].textContent : null;
};

test('the first waiting request is selected, and the arrow keys move the choice', async () => {
  const page = await ready();
  assert.match(selectedTitle(page), /Title 7/);
  assert.equal(page.elements.pending.children[0].attrs['aria-current'], 'true');

  assert.equal(page.press('ArrowDown'), true, 'the page does not scroll as well');
  assert.match(selectedTitle(page), /Title 8/);
  page.press('ArrowDown');
  assert.match(selectedTitle(page), /Title 8/, 'stops at the last one');
  page.press('ArrowUp');
  page.press('ArrowUp');
  assert.match(selectedTitle(page), /Title 7/, 'and at the first one');
});

test('A approves, R rejects and S skips, for the chosen request', async () => {
  const page = await ready();
  page.table['POST /api/queue/8/approve'] = { status: 204 };
  page.table['POST /api/queue/7/reject'] = { status: 204 };
  page.table['POST /api/skip'] = { status: 200, body: { skipped: true } };

  page.press('r');
  page.press('ArrowDown');
  page.press('A');
  page.press('S');
  await settle();
  assert.deepEqual(posts(page), ['/api/queue/7/reject', '/api/queue/8/approve', '/api/skip']);
});

test('the choice stays on its request, and moves on when that request is gone', async () => {
  const page = await ready();
  page.press('ArrowDown'); // Title 8
  page.socket().receive(envelope(6, 'queue_updated', { pending: [item(6), item(7), item(8), item(10)], approved: [], now_playing: null }));
  assert.match(selectedTitle(page), /Title 8/, 'a new request arriving does not move the choice');

  page.socket().receive(envelope(7, 'queue_updated', { pending: [item(6), item(7), item(10)], approved: [], now_playing: null }));
  assert.match(selectedTitle(page), /Title 10/, 'the request that took its place');
  page.socket().receive(envelope(8, 'queue_updated', { pending: [item(6), item(7)], approved: [], now_playing: null }));
  assert.match(selectedTitle(page), /Title 7/, 'the last one when the end was removed');
  page.socket().receive(envelope(9, 'queue_updated', { pending: [], approved: [], now_playing: null }));
  assert.equal(selectedTitle(page), null);
  page.press('a');
  page.press('r');
  await settle();
  assert.deepEqual(posts(page), [], 'nothing is chosen, so nothing is sent');
});

test('clicking a request chooses it, and the buttons still work', async () => {
  const page = await ready();
  page.elements.pending.children[1].fire('click');
  assert.match(selectedTitle(page), /Title 8/);
  page.table['POST /api/queue/8/approve'] = { status: 204 };
  page.press('a');
  await settle();
  assert.deepEqual(posts(page), ['/api/queue/8/approve']);
});

test('clicking a row of the play queue does not move the choice among the waiting requests', async () => {
  const page = await ready();
  page.press('ArrowDown'); // Title 8
  page.elements.approved.children[0].fire('click');
  assert.match(selectedTitle(page), /Title 8/);
  assert.equal(page.elements.approved.children[0].classes.has('selected'), false);
});

test('shortcuts are ignored while typing, with Ctrl, in the chat list and on other pages', async () => {
  const page = await ready();
  page.table['POST /api/skip'] = { status: 200, body: { skipped: true } };
  for (const tagName of ['INPUT', 'TEXTAREA', 'SELECT']) {
    assert.equal(page.press('s', { target: { tagName } }), false, tagName);
  }
  assert.equal(page.press('s', { target: { tagName: 'DIV', isContentEditable: true } }), false);
  assert.equal(page.press('s', { ctrlKey: true }), false);
  assert.equal(page.press('ArrowDown', { target: page.elements['chat-log'] }), false);
  assert.equal(page.press('x'), false);
  page.go('#rules');
  page.press('s');
  page.go('#queue');
  await settle();
  assert.deepEqual(posts(page), []);

  page.press('s');
  await settle();
  assert.deepEqual(posts(page), ['/api/skip'], 'and it works again on the queue page');
});

test('shortcuts do nothing while the sign-in form is shown', async () => {
  const page = await ready();
  page.table['POST /api/skip'] = { status: 401, body: { error: 'missing or wrong token' } };
  buttons(page.elements.now, 'skip')[0].fire('click');
  await settle();
  assert.ok(!page.elements.login.hidden);
  const before = page.log.requests.length;
  page.press('s');
  page.press('ArrowDown');
  await settle();
  assert.equal(page.log.requests.length, before);
});

test('S does nothing when no video is playing', async () => {
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body: snapshot({ queue: { pending: [item(7)], approved: [], now_playing: null } }) }) } });
  assert.equal(page.press('s'), false);
  await settle();
  assert.deepEqual(posts(page), []);
});

// --- copying the overlay address ---

test('the overlay address is shown without the token', async () => {
  const page = await ready({ stored: 'secret-token' });
  assert.equal(page.elements['overlay-address'].textContent, 'localhost:8787/overlay/');
  assert.equal(page.elements['overlay-shown'].textContent, 'localhost:8787/overlay/');
});

test('copying gives OBS the address with localhost and the token', async () => {
  const page = await ready({ stored: 'tok en' });
  page.elements['copy-overlay'].fire('click');
  await settle();
  assert.equal(page.log.clipboard, 'http://localhost:8787/overlay/?token=tok%20en');
  assert.equal(page.elements.flash.textContent, 'Overlay URL copied');
  assert.ok(!page.elements.flash.hidden);

  page.log.clipboard = null;
  page.elements['copy-overlay-page'].fire('click');
  await settle();
  assert.equal(page.log.clipboard, 'http://localhost:8787/overlay/?token=tok%20en');
});

test('without a token the copied address has no query', async () => {
  const page = await ready();
  page.elements['copy-overlay'].fire('click');
  await settle();
  assert.equal(page.log.clipboard, 'http://localhost:8787/overlay/');
});

test('where the clipboard is not available the old way of copying is tried', async () => {
  const page = await ready({ clipboard: false });
  page.elements['copy-overlay'].fire('click');
  await settle();
  assert.deepEqual(page.log.copied, ['copy']);
  assert.equal(page.elements.flash.textContent, 'Overlay URL copied');
});

test('when nothing can copy, the moderator is told to do it by hand', async () => {
  const page = await ready({ clipboard: false, execCommandResult: false });
  page.elements['copy-overlay'].fire('click');
  await settle();
  assert.equal(page.elements.error.textContent, 'Could not copy. Select the address and copy it by hand.');
  assert.ok(!page.elements.error.hidden);
});

// --- keeping the page in step with the script ---

test('every page has a heading for the keyboard focus, and a menu entry in both menus', () => {
  const ids = new Set([...html.matchAll(/\bid="([^"]+)"/g)].map((m) => m[1]));
  const block = /var HEADINGS = \{([^}]*)\}/.exec(source)[1];
  const headings = [...block.matchAll(/(\w+): '([^']+)'/g)];
  assert.deepEqual(headings.map((m) => m[1]), VIEWS);
  for (const [, view, heading] of headings) {
    assert.ok(ids.has(heading), `missing #${heading}`);
    for (const id of [`view-${view}`, `nav-${view}`, `bnav-${view}`]) assert.ok(ids.has(id), `missing #${id}`);
  }
});

test('the page has no inline script or style, and loads nothing from other sites', () => {
  assert.ok(!/\sstyle="/.test(html), 'a style attribute would be blocked by the content security policy');
  assert.ok(!/<script(?![^>]*\bsrc=)[^>]*>/.test(html), 'an inline script would be blocked');
  for (const [, url] of html.matchAll(/(?:src|href)="([^"#][^"]*)"/g)) {
    assert.ok(!/^[a-z]+:|^\/\//i.test(url), `external address ${url}`);
  }
});

test('the style sheet loads its fonts from this server only', () => {
  const css = fs.readFileSync(path.join(root, 'static/dashboard/dashboard.css'), 'utf8');
  const urls = [...css.matchAll(/url\(([^)]+)\)/g)].map((m) => m[1].replace(/['"]/g, ''));
  assert.ok(urls.length >= 2);
  for (const url of urls) {
    assert.match(url, /^fonts\/[a-z0-9-]+\.woff2$/);
    assert.ok(fs.existsSync(path.join(root, 'static/dashboard', url)), `${url} is missing`);
  }
});

// --- the TikTok username ---

const patches = (page) => page.log.requests.filter((r) => r.path === '/api/chat/tiktok');

test('the username form shows the current username', async () => {
  const page = await ready();
  assert.equal(page.elements['tiktok-username'].value, 'streamer');
  const next = snapshot().settings;
  next.chat.tiktok.username = 'someone_else';
  page.socket().receive(envelope(6, 'settings_updated', next));
  assert.equal(page.elements['tiktok-username'].value, 'someone_else', 'a change made elsewhere shows up');
});

test('a username being typed is not overwritten by a live update', async () => {
  const page = await ready();
  page.document.activeElement = page.elements['tiktok-username'];
  page.elements['tiktok-username'].value = 'half_typed';
  const next = snapshot().settings;
  next.chat.tiktok.username = 'someone_else';
  page.socket().receive(envelope(6, 'settings_updated', next));
  assert.equal(page.elements['tiktok-username'].value, 'half_typed');
});

test('saving sends the username cleaned up, and shows the answer', async () => {
  const page = await ready();
  const updated = snapshot();
  updated.settings.chat.tiktok.username = 'New.Name';
  page.table['PATCH /api/chat/tiktok'] = { status: 200, body: updated };

  page.elements['tiktok-username'].value = '  @New.Name ';
  page.elements['tiktok-form'].fire('submit');
  await settle();

  assert.equal(patches(page).length, 1);
  assert.equal(patches(page)[0].method, 'PATCH');
  assert.deepEqual(patches(page)[0].body, { username: 'New.Name' });
  assert.equal(page.elements['tiktok-username'].value, 'New.Name');
  assert.equal(page.elements.flash.textContent, 'TikTok username saved');
  assert.ok(page.elements.error.hidden);
});

test('saving the same username sends nothing', async () => {
  const page = await ready();
  page.elements['tiktok-username'].value = ' @streamer ';
  page.elements['tiktok-form'].fire('submit');
  await settle();
  assert.equal(patches(page).length, 0);
  assert.equal(page.elements['tiktok-username'].value, 'streamer');
  assert.equal(page.elements.flash.textContent, 'TikTok username saved');
});

test('an unusable username is refused before anything is sent', async () => {
  const page = await ready();
  for (const bad of ['a/b', 'a b', '@', ' @ ', 'x?y=1', 'héllo', 'x'.repeat(65), 'https://www.tiktok.com/@name']) {
    page.elements['tiktok-username'].value = bad;
    page.elements['tiktok-form'].fire('submit');
    await settle();
    assert.equal(patches(page).length, 0, `${bad}: nothing sent`);
    assert.equal(page.elements.error.textContent, 'Invalid value for TikTok username (without @)', bad);
  }
});

test('an empty username is sent: it stops the chat', async () => {
  const page = await ready();
  const updated = snapshot();
  updated.settings.chat.tiktok.username = '';
  page.table['PATCH /api/chat/tiktok'] = { status: 200, body: updated };
  page.elements['tiktok-username'].value = '   ';
  page.elements['tiktok-form'].fire('submit');
  await settle();
  assert.deepEqual(patches(page)[0].body, { username: '' });
  assert.equal(page.elements['tiktok-username'].value, '');
});

test('the server refusing the username is shown and the page is left as it was', async () => {
  const page = await ready();
  page.table['PATCH /api/chat/tiktok'] = { status: 400, body: { error: 'invalid config value `chat.tiktok.username`: nope' } };
  page.elements['tiktok-username'].value = 'other';
  page.elements['tiktok-form'].fire('submit');
  await settle();
  assert.match(page.elements.error.textContent, /chat\.tiktok\.username/);
  assert.ok(page.elements.flash.hidden);
});

test('without a username the queue page says so and points to the settings', async () => {
  const page = await ready();
  assert.ok(page.elements['setup-hint'].hidden, 'nothing to say while a chat is connected');

  page.socket().receive(envelope(6, 'chat_status', { state: 'not_configured', channel: '', text: 'No TikTok username yet. Enter it in Settings to read the chat.' }));
  assert.ok(!page.elements['setup-hint'].hidden);
  assert.equal(page.elements['setup-hint-text'].textContent, 'No TikTok username yet. Enter it in Settings to read the chat.');
  assert.equal(page.elements['chat-status'].textContent, 'No TikTok username yet. Enter it in Settings to read the chat.');
  assert.ok(page.elements['chat-status'].classes.has('state-bad'));
  assert.equal(page.elements['channel-name'].textContent, '');
  assert.equal(page.elements['channel-initial'].textContent, '');

  page.socket().receive(envelope(7, 'chat_status', { state: 'connected', channel: 'new_one', text: 'Connected to TikTok chat @new_one' }));
  assert.ok(page.elements['setup-hint'].hidden);
  assert.equal(page.elements['channel-name'].textContent, 'new_one');
});

test('a page that starts without a username shows the hint at once', async () => {
  const body = snapshot({ chat: { state: 'not_configured', channel: '', text: 'Enter a username' } });
  body.settings.chat.tiktok.username = '';
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body }) } });
  assert.ok(!page.elements['setup-hint'].hidden);
  assert.equal(page.elements['tiktok-username'].value, '');
});
