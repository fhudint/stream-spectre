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
  scrollIntoView(options) {
    this.scrolledTo = options;
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
      mode: 'manual', tiktok_min_role: 'follower', youtube_min_role: 'viewer', youtube_superchat_bypass: false,
      min_gifter_level: 0, min_fan_club_level: 0,
      cooldown_secs: 60, max_duration_secs: 600, max_queue_len: 20, youtube_max_waiting: 0, tiktok_max_waiting: 0,
      allow_live: false, allow_shorts: true, allowed_categories: [], volume: 50, max_volume: 80,
    },
    blacklist: { video_ids: ['aaaaaaaaaaa'], channels: [], title_words: [], senders: [] },
    dashboard: { chat_history: 20 },
    command: { prefix: '!', commands: [{ name: 'v', source: 'youtube' }] },
    chat: { tiktok: { username: 'streamer' } },
  },
  chat_log: [],
  volume: 50,
  queue: { pending: [item(7), item(8)], approved: [item(9)], now_playing: item(3) },
  chat: { state: 'connected', channel: 'streamer', text: 'Connected to TikTok chat @streamer' },
  ...extra,
});

const envelope = (seq, type, data) => ({ v: 1, seq, ts: 0, type, data });
const accountBody = (extra = {}) => ({
  youtube_key: { set: true, source: 'saved', last4: 'rstu' },
  dashboard_token: { set: false },
  editable: true,
  ...extra,
});
const settle = async () => {
  for (let i = 0; i < 12; i += 1) await new Promise((resolve) => setImmediate(resolve));
};

function load({
  // Most tests are about the queue page; the landing page has tests of its own.
  hash = '#queue',
  stored = null,
  routes = {},
  protocol = 'http:',
  storageBroken = false,
  frames = false,
  clipboard = true,
  execCommandResult = true,
  clock = 1_000_000,
} = {}) {
  let now = clock;
  const log = { requests: [], sockets: [], timeouts: [], intervals: [], history: [], copied: [], documentListeners: {}, windowListeners: {}, frames: [] };

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
    'GET /api/account': () => ({ status: 200, body: accountBody() }),
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
  // A browser that draws in frames: what is asked for waits until the test runs it.
  if (frames) sandbox.requestAnimationFrame = (fn) => log.frames.push(fn);
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
    // A click bubbles up to the document, like in a browser.
    click: (node) => {
      node.fire('click');
      for (const listener of log.documentListeners.click || []) listener({ target: node });
    },
    clickElsewhere: () => {
      for (const listener of log.documentListeners.click || []) listener({ target: {} });
    },
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
  assert.equal(page.elements['connection'].textContent, 'Program · Disconnected');
  page.socket().open();
  assert.equal(page.elements['connection'].textContent, 'Program · Connected');
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
  // Each list says what is true of it, so "empty" never contradicts a video playing.
  assert.equal(page.elements.pending.children[0].textContent, 'No requests waiting for approval');
  assert.equal(page.elements.approved.children[0].textContent, 'Nothing in the play queue');
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
  assert.equal(page.elements.pending.children[0].textContent, 'No requests waiting for approval');
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
    assert.equal(page.elements.connection.textContent, 'Program · Reconnecting…');
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
  assert.equal(page.elements['setting-tiktok_min_role'].value, 'follower');
  assert.equal(page.elements['setting-youtube_min_role'].value, 'viewer');
  assert.equal(page.elements['setting-youtube_auto_approve'].value, '', 'not set: nobody');
  assert.equal(page.elements['setting-youtube_cooldown_secs'].value, '', 'not set: the general one');
  assert.equal(page.elements['setting-allow_shorts'].checked, true);
  assert.equal(page.elements['setting-allow_live'].checked, false);
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
  assert.deepEqual(patch.body, { video_ids: ['aaaaaaaaaaa'], channels: ['Bad', 'Other'], title_words: [], senders: [] });
  assert.equal(page.elements.flash.textContent, 'Blacklist saved');
  assert.equal(page.elements['blacklist-channels'].value, 'Bad\nOther');
});

test('blocked viewers are a list of their own on the blacklist page', async () => {
  const body = snapshot();
  body.settings.blacklist.senders = ['Bad Viewer', 'youtube:UC1'];
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body }) } });
  assert.equal(page.elements['blacklist-senders'].value, 'Bad Viewer\nyoutube:UC1');
  const updated = snapshot();
  updated.settings.blacklist.senders = ['Bad Viewer', 'youtube:UC1', 'Third'];
  page.table['PATCH /api/blacklist'] = { status: 200, body: updated };
  page.elements['blacklist-senders'].value = 'Bad Viewer\nyoutube:UC1\n Third \n';
  page.elements['blacklist-form'].fire('submit');
  await settle();
  assert.deepEqual(page.log.requests.find((r) => r.path === '/api/blacklist').body.senders, ['Bad Viewer', 'youtube:UC1', 'Third']);
});

const entry = (id, extra = {}) => ({
  id,
  source: 'youtube',
  video_id: ITEM_ID,
  title: `Video ${id}`,
  channel: 'Some Channel',
  requested_by: `Viewer ${id}`,
  requested_at_ms: 0,
  status: 'played',
  reason: '',
  ...extra,
});
const withHistory = (history) => ({ 'GET /api/state': () => ({ status: 200, body: snapshot({ history }) }) });
const blacklistButtons = (node) => buttons(node, 'blacklist');

test('the requests page lists every request, newest first, with what became of it', async () => {
  const history = [
    entry(1),
    entry(2, { status: 'refused', source: 'tiktok', video_id: 'ZSpGELffb', title: '', channel: '', reason: 'Please wait 30 seconds' }),
    entry(3, { status: 'waiting', source: 'tiktok', video_id: '7700124541145599252', title: '@cherlynn052', channel: 'cherlynn052' }),
  ];
  const page = await ready({ hash: '#requests', routes: withHistory(history) });
  assert.deepEqual(shown(page), ['requests']);
  const rows = page.elements.requests.children;
  assert.equal(rows.length, 3);
  assert.match(rows[0].textContent, /@cherlynn052.*Awaiting approval.*7700124541145599252/);
  assert.match(rows[0].textContent, /TikTok/);
  assert.match(rows[1].textContent, /Refused/);
  assert.match(rows[1].textContent, /Please wait 30 seconds/);
  assert.match(rows[2].textContent, /Video 1.*Played.*Some Channel/);
  assert.deepEqual(blacklistButtons(rows[0]).map((b) => b.textContent), ['Blacklist video', 'Blacklist account']);
  assert.deepEqual(blacklistButtons(rows[1]), [], 'a short code cannot be blacklisted');
  assert.deepEqual(blacklistButtons(rows[2]).map((b) => b.textContent), ['Blacklist video', 'Blacklist channel']);
  assert.equal(buttons(rows[2], 'reject').length, 0, 'the list only blacklists');
});

test('an empty history says so, and a live event adds a request or changes its status', async () => {
  const page = await ready({ hash: '#requests' });
  assert.equal(page.elements.requests.children[0].textContent, 'No requests yet');
  page.socket().receive(envelope(6, 'history_entry', entry(1, { status: 'waiting' })));
  assert.equal(page.elements.requests.children.length, 1);
  assert.match(page.elements.requests.children[0].textContent, /Awaiting approval/);
  page.socket().receive(envelope(7, 'history_entry', entry(1, { status: 'playing' })));
  page.socket().receive(envelope(8, 'history_entry', entry(2, { status: 'rejected' })));
  const rows = page.elements.requests.children;
  assert.equal(rows.length, 2);
  assert.match(rows[0].textContent, /Rejected/);
  assert.match(rows[1].textContent, /Playing now/);
});

test('a request has a button that copies a link to its video', async () => {
  const history = [
    entry(1),
    entry(2, { source: 'tiktok', video_id: '7700124541145599252', title: '@cherlynn052', channel: 'cherlynn052' }),
    entry(3, { status: 'refused', source: 'tiktok', video_id: 'ZSpGELffb', title: '', channel: '' }),
    entry(4, { video_id: '<x>' }),
  ];
  const page = await ready({ hash: '#requests', routes: withHistory(history) });
  const rows = page.elements.requests.children; // newest first
  assert.deepEqual(buttons(rows[0], 'copy'), [], 'nothing to open for an ID that is not one');
  const copy = (row) => buttons(row, 'copy')[0];
  assert.equal(copy(rows[1]).textContent, 'Copy link');

  copy(rows[2]).fire('click');
  await settle();
  assert.equal(page.log.clipboard, 'https://www.tiktok.com/@cherlynn052/video/7700124541145599252');
  assert.equal(page.elements.flash.textContent, 'Link copied');
  copy(rows[1]).fire('click');
  await settle();
  assert.equal(page.log.clipboard, 'https://vt.tiktok.com/ZSpGELffb/');
  copy(rows[3]).fire('click');
  await settle();
  assert.equal(page.log.clipboard, `https://www.youtube.com/watch?v=${ITEM_ID}`);
});

test('the requests are drawn when the page is opened, not for every message while it is not shown', async () => {
  const page = await ready({ routes: withHistory([entry(1)]) });
  assert.equal(page.elements.requests.children.length, 0);
  page.go('#requests');
  assert.equal(page.elements.requests.children.length, 1);
  assert.ok(page.elements['requests-heading'].focused);
});

test('blacklisting from the awaiting list adds the entry, then rejects the request', async () => {
  const page = await ready();
  const updated = snapshot();
  updated.settings.blacklist.video_ids = ['aaaaaaaaaaa', ITEM_ID];
  page.table['PATCH /api/blacklist'] = { status: 200, body: updated };
  page.table['POST /api/queue/7/reject'] = { status: 204 };

  const [video, channel] = blacklistButtons(page.elements.pending.children[0]);
  assert.equal(video.textContent, 'Blacklist video');
  assert.equal(channel.textContent, 'Blacklist channel');
  video.fire('click');
  await settle();

  const sent = page.log.requests.filter((r) => r.method !== 'GET').map((r) => `${r.method} ${r.path}`);
  assert.deepEqual(sent, ['PATCH /api/blacklist', 'POST /api/queue/7/reject']);
  assert.deepEqual(page.log.requests.find((r) => r.path === '/api/blacklist').body, { video_ids: ['aaaaaaaaaaa', ITEM_ID] });
  assert.equal(page.elements.flash.textContent, 'Added to the blacklist');
});

test('the channel button sends the channel, and an approved request can be blacklisted too', async () => {
  const page = await ready();
  page.table['PATCH /api/blacklist'] = { status: 200, body: snapshot() };
  page.table['POST /api/queue/9/reject'] = { status: 204 };
  blacklistButtons(page.elements.approved.children[0])[1].fire('click');
  await settle();
  assert.deepEqual(page.log.requests.find((r) => r.path === '/api/blacklist').body, { channels: ['Some Channel'] });
  assert.ok(page.log.requests.some((r) => r.path === '/api/queue/9/reject'));
});

test('a blacklist the server refuses leaves the request where it is', async () => {
  const page = await ready();
  page.table['PATCH /api/blacklist'] = { status: 400, body: { error: 'bad entry' } };
  blacklistButtons(page.elements.pending.children[0])[0].fire('click');
  await settle();
  assert.ok(!page.log.requests.some((r) => r.path.endsWith('/reject')));
  assert.ok(!page.elements.error.hidden);
});

test('from the requests page it only blacklists, and what is listed cannot be added again', async () => {
  const page = await ready({ hash: '#requests', routes: withHistory([entry(1, { status: 'waiting' })]) });
  const updated = snapshot({ history: [entry(1, { status: 'waiting' })] });
  updated.settings.blacklist.channels = ['Some Channel'];
  page.table['PATCH /api/blacklist'] = { status: 200, body: updated };

  const [video, channel] = blacklistButtons(page.elements.requests.children[0]);
  channel.fire('click');
  await settle();
  assert.deepEqual(page.log.requests.filter((r) => r.method !== 'GET').map((r) => r.path), ['/api/blacklist']);
  // The new blacklist redraws the list: the channel is now listed.
  const after = blacklistButtons(page.elements.requests.children[0]);
  assert.equal(after[1].textContent, 'Blacklisted');
  assert.ok(after[1].disabled);
  assert.ok(!after[0].disabled);
  assert.ok(video);
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

test('each chat line says which chat it came from', async () => {
  const log = [line('from tiktok'), line('from youtube', { source: 'youtube', role: 'everyone' }), line('old server', { source: undefined })];
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body: snapshot({ chat_log: log }) }) } });
  const badge = (li) => descendants(li).find((n) => n.classes.has('chat-source'));
  const items = chatItems(page);
  assert.deepEqual(items.map((li) => badge(li).attrs['aria-label']), ['TikTok', 'YouTube', 'TikTok'], 'a line with no source is from before there were two');
  assert.equal(items[0].children[0], badge(items[0]), 'the icon is the first thing in the line, at its left');
  assert.equal(badge(items[1]).attrs.role, 'img');
  assert.ok(badge(items[1]).classes.has('source-youtube') && badge(items[0]).classes.has('source-tiktok'));
  page.socket().receive(envelope(6, 'chat_message', line('live one', { source: 'youtube' })));
  assert.equal(badge(chatItems(page).at(-1)).attrs['aria-label'], 'YouTube');
});

test('a gift is a line of the chat with a mark of its own, and "only video requests" leaves it out', async () => {
  const log = [line('hello'), line('sent 5 \u00d7 Rose', { gift: true, name: 'Rina' }), line('!v dQw4w9WgXcQ', { command: true })];
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body: snapshot({ chat_log: log }) }) } });
  const items = chatItems(page);
  const part = (li, name) => descendants(li).find((n) => n.classes.has(name));
  assert.ok(items[1].classes.has('chat-gift') && !items[0].classes.has('chat-gift'));
  assert.equal(part(items[1], 'chat-text').textContent, 'sent 5 \u00d7 Rose');
  assert.equal(part(items[1], 'chat-name').textContent, 'Rina');
  page.elements['chat-commands-only'].checked = true;
  page.elements['chat-commands-only'].fire('change');
  assert.equal(chatItems(page).length, 1, 'only the video request is left');
});

test('a line of the developer has a tick and a dev badge instead of its role', async () => {
  const log = [line('hi', { role: 'moderator' }), line('hi', { role: 'moderator', developer: true, name: 'fhudint' })];
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body: snapshot({ chat_log: log }) }) } });
  const [plain, dev] = chatItems(page);
  const part = (li, name) => descendants(li).find((n) => n.classes.has(name));
  assert.ok(!part(plain, 'chat-tick'));
  assert.ok(part(dev, 'chat-tick'));
  assert.equal(part(dev, 'chat-role').textContent, 'dev');
  assert.notEqual(part(plain, 'chat-role').textContent, 'dev');
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

const MENU_VIEWS = ['overview', 'queue', 'chat', 'requests', 'blacklist', 'sources', 'rules', 'overlay', 'settings'];
const VIEWS = [...MENU_VIEWS, 'account'];
const shown = (page) => VIEWS.filter((v) => !page.elements[`view-${v}`].hidden);
const current = (page, prefix) => MENU_VIEWS.filter((v) => page.elements[`${prefix}-${v}`].attrs['aria-current'] === 'page');

test('the overview is the landing page, and both menus mark it', async () => {
  const page = await ready({ hash: '' });
  assert.deepEqual(shown(page), ['overview']);
  assert.deepEqual(current(page, 'nav'), ['overview']);
  assert.deepEqual(current(page, 'bnav'), ['overview']);
  assert.ok(!page.elements['overview-heading'].focused, 'opening the page does not steal the focus');
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
  assert.deepEqual(shown(page), ['overview']);
  assert.ok(page.elements['overview-heading'].focused);
});

test('a token in the address is not taken for a page', async () => {
  const page = await ready({ hash: '#token=abc' });
  assert.deepEqual(shown(page), ['overview']);
});

test('the phone tabs choose between the queue and the notices on the queue page', async () => {
  const page = await ready();
  const pressed = () => ['tab-queue', 'tab-notices'].filter((id) => page.elements[id].attrs['aria-pressed'] === 'true');
  assert.equal(page.elements['view-queue'].attrs['data-pane'], 'queue');
  assert.deepEqual(pressed(), ['tab-queue']);

  page.elements['tab-notices'].fire('click');
  assert.equal(page.elements['view-queue'].attrs['data-pane'], 'notices');
  assert.deepEqual(pressed(), ['tab-notices']);

  page.elements['tab-queue'].fire('click');
  assert.equal(page.elements['view-queue'].attrs['data-pane'], 'queue');
});

test('the chat has a page of its own, with its own menu entries, and not on the queue page', async () => {
  const part = (id) => {
    const start = html.indexOf(`id="${id}"`);
    const next = html.slice(start + 10).search(/<(section|dialog) id="view-/);
    return html.slice(start, next < 0 ? html.length : start + 10 + next);
  };
  assert.ok(part('view-chat').includes('id="chat-log"') && part('view-chat').includes('id="chat-commands-only"'));
  assert.ok(!part('view-queue').includes('id="chat-log"'));
  assert.ok(part('view-queue').includes('id="notices"'), 'the notices stay on the queue page');
  assert.match(html, /id="nav-chat"[^>]*href="#chat"/);
  assert.match(html, /id="bnav-chat" href="#chat"/);

  const page = await ready();
  page.go('#chat');
  assert.deepEqual(shown(page), ['chat']);
  assert.deepEqual(current(page, 'nav'), ['chat']);
  assert.deepEqual(current(page, 'bnav'), ['chat']);
  assert.ok(page.elements['chat-heading'].focused);
  page.elements['chat-log'].scrollHeight = 900;
  page.go('#queue');
  page.go('#chat');
  assert.equal(page.elements['chat-log'].scrollTop, 900, 'the newest line is shown on opening');
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
  assert.deepEqual(chips(), ['Commands !v', 'YouTube: everyone', 'TikTok: Follower and above', '60 s cooldown', 'Max. 10:00', 'Queue of 20', '1 blocked']);

  const next = snapshot().settings;
  next.moderation.mode = 'auto';
  next.moderation.tiktok_min_role = 'everyone';
  next.moderation.youtube_min_role = 'member';
  next.moderation.cooldown_secs = 0;
  next.blacklist = { video_ids: [], channels: [], title_words: [], senders: [] };
  page.socket().receive(envelope(6, 'settings_updated', next));
  assert.deepEqual(chips(), ['Commands !v', 'YouTube: Member and above', 'TikTok: everyone', 'Automatic approval', 'No cooldown', 'Max. 10:00', 'Queue of 20']);
});

test('the top bar shows the name chosen on the profile, with its first letter, and not the chat channel', async () => {
  const plain = await ready();
  assert.equal(plain.elements['channel-name'].textContent, 'Account');
  assert.equal(plain.elements['channel-initial'].textContent, 'A');

  const body = snapshot();
  body.settings.dashboard = { chat_history: 20, display_name: 'rina', avatar: '' };
  const named = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body }) } });
  assert.equal(named.elements['channel-name'].textContent, 'rina');
  assert.equal(named.elements['channel-initial'].textContent, 'R');
  assert.equal(named.elements['account-button'].attrs['aria-label'], 'Account menu: rina');
});

test('the top bar shows the picture of the avatar when there is one', async () => {
  const picture = 'data:image/jpeg;base64,/9j/4AAQ';
  const body = snapshot();
  body.settings.dashboard = { chat_history: 20, display_name: '', avatar: picture };
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body }) } });
  const images = descendants(page.elements['channel-initial']).filter((n) => n.tag === 'img');
  assert.equal(images.length, 1);
  assert.equal(images[0].attrs.src, picture);
  assert.equal(images[0].attrs.alt, '', 'decorative: the name is next to it');
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

test('the arrow keys bring the chosen request into view, as the list scrolls by itself', async () => {
  const page = await ready();
  page.press('ArrowDown');
  assert.equal(page.elements.pending.children[1].scrolledTo.block, 'nearest');
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
  assert.equal(page.elements['overlay-shown'].textContent, 'localhost:8787/overlay/mediashare/');
});

test('copying gives OBS the address with localhost and the token', async () => {
  const page = await ready({ stored: 'tok en' });
  page.elements['copy-overlay-page'].fire('click');
  await settle();
  assert.equal(page.log.clipboard, 'http://localhost:8787/overlay/mediashare/?token=tok%20en');
  assert.equal(page.elements.flash.textContent, 'Overlay URL copied');
  assert.ok(!page.elements.flash.hidden);
});

test('without a token the copied address has no query', async () => {
  const page = await ready();
  page.elements['copy-overlay-page'].fire('click');
  await settle();
  assert.equal(page.log.clipboard, 'http://localhost:8787/overlay/mediashare/');
});

test('where the clipboard is not available the old way of copying is tried', async () => {
  const page = await ready({ clipboard: false });
  page.elements['copy-overlay-page'].fire('click');
  await settle();
  assert.deepEqual(page.log.copied, ['copy']);
  assert.equal(page.elements.flash.textContent, 'Overlay URL copied');
});

test('when nothing can copy, the moderator is told to do it by hand', async () => {
  const page = await ready({ clipboard: false, execCommandResult: false });
  page.elements['copy-overlay-page'].fire('click');
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
    const wanted = MENU_VIEWS.includes(view) ? [`view-${view}`, `nav-${view}`, `bnav-${view}`] : [`view-${view}`];
    for (const id of wanted) assert.ok(ids.has(id), `missing #${id}`);
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
    // The chat icons are small SVG pictures inlined in the sheet; nothing is fetched.
    if (/^data:image\/svg\+xml,%3Csvg /.test(url)) continue;
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

  page.socket().receive(envelope(6, 'chat_status', { state: 'not_configured', channel: '', text: 'No TikTok username yet. Enter it on the Account page to read the chat.' }));
  assert.ok(!page.elements['setup-hint'].hidden);
  assert.equal(page.elements['setup-hint-text'].textContent, 'No chat to read yet. Enter a TikTok username or a YouTube live on the Account page.');
  assert.equal(page.elements['chat-status'].textContent, '', 'a chat with nothing set has no pill: the hint says it');
  // The avatar stays, since it opens the account menu; it just says "Account".
  assert.equal(page.elements['channel-name'].textContent, 'Account');
  assert.equal(page.elements['channel-initial'].textContent, 'A');

  page.socket().receive(envelope(7, 'chat_status', { state: 'connected', channel: 'new_one', text: 'Connected to TikTok chat @new_one' }));
  assert.ok(page.elements['setup-hint'].hidden);
  assert.equal(page.elements['channel-name'].textContent, 'Account', 'the chat channel is not the name');
  assert.equal(page.elements['menu-channel'].textContent, 'new_one', 'the menu says which chat is read');
});

test('a page that starts without a username shows the hint at once', async () => {
  const body = snapshot({ chat: { state: 'not_configured', channel: '', text: 'Enter a username' } });
  body.settings.chat.tiktok.username = '';
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body }) } });
  assert.ok(!page.elements['setup-hint'].hidden);
  assert.equal(page.elements['tiktok-username'].value, '');
});

// --- two chats ---

const chatView = (source, state, text, channel = '') => ({ source, state, channel, text });
const twoChats = (extra = {}) =>
  snapshot({
    chats: [chatView('tiktok', 'connected', 'TikTok is connected', 'streamer'), chatView('youtube', 'waiting_for_live', 'YouTube waits', 'HFSuEKR1b0w')],
    ...extra,
  });
const withChats = (body) => ({ routes: { 'GET /api/state': () => ({ status: 200, body }) } });

test('each chat has its own pill, and an event only changes the pill of its source', async () => {
  const page = await ready(withChats(twoChats()));
  assert.equal(page.elements['chat-status'].textContent, 'TikTok is connected');
  assert.ok(page.elements['chat-status'].classes.has('state-ok'));
  assert.equal(page.elements['chat-status-youtube'].textContent, 'YouTube waits');
  assert.ok(page.elements['chat-status-youtube'].classes.has('state-bad'));

  page.socket().receive(envelope(6, 'chat_status', chatView('youtube', 'connected', 'YouTube is connected', 'HFSuEKR1b0w')));
  assert.equal(page.elements['chat-status-youtube'].textContent, 'YouTube is connected');
  assert.ok(page.elements['chat-status-youtube'].classes.has('state-ok'));
  assert.equal(page.elements['chat-status'].textContent, 'TikTok is connected');
});

test('a chat with nothing to read, or switched off, has no pill, and the hint comes only when no chat has anything', async () => {
  const body = twoChats({ chats: [chatView('tiktok', 'not_configured', 'x'), chatView('youtube', 'connected', 'YouTube is connected', 'v')] });
  const page = await ready(withChats(body));
  assert.equal(page.elements['chat-status'].textContent, '');
  assert.ok(page.elements['setup-hint'].hidden, 'the other chat is working');
  assert.ok(!page.elements['menu-disconnect'].hidden);

  page.socket().receive(envelope(6, 'chat_status', chatView('youtube', 'disabled', 'off')));
  assert.equal(page.elements['chat-status-youtube'].textContent, '', 'off on purpose: no pill');
  assert.ok(!page.elements['setup-hint'].hidden, 'nothing is being read now');
  assert.ok(page.elements['menu-disconnect'].hidden, 'nothing to disconnect');
});

test('the menu connects the chats again when either was disconnected from it', async () => {
  const page = await ready(withChats(twoChats({ chats: [chatView('tiktok', 'paused', 'p'), chatView('youtube', 'paused', 'p')] })));
  assert.equal(page.elements['menu-disconnect'].textContent, 'Connect chat');
  page.table['PUT /api/chat/connection'] = { status: 200, body: twoChats() };
  page.elements['menu-disconnect'].fire('click');
  await settle();
  assert.deepEqual(page.log.requests.find((r) => r.path === '/api/chat/connection').body, { connected: true });

  // With only YouTube in use, TikTok has nothing set and says so, not "paused".
  const youtubeOnly = await ready(withChats(twoChats({ chats: [chatView('tiktok', 'not_configured', 'n'), chatView('youtube', 'paused', 'p')] })));
  assert.equal(youtubeOnly.elements['menu-disconnect'].textContent, 'Connect chat');
});

test('the account page has a switch and a status line for each chat, and the YouTube live', async () => {
  const body = twoChats();
  body.settings.chat.youtube = { video: 'HFSuEKR1b0w', enabled: false };
  body.settings.chat.tiktok = { username: 'streamer', enabled: true };
  const page = await ready(withChats(body));
  assert.equal(page.elements['tiktok-enabled'].checked, true);
  assert.equal(page.elements['youtube-chat-enabled'].checked, false);
  assert.equal(page.elements['youtube-chat-video'].value, 'HFSuEKR1b0w');
  // A chat that is being read says how busy it is, too.
  assert.equal(page.elements['tiktok-status'].textContent, 'TikTok is connected \u00b7 0 messages/min');
  assert.equal(page.elements['youtube-chat-status'].textContent, 'YouTube waits');
  page.socket().receive(envelope(6, 'chat_status', chatView('youtube', 'connected', 'Now connected', 'v')));
  assert.equal(page.elements['youtube-chat-status'].textContent, 'Now connected \u00b7 0 messages/min');
});

test('switching a chat on or off sends only the switch, and the answer is adopted', async () => {
  const page = await ready(withChats(twoChats()));
  const answer = twoChats();
  answer.settings.chat.youtube = { video: '', enabled: false };
  page.table['PATCH /api/chat/youtube'] = { status: 200, body: answer };
  page.elements['youtube-chat-enabled'].checked = false;
  page.elements['youtube-chat-enabled'].fire('change');
  await settle();
  assert.deepEqual(page.log.requests.find((r) => r.path === '/api/chat/youtube').body, { enabled: false });
  assert.equal(page.elements.flash.textContent, 'Chat switched off');

  page.table['PATCH /api/chat/tiktok'] = { status: 200, body: twoChats() };
  page.elements['tiktok-enabled'].checked = true;
  page.elements['tiktok-enabled'].fire('change');
  await settle();
  assert.deepEqual(page.log.requests.find((r) => r.path === '/api/chat/tiktok').body, { enabled: true });
  assert.equal(page.elements.flash.textContent, 'Chat switched on');
});

test('the YouTube live is sent as typed and shown as the server keeps it; a refusal is shown', async () => {
  const page = await ready(withChats(twoChats()));
  const answer = twoChats();
  answer.settings.chat.youtube = { video: 'HFSuEKR1b0w', enabled: true };
  page.table['PATCH /api/chat/youtube'] = { status: 200, body: answer };
  page.elements['youtube-chat-video'].value = '  https://www.youtube.com/live/HFSuEKR1b0w  ';
  page.elements['youtube-chat-form'].fire('submit');
  await settle();
  assert.deepEqual(page.log.requests.find((r) => r.path === '/api/chat/youtube').body, { video: 'https://www.youtube.com/live/HFSuEKR1b0w' });
  assert.equal(page.elements['youtube-chat-video'].value, 'HFSuEKR1b0w');
  assert.equal(page.elements.flash.textContent, 'YouTube live saved');

  page.table['PATCH /api/chat/youtube'] = { status: 400, body: { error: 'chat.youtube.video: must be a video' } };
  page.elements['youtube-chat-video'].value = 'nope';
  page.elements['youtube-chat-form'].fire('submit');
  await settle();
  assert.match(page.elements.error.textContent, /chat\.youtube\.video/);
});

// --- the icon ---

// Width, height and colour type of a PNG file, read from its header.
function pngInfo(file) {
  const bytes = fs.readFileSync(file);
  assert.deepEqual([...bytes.subarray(0, 8)], [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a], `${file} is not a PNG`);
  return { width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20), colorType: bytes[25] };
}

test('the page links an icon of each kind, and every one exists with the right type and size', () => {
  const links = [...html.matchAll(/<link\b([^>]*)>/g)].map((m) => Object.fromEntries([...m[1].matchAll(/([\w-]+)="([^"]*)"/g)].map((a) => [a[1], a[2]])));
  const icons = links.filter((l) => /icon/.test(l.rel));
  assert.deepEqual(icons.map((l) => l.href), ['favicon-32.png', 'favicon.svg', 'apple-touch-icon.png']);
  for (const link of icons) {
    assert.ok(fs.existsSync(path.join(root, 'static/dashboard', link.href)), `${link.href} is missing`);
  }
  assert.equal(icons[0].type, 'image/png');
  assert.equal(icons[1].type, 'image/svg+xml');
  assert.deepEqual(pngInfo(path.join(root, 'static/dashboard/favicon-32.png')), { width: 32, height: 32, colorType: 6 });
  // iOS rounds the corners itself, so its icon is a full square without transparency.
  assert.deepEqual(pngInfo(path.join(root, 'static/dashboard/apple-touch-icon.png')), { width: 180, height: 180, colorType: 2 });
});

test('the app icon comes in the sizes the README and the repository need', () => {
  for (const size of [16, 32, 48, 64, 128, 256, 512]) {
    const info = pngInfo(path.join(root, `assets/icon-${size}.png`));
    assert.deepEqual([info.width, info.height], [size, size], `icon-${size}.png`);
    assert.equal(info.colorType, 6, `icon-${size}.png keeps its transparent corners`);
  }
});

test('the icon is the ghost of the sidebar in the accent colour', () => {
  const svg = fs.readFileSync(path.join(root, 'assets/icon.svg'), 'utf8');
  const ghost = /<path d="([^"]+)"/.exec(html)[1];
  assert.ok(svg.includes(`d="${ghost}"`), 'the same outline as the logo in the page');
  const accent = /--accent:\s*(#[0-9a-f]{6})/i.exec(fs.readFileSync(path.join(root, 'static/dashboard/dashboard.css'), 'utf8'))[1];
  assert.ok(svg.includes(`fill="${accent}"`), 'the same colour as the page accent');
  assert.match(svg, /^<svg xmlns="http:\/\/www\.w3\.org\/2000\/svg" viewBox="0 0 64 64"/);
  assert.ok(!/<script|onload|href=/i.test(svg), 'an icon has no scripts or links');
});

// --- the account menu and page ---

const KEY = 'TESTKEY-1234567890abcdefghijklmnopqrstu';
const accountRoute = (body) => ({ status: 200, body });
const menuOpen = (page) => !page.elements['account-menu'].hidden;

test('the avatar opens a menu with the settings link and the disconnect item, and closes it again', async () => {
  const page = await ready();
  const button = page.elements['account-button'];
  assert.ok(!menuOpen(page));
  assert.match(html, /<button id="account-button"[^>]*aria-expanded="false"/, 'closed in the markup');
  assert.equal(button.attrs['aria-label'], 'Account menu: Account');
  assert.equal(page.elements['channel-name'].textContent, 'Account');
  assert.equal(page.elements['menu-channel'].textContent, 'streamer');

  page.click(button);
  assert.ok(menuOpen(page), 'opened');
  assert.equal(button.attrs['aria-expanded'], 'true');
  assert.equal(page.elements['menu-settings'].textContent, 'Account settings');
  assert.equal(page.elements['menu-disconnect'].textContent, 'Disconnect chat');
  assert.ok(!page.elements['menu-disconnect'].hidden);

  page.click(button);
  assert.ok(!menuOpen(page), 'the button closes it too');
  assert.equal(button.attrs['aria-expanded'], 'false');
});

test('the menu closes on a click elsewhere, on Escape (with the focus back on the avatar) and on a choice', async () => {
  const page = await ready();
  const button = page.elements['account-button'];

  page.click(button);
  page.clickElsewhere();
  assert.ok(!menuOpen(page), 'a click elsewhere');

  page.click(button);
  assert.equal(page.press('Escape'), false);
  assert.ok(!menuOpen(page), 'Escape');
  assert.ok(button.focused, 'the focus returns to the avatar');
  assert.equal(button.attrs['aria-expanded'], 'false');

  page.click(button);
  page.click(page.elements['menu-settings']);
  assert.ok(!menuOpen(page), 'a choice');

  // Escape with the menu closed is left alone.
  button.focused = false;
  page.press('Escape');
  assert.ok(!button.focused);
});

test('the menu has one settings link, to the account page, and no separate entries for each credential', () => {
  assert.match(html, /id="menu-settings" href="#account"/);
  for (const gone of ['menu-tiktok', 'menu-youtube', 'menu-token']) assert.ok(!html.includes(`id="${gone}"`), gone);
  const menu = /<div id="account-menu"[\s\S]*?<\/div>/.exec(html)[0];
  assert.equal([...menu.matchAll(/<a /g)].length, 1);
  assert.match(menu, /<button id="menu-disconnect" type="button"/);
});

test('without a channel the avatar says Account, and still opens the menu', async () => {
  const body = snapshot({ chat: null });
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body }) } });
  assert.equal(page.elements['channel-name'].textContent, 'Account');
  assert.equal(page.elements['channel-initial'].textContent, 'A');
  assert.ok(page.elements['menu-channel'].hidden);
  page.click(page.elements['account-button']);
  assert.ok(menuOpen(page));
});

test('#account opens the account dialog over the page, marks no menu entry, and loads what is set', async () => {
  const page = await ready({ hash: '#account' });
  await settle();
  assert.deepEqual(shown(page), ['overview', 'account'], 'a dialog over the page, not a page');
  assert.deepEqual(current(page, 'nav'), ['overview'], 'the page under it stays marked');
  assert.equal(page.log.requests.filter((r) => r.path === '/api/account').length, 1);
  assert.equal(page.elements['key-status'].textContent, 'A key is set (it ends in rstu).');
});

test('#account/<part> brings the keyboard to that part, and the old addresses of the sources lead to their page', async () => {
  const page = await ready();
  page.go('#account/youtube');
  assert.deepEqual(shown(page), ['sources']);
  assert.equal(page.elements['account-youtube'].open, true, 'the key is in the details, opened for it');
  assert.ok(page.elements['youtube-key'].focused);
  page.go('#account/tiktok');
  assert.deepEqual(shown(page), ['sources']);
  assert.ok(page.elements['tiktok-username'].focused);
  page.go('#account/token');
  assert.ok(page.elements['token-heading'].focused);
  page.go('#account/nothing');
  assert.ok(page.elements['account-heading'].focused);
  page.go('#queue');
  assert.deepEqual(shown(page), ['queue']);
});

test('the account is asked for when its page opens, not before', async () => {
  const page = await ready();
  await settle();
  assert.equal(page.log.requests.filter((r) => r.path === '/api/account').length, 0);
  page.go('#account');
  await settle();
  assert.equal(page.log.requests.filter((r) => r.path === '/api/account').length, 1);
  page.go('#sources');
  await settle();
  assert.equal(page.log.requests.filter((r) => r.path === '/api/account').length, 2, 'the key status is on the Sources page');
});

test('the key status says whether a key is set, where it came from, and shows only its ending', async () => {
  const cases = [
    [{ set: true, source: 'saved', last4: 'rstu' }, 'A key is set (it ends in rstu).', /Saved from this dashboard/],
    [{ set: true, source: 'environment', last4: 'wxyz' }, 'A key is set (it ends in wxyz).', /YOUTUBE_API_KEY environment variable/],
    [{ set: true, source: 'environment', last4: null }, 'A key is set.', /YOUTUBE_API_KEY/],
    [{ set: false, source: 'none', last4: null }, 'No key is set. Video requests are refused until you enter one.', /^$/],
  ];
  for (const [youtube_key, status, source] of cases) {
    const page = await ready({ hash: '#account', routes: { 'GET /api/account': () => accountRoute(accountBody({ youtube_key })) } });
    await settle();
    assert.equal(page.elements['key-status'].textContent, status);
    assert.match(page.elements['key-source'].textContent, source);
  }
});

test('the token status says only whether a token is in use', async () => {
  for (const [set, text] of [[true, 'A token protects this dashboard.'], [false, 'No token is set. The dashboard only accepts connections from this computer.']]) {
    const page = await ready({ hash: '#account', routes: { 'GET /api/account': () => accountRoute(accountBody({ dashboard_token: { set } })) } });
    await settle();
    assert.equal(page.elements['token-status'].textContent, text);
  }
});

test('the remove button is only there for a saved key that this computer may remove', async () => {
  const states = [
    [{ youtube_key: { set: true, source: 'saved', last4: 'rstu' } }, true],
    [{ youtube_key: { set: true, source: 'environment', last4: 'rstu' } }, false],
    [{ youtube_key: { set: false, source: 'none', last4: null } }, false],
    [{ youtube_key: { set: true, source: 'saved', last4: 'rstu' }, editable: false }, false],
  ];
  for (const [extra, visible] of states) {
    const page = await ready({ hash: '#account', routes: { 'GET /api/account': () => accountRoute(accountBody(extra)) } });
    await settle();
    assert.equal(!page.elements['key-remove'].hidden, visible, JSON.stringify(extra));
  }
});

test('from another computer the account can be read but the form is locked', async () => {
  const page = await ready({ hash: '#account', routes: { 'GET /api/account': () => accountRoute(accountBody({ editable: false })) } });
  await settle();
  assert.ok(!page.elements['key-locked'].hidden);
  assert.match(page.elements['key-locked'].textContent, /another computer/);
  assert.equal(page.elements['youtube-key'].disabled, true);
  assert.equal(page.elements['key-save'].disabled, true);
  assert.equal(page.elements['key-status'].textContent, 'A key is set (it ends in rstu).');

  const here = await ready({ hash: '#account' });
  await settle();
  assert.ok(here.elements['key-locked'].hidden);
  assert.equal(here.elements['youtube-key'].disabled, false);
  assert.equal(here.elements['key-save'].disabled, false);
});

const putKeys = (page) => page.log.requests.filter((r) => r.method === 'PUT' && r.path === '/api/account/youtube-key');

test('a key is sent trimmed, taken out of the page at once, and never shown', async () => {
  const page = await ready({ hash: '#account', stored: 'tok' });
  await settle();
  page.table['PUT /api/account/youtube-key'] = accountRoute(accountBody({ checked: 'valid', youtube_key: { set: true, source: 'saved', last4: 'rstu' } }));
  page.elements['youtube-key'].value = `  ${KEY}\n`;
  page.elements['key-form'].fire('submit');
  await settle();

  assert.equal(putKeys(page).length, 1);
  assert.deepEqual(putKeys(page)[0].body, { key: KEY });
  assert.equal(putKeys(page)[0].headers.Authorization, 'Bearer tok');
  assert.equal(page.elements['youtube-key'].value, '', 'the field is emptied');
  assert.equal(page.elements.flash.textContent, 'YouTube API key saved');
  assert.ok(page.elements.error.hidden);
  assert.equal(page.elements['key-status'].textContent, 'A key is set (it ends in rstu).');

  // The key is nowhere in what the page shows or remembers.
  for (const id of ['key-status', 'key-source', 'flash', 'error', 'token-status', 'account-heading']) {
    assert.ok(!page.elements[id].textContent.includes(KEY), id);
  }
  assert.ok(![...page.storage.values()].some((value) => String(value).includes(KEY)), 'not in session storage');
});

test('a key that used up the quota is saved with its own note', async () => {
  const page = await ready({ hash: '#account' });
  await settle();
  page.table['PUT /api/account/youtube-key'] = accountRoute(accountBody({ checked: 'quota_exhausted' }));
  page.elements['youtube-key'].value = KEY;
  page.elements['key-form'].fire('submit');
  await settle();
  assert.match(page.elements.flash.textContent, /quota is used up/);
});

test('a key in the wrong shape is refused in the page, before anything is sent', async () => {
  const page = await ready({ hash: '#account' });
  await settle();
  const cases = [
    ['', 'Enter a key first.'],
    ['   \n', 'Enter a key first.'],
    ['two words', /not shaped like a YouTube API key/],
    ['line\nbreak', /not shaped like a YouTube API key/],
    ['café-key', /not shaped like a YouTube API key/],
    ['x'.repeat(201), /not shaped like a YouTube API key/],
  ];
  for (const [typed, expected] of cases) {
    page.elements['youtube-key'].value = typed;
    page.elements['key-form'].fire('submit');
    await settle();
    assert.equal(putKeys(page).length, 0, JSON.stringify(typed));
    if (typeof expected === 'string') assert.equal(page.elements.error.textContent, expected);
    else assert.match(page.elements.error.textContent, expected);
  }
  assert.ok(!page.elements.error.hidden);
});

test('every reason the server gives is shown in words, and the typed key is kept for another try', async () => {
  const page = await ready({ hash: '#account' });
  await settle();
  const reasons = [
    [400, 'key_rejected', /YouTube refused this key/],
    [502, 'cannot_check', /Could not reach YouTube/],
    [500, 'not_saved', /could not be written to disk/],
    [403, 'loopback_only', /only be changed from the computer/],
    [400, 'key_empty', /Enter a key first/],
    [400, 'key_bad_format', /not shaped like/],
    [400, 'bad_request', /not understood/],
  ];
  for (const [status, code, text] of reasons) {
    page.table['PUT /api/account/youtube-key'] = { status, body: { error: 'English text from the server', code } };
    page.elements['youtube-key'].value = KEY;
    page.elements['key-form'].fire('submit');
    await settle();
    assert.match(page.elements.error.textContent, text, code);
    assert.ok(!page.elements.error.textContent.includes('English text from the server'), code);
    assert.ok(!page.elements.error.textContent.includes(KEY), code);
    assert.equal(page.elements['youtube-key'].value, KEY, 'kept so it can be corrected');
    assert.ok(page.elements.flash.hidden);
  }
  // An error without a known code falls back to the server's message.
  page.table['PUT /api/account/youtube-key'] = { status: 500, body: { error: 'something odd' } };
  page.elements['key-form'].fire('submit');
  await settle();
  assert.equal(page.elements.error.textContent, 'Request failed: something odd');
});

test('a refused token while saving a key shows the sign-in form', async () => {
  const page = await ready({ hash: '#account' });
  await settle();
  page.table['PUT /api/account/youtube-key'] = { status: 401, body: { error: 'missing or wrong token' } };
  page.elements['youtube-key'].value = KEY;
  page.elements['key-form'].fire('submit');
  await settle();
  assert.ok(!page.elements.login.hidden);
});

test('the saved key can be removed, and the page then shows what is left', async () => {
  const page = await ready({ hash: '#account' });
  await settle();
  page.table['DELETE /api/account/youtube-key'] = accountRoute(accountBody({ youtube_key: { set: false, source: 'none', last4: null } }));
  page.click(page.elements['key-remove']);
  await settle();
  const sent = page.log.requests.filter((r) => r.method === 'DELETE');
  assert.deepEqual(sent.map((r) => r.path), ['/api/account/youtube-key']);
  assert.equal(page.elements.flash.textContent, 'Saved key removed');
  assert.match(page.elements['key-status'].textContent, /No key is set/);
  assert.ok(page.elements['key-remove'].hidden);

  page.table['DELETE /api/account/youtube-key'] = { status: 403, body: { error: 'x', code: 'loopback_only' } };
  page.click(page.elements['key-remove']);
  await settle();
  assert.match(page.elements.error.textContent, /only be changed from the computer/);
});

test('an answer that is not a proper account description is ignored', async () => {
  for (const junk of [null, 'text', [], {}, { youtube_key: 5 }, { youtube_key: {}, dashboard_token: null }]) {
    const page = await ready({ hash: '#account', routes: { 'GET /api/account': () => accountRoute(junk) } });
    await settle();
    assert.equal(page.elements['key-status'].textContent, '', JSON.stringify(junk));
  }
});

test('the TikTok username form and the key form are on the Sources page, the key in its details', () => {
  const part = (id) => {
    const start = html.indexOf(`id="${id}"`);
    const next = html.slice(start + 10).search(/<(section|dialog) id="view-/);
    return html.slice(start, next < 0 ? html.length : start + 10 + next);
  };
  assert.ok(part('view-sources').includes('id="tiktok-form"'));
  assert.ok(!part('view-account').includes('id="tiktok-form"') && !part('view-settings').includes('id="tiktok-form"'));
  assert.ok(part('view-settings').includes('id="chat-form"'));
  for (const id of ['key-form', 'youtube-key', 'key-save', 'key-remove']) assert.ok(part('view-sources').includes(`id="${id}"`), id);
  const details = part('account-youtube');
  assert.ok(details.startsWith('id="account-youtube" class="adv"') && details.includes('id="key-form"'));
  assert.ok(!/<details[^>]*\bopen\b/.test(html), 'the key is closed until asked for');
});

test('the key field never lets a browser fill it in or spell-check it', () => {
  const input = /<input id="youtube-key"([^>]*)>/.exec(html)[1];
  assert.match(input, /type="password"/);
  assert.match(input, /autocomplete="off"/);
  assert.match(input, /spellcheck="false"/);
  assert.match(input, /maxlength="200"/);
});

// --- disconnecting the chat from the menu ---

const connectionCalls = (page) => page.log.requests.filter((r) => r.path === '/api/chat/connection');

test('the menu offers to disconnect the chat while it is connected, and sends it', async () => {
  const page = await ready();
  assert.ok(!page.elements['menu-disconnect'].hidden);
  assert.equal(page.elements['menu-disconnect'].textContent, 'Disconnect chat');

  const paused = snapshot({ chat: { state: 'paused', channel: 'streamer', text: 'Chat disconnected. Choose Connect chat in the account menu to read it again.' } });
  page.table['PUT /api/chat/connection'] = { status: 200, body: paused };
  page.click(page.elements['account-button']);
  page.click(page.elements['menu-disconnect']);
  await settle();

  assert.equal(connectionCalls(page).length, 1);
  assert.equal(connectionCalls(page)[0].method, 'PUT');
  assert.deepEqual(connectionCalls(page)[0].body, { connected: false });
  assert.equal(page.elements.flash.textContent, 'Chat disconnected');
  assert.ok(!menuOpen(page), 'the menu closes');
  assert.equal(page.elements['menu-disconnect'].textContent, 'Connect chat');
  assert.match(page.elements['chat-status'].textContent, /Chat disconnected/);
  assert.ok(page.elements['chat-status'].classes.has('state-bad'));
});

test('while it is disconnected the same item connects it again', async () => {
  const body = snapshot({ chat: { state: 'paused', channel: 'streamer', text: 'Chat disconnected.' } });
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body }) } });
  assert.equal(page.elements['menu-disconnect'].textContent, 'Connect chat');

  const back = snapshot({ chat: { state: 'reconnecting', channel: 'streamer', text: 'Reconnecting to chat…' } });
  page.table['PUT /api/chat/connection'] = { status: 200, body: back };
  page.click(page.elements['menu-disconnect']);
  await settle();
  assert.deepEqual(connectionCalls(page)[0].body, { connected: true });
  assert.equal(page.elements.flash.textContent, 'Connecting the chat');
  assert.equal(page.elements['menu-disconnect'].textContent, 'Disconnect chat');
});

test('the item follows the chat status as it changes, and is not there with nothing to switch', async () => {
  const page = await ready();
  page.socket().receive(envelope(6, 'chat_status', { state: 'paused', channel: 'streamer', text: 'x' }));
  assert.equal(page.elements['menu-disconnect'].textContent, 'Connect chat');
  page.socket().receive(envelope(7, 'chat_status', { state: 'waiting_for_live', channel: 'streamer', text: 'y' }));
  assert.equal(page.elements['menu-disconnect'].textContent, 'Disconnect chat');
  assert.ok(!page.elements['menu-disconnect'].hidden);
  page.socket().receive(envelope(8, 'chat_status', { state: 'not_configured', channel: '', text: 'z' }));
  assert.ok(page.elements['menu-disconnect'].hidden, 'no username: nothing to disconnect');

  const noChat = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body: snapshot({ chat: null }) }) } });
  assert.ok(noChat.elements['menu-disconnect'].hidden, 'no status known yet');
});

test('a refused disconnect shows the error and leaves the item as it was', async () => {
  const page = await ready();
  page.table['PUT /api/chat/connection'] = { status: 500, body: { error: 'broke' } };
  page.click(page.elements['menu-disconnect']);
  await settle();
  assert.equal(page.elements.error.textContent, 'Request failed: broke');
  assert.equal(page.elements['menu-disconnect'].textContent, 'Disconnect chat');

  page.table['PUT /api/chat/connection'] = { status: 401, body: { error: 'missing or wrong token' } };
  page.click(page.elements['menu-disconnect']);
  await settle();
  assert.ok(!page.elements.login.hidden);
});

// --- pausing the video that is playing ---

const pauseCalls = (page) => page.log.requests.filter((r) => r.path === '/api/playback/pause');
const pauseButton = (page) => buttons(page.elements.now, 'pause')[0];

test('the playing card has a pause button, and pressing it sends the pause', async () => {
  const page = await ready();
  assert.equal(pauseButton(page).textContent, 'Pause');
  assert.equal(buttons(page.elements.now, 'skip').length, 1, 'next to the skip button');
  assert.ok(descendants(page.elements.now).find((n) => n.classes.has('badge-paused')).hidden);

  page.table['PUT /api/playback/pause'] = { status: 200, body: snapshot({ playback_paused: true }) };
  pauseButton(page).fire('click');
  await settle();
  assert.equal(pauseCalls(page).length, 1);
  assert.equal(pauseCalls(page)[0].method, 'PUT');
  assert.deepEqual(pauseCalls(page)[0].body, { paused: true });
  assert.equal(pauseButton(page).textContent, 'Resume');
  assert.equal(pauseButton(page).attrs['aria-pressed'], 'true');
  const badge = descendants(page.elements.now).find((n) => n.classes.has('badge-paused'));
  assert.ok(!badge.hidden);
  assert.equal(badge.textContent, 'Paused');
  assert.ok(descendants(page.elements.now).find((n) => n.classes.has('live')).classes.has('is-paused'));
});

test('while paused the same button resumes', async () => {
  const body = snapshot({ playback_paused: true });
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body }) } });
  assert.equal(pauseButton(page).textContent, 'Resume');
  page.table['PUT /api/playback/pause'] = { status: 200, body: snapshot({ playback_paused: false }) };
  pauseButton(page).fire('click');
  await settle();
  assert.deepEqual(pauseCalls(page)[0].body, { paused: false });
  assert.equal(pauseButton(page).textContent, 'Pause');
});

test('a pause made on another dashboard shows here without rebuilding the card', async () => {
  const page = await ready();
  const bar = descendants(page.elements.now).find((n) => n.tag === 'progress');
  page.socket().receive(envelope(6, 'playback_paused', { item_id: 3, paused: true }));
  assert.equal(pauseButton(page).textContent, 'Resume');
  assert.equal(descendants(page.elements.now).find((n) => n.tag === 'progress'), bar, 'the same card');
  page.socket().receive(envelope(7, 'playback_paused', { item_id: 3, paused: false }));
  assert.equal(pauseButton(page).textContent, 'Pause');
});

test('the next video shows as playing, not paused', async () => {
  const page = await ready();
  page.socket().receive(envelope(6, 'playback_paused', { item_id: 3, paused: true }));
  page.socket().receive(envelope(7, 'play', { item: item(4), volume: 50 }));
  page.socket().receive(envelope(8, 'queue_updated', { pending: [], approved: [], now_playing: item(4) }));
  assert.equal(pauseButton(page).textContent, 'Pause');
  assert.ok(descendants(page.elements.now).find((n) => n.classes.has('badge-paused')).hidden);
});

test('P pauses and resumes, but only while a video is playing and not while typing', async () => {
  const page = await ready();
  page.table['PUT /api/playback/pause'] = { status: 200, body: snapshot({ playback_paused: true }) };
  assert.equal(page.press('p'), true);
  await settle();
  assert.deepEqual(pauseCalls(page).map((r) => r.body), [{ paused: true }]);

  page.table['PUT /api/playback/pause'] = { status: 200, body: snapshot({ playback_paused: false }) };
  assert.equal(page.press('P'), true);
  await settle();
  assert.deepEqual(pauseCalls(page).map((r) => r.body), [{ paused: true }, { paused: false }]);

  assert.equal(page.press('p', { target: { tagName: 'INPUT' } }), false);
  assert.equal(page.press('p', { ctrlKey: true }), false);
  assert.equal(pauseCalls(page).length, 2);

  page.socket().receive(envelope(6, 'queue_updated', { pending: [], approved: [], now_playing: null }));
  assert.equal(page.press('p'), false);
  await settle();
  assert.equal(pauseCalls(page).length, 2, 'nothing is playing');
});

test('a refused pause shows the error and the card stays as it was', async () => {
  const page = await ready();
  page.table['PUT /api/playback/pause'] = { status: 409, body: { error: 'no video is playing', code: 'nothing_playing' } };
  pauseButton(page).fire('click');
  await settle();
  assert.equal(page.elements.error.textContent, 'Request failed: no video is playing');
  assert.equal(pauseButton(page).textContent, 'Pause');

  page.table['PUT /api/playback/pause'] = { status: 401, body: { error: 'missing or wrong token' } };
  pauseButton(page).fire('click');
  await settle();
  assert.ok(!page.elements.login.hidden);
});

test('the keyboard hint lists P', () => {
  assert.match(html, /<kbd>P<\/kbd><span data-i18n="dashboard\.keys\.pause">/);
});

// --- chat commands (Rules page) ---

const commandPatches = (page) => page.log.requests.filter((r) => r.path === '/api/commands');
const commandRows = (page) =>
  page.elements['commands-rows'].children.map((row) => ({
    name: row.children[0],
    source: row.children[1],
    remove: row.children[2],
  }));
const typedCommands = (page) => commandRows(page).map((row) => [row.name.value, row.source.value]);
const withCommands = (command) => {
  const updated = snapshot();
  updated.settings.command = command;
  return updated;
};
const twoCommands = { prefix: '!', commands: [{ name: 'v', source: 'youtube' }, { name: 'yt', source: 'youtube' }] };

test('the rules page shows the commands in force, with an example and in the rules card', async () => {
  const page = await ready();
  assert.equal(page.elements['commands-prefix'].value, '!');
  assert.deepEqual(typedCommands(page), [['v', 'youtube']]);
  assert.equal(page.elements['commands-example'].textContent, 'Example: !v dQw4w9WgXcQ');
  assert.ok(page.elements['rules-chips'].children.some((chip) => chip.textContent === 'Commands !v'));
  // One word is all there is: it cannot be removed.
  assert.equal(commandRows(page)[0].remove.disabled, true);

  const more = await ready();
  more.socket().receive(envelope(6, 'settings_updated', { ...snapshot().settings, command: twoCommands }));
  assert.deepEqual(typedCommands(more), [['v', 'youtube'], ['yt', 'youtube']]);
  assert.ok(commandRows(more).every((row) => row.remove.disabled === false));
  assert.ok(more.elements['rules-chips'].children.some((chip) => chip.textContent === 'Commands !v !yt'));
});

test('adding a word and saving sends the whole list, and shows the answer', async () => {
  const page = await ready();
  page.table['PATCH /api/commands'] = { status: 200, body: withCommands({ prefix: '!', commands: [{ name: 'v', source: 'youtube' }, { name: 'vt', source: 'youtube' }] }) };

  page.click(page.elements['commands-add']);
  assert.equal(commandRows(page).length, 2);
  assert.equal(commandRows(page)[1].name.focused, true, 'the new word is ready to type in');
  // Typed with the prefix in front: the prefix is dropped.
  commandRows(page)[1].name.value = ' !vt ';
  page.elements['commands-form'].fire('submit');
  await settle();

  assert.equal(commandPatches(page).length, 1);
  assert.deepEqual(commandPatches(page)[0].body, {
    prefix: '!',
    commands: [{ name: 'v', source: 'youtube' }, { name: 'vt', source: 'youtube' }],
  });
  assert.deepEqual(typedCommands(page), [['v', 'youtube'], ['vt', 'youtube']]);
  assert.equal(page.elements.flash.textContent, 'Chat commands saved');
  assert.ok(page.elements.error.hidden);
});

test('the prefix can be changed on its own', async () => {
  const page = await ready();
  page.table['PATCH /api/commands'] = { status: 200, body: withCommands({ prefix: '?', commands: [{ name: 'v', source: 'youtube' }] }) };
  page.elements['commands-prefix'].value = '?';
  page.elements['commands-form'].fire('submit');
  await settle();
  assert.deepEqual(commandPatches(page)[0].body, { prefix: '?', commands: [{ name: 'v', source: 'youtube' }] });
  assert.equal(page.elements['commands-example'].textContent, 'Example: ?v dQw4w9WgXcQ');
});

test('a word can be removed, and a changed word is sent', async () => {
  const page = await ready();
  page.socket().receive(envelope(6, 'settings_updated', { ...snapshot().settings, command: twoCommands }));
  page.table['PATCH /api/commands'] = { status: 200, body: withCommands({ prefix: '!', commands: [{ name: 'clip', source: 'youtube' }] }) };

  page.click(commandRows(page)[0].remove);
  assert.deepEqual(typedCommands(page), [['yt', 'youtube']]);
  assert.equal(commandRows(page)[0].remove.disabled, true, 'the last one stays');
  commandRows(page)[0].name.value = 'clip';
  page.elements['commands-form'].fire('submit');
  await settle();
  assert.deepEqual(commandPatches(page)[0].body.commands, [{ name: 'clip', source: 'youtube' }]);
});

test('saving what is already in force sends nothing and tidies what was typed', async () => {
  const page = await ready();
  page.elements['commands-prefix'].value = ' ! ';
  commandRows(page)[0].name.value = ' !V'.toLowerCase();
  page.elements['commands-form'].fire('submit');
  await settle();
  assert.equal(commandPatches(page).length, 0);
  assert.equal(page.elements['commands-prefix'].value, '!');
  assert.deepEqual(typedCommands(page), [['v', 'youtube']]);
  assert.equal(page.elements.flash.textContent, 'Chat commands saved');
});

test('commands that cannot work are refused before anything is sent', async () => {
  const page = await ready();
  let seq = 100;
  const attempt = (prefix, words) => {
    seq += 1;
    page.socket().receive(envelope(seq, 'settings_updated', { ...snapshot().settings, command: { prefix: '!', commands: words.map((name) => ({ name, source: 'youtube' })) } }));
    page.elements['commands-prefix'].value = prefix;
  };
  const submit = async () => {
    page.elements['commands-form'].fire('submit');
    await settle();
    return page.elements.error.textContent;
  };

  attempt('', ['v']);
  assert.equal(await submit(), 'Invalid value for Prefix');
  attempt('a b', ['v']);
  assert.equal(await submit(), 'Invalid value for Prefix');
  attempt('12345', ['v']);
  assert.equal(await submit(), 'Invalid value for Prefix');

  for (const bad of ['', '   ', 'two words', 'vídeo', 'v!', 'x'.repeat(33)]) {
    attempt('!', ['v', 'ok']);
    commandRows(page)[1].name.value = bad;
    assert.equal(await submit(), 'Invalid value for Word', JSON.stringify(bad));
    assert.equal(commandRows(page)[1].name.focused, true, 'the word at fault gets the focus');
  }

  attempt('!', ['v', 'yt']);
  commandRows(page)[1].name.value = 'V';
  assert.equal(await submit(), 'Each word can be used once; capital letters do not count');

  // Nine rows are too many.
  attempt('!', ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h']);
  page.click(page.elements['commands-add']);
  commandRows(page)[8].name.value = 'i';
  assert.equal(await submit(), 'Use between 1 and 8 commands');

  assert.equal(commandPatches(page).length, 0);
});

test('the server refusing the commands is shown and the typed words stay', async () => {
  const page = await ready();
  page.table['PATCH /api/commands'] = { status: 400, body: { error: 'invalid config value `command.commands[1].name`: is used twice' } };
  page.click(page.elements['commands-add']);
  commandRows(page)[1].name.value = 'yt';
  page.elements['commands-form'].fire('submit');
  await settle();
  assert.match(page.elements.error.textContent, /command\.commands\[1\]\.name/);
  assert.ok(page.elements.flash.hidden);
  assert.deepEqual(typedCommands(page), [['v', 'youtube'], ['yt', 'youtube']]);
});

test('a change from outside does not wipe words that are being edited, until they are saved', async () => {
  const page = await ready();
  page.click(page.elements['commands-add']);
  commandRows(page)[1].name.value = 'half';
  // A moderator on another screen changes a rule.
  const other = snapshot().settings;
  other.moderation.cooldown_secs = 5;
  page.socket().receive(envelope(6, 'settings_updated', other));
  assert.deepEqual(typedCommands(page), [['v', 'youtube'], ['half', 'youtube']]);

  page.table['PATCH /api/commands'] = { status: 200, body: withCommands({ prefix: '!', commands: [{ name: 'v', source: 'youtube' }, { name: 'half', source: 'youtube' }] }) };
  page.elements['commands-form'].fire('submit');
  await settle();
  // Saved: the form follows the server again.
  page.socket().receive(envelope(20, 'settings_updated', { ...other, command: twoCommands }));
  assert.deepEqual(typedCommands(page), [['v', 'youtube'], ['yt', 'youtube']]);
});

test('typing in the prefix or a word counts as editing, so an outside change leaves it alone', async () => {
  for (const edit of [
    (page) => { page.elements['commands-prefix'].value = '?'; page.elements['commands-prefix'].fire('input'); },
    (page) => { commandRows(page)[0].name.value = 'mine'; commandRows(page)[0].name.fire('input'); },
    (page) => { commandRows(page)[0].source.fire('change'); },
  ]) {
    const page = await ready();
    edit(page);
    const typed = [page.elements['commands-prefix'].value, ...typedCommands(page).flat()];
    page.socket().receive(envelope(6, 'settings_updated', { ...snapshot().settings, command: twoCommands }));
    assert.deepEqual([page.elements['commands-prefix'].value, ...typedCommands(page).flat()], typed);
  }
});

test('a page whose server sends no command settings still works', async () => {
  const old = snapshot();
  delete old.settings.command;
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body: old }) } });
  assert.equal(page.elements['commands-rows'].children.length, 0);
  assert.ok(!page.elements['rules-chips'].children.some((chip) => /^Commands/.test(chip.textContent)));
});

// --- TikTok videos in the queue ---

const TT = '7700124541145599252';
const ttItem = (id, extra = {}) => ({
  id,
  source: 'tiktok',
  video_id: TT,
  title: '@cherlynn052',
  channel: 'TikTok',
  duration_secs: 0,
  thumbnail_url: '',
  requested_by: { name: `Viewer ${id}`, role: 'follower' },
  ...extra,
});
const withTiktok = (queue) => snapshot({ queue: { pending: [], approved: [], now_playing: null, ...queue } });

test('a TikTok request in a list shows its account, ID and source instead of a length', async () => {
  const body = withTiktok({ pending: [ttItem(7)] });
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body }) } });
  const row = page.elements.pending.children[0];
  const text = row.textContent;
  assert.match(text, /@cherlynn052/);
  assert.match(text, new RegExp(TT));
  assert.match(text, /TikTok/);
  assert.ok(!descendants(row).some((n) => n.tag === 'img'), 'no picture to show');
  const badge = descendants(row).find((n) => n.classes.has('duration'));
  assert.equal(badge && badge.textContent, 'TikTok', 'the source takes the place of the length');
  assert.ok(!/\d:\d\d/.test(text.replace(TT, '')), 'no length');
  // It can be approved and rejected like any other.
  assert.equal(buttons(row, 'approve').length, 1);
  assert.equal(buttons(row, 'reject').length, 1);
});

test('the playing card of a TikTok video has a bar and a pause button, and the length once its player has told it', async () => {
  const body = withTiktok({ now_playing: ttItem(3), approved: [item(9)] });
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body }) } });
  const card = page.elements.now;
  const bar = () => descendants(page.elements.now).find((n) => n.tag === 'progress');
  assert.ok(bar(), 'the bar is there from the start');
  assert.match(card.textContent, /@cherlynn052/);
  assert.ok(!/0:00 of/.test(card.textContent), 'nothing written under it until the length is known');
  assert.ok(!pauseButton(page).hidden, 'it can be paused');
  assert.equal(buttons(card, 'skip').length, 1);
  assert.match(card.textContent, /Next: Title 9/);

  page.socket().receive(envelope(6, 'playback', { item_id: 3, position_secs: 5, duration_secs: 15 }));
  assert.equal(bar().attrs.max, '15');
  assert.equal(bar().attrs.value, '5');
  assert.match(page.elements.now.textContent, /0:05 of 0:15/);
  assert.match(page.elements.now.textContent, /10 s left/);
  page.socket().receive(envelope(7, 'playback', { item_id: 3, position_secs: 400, duration_secs: 15 }));
  assert.equal(bar().attrs.value, '15', 'never past the end');
});

test('a TikTok video is paused from the dashboard and by the key', async () => {
  const body = withTiktok({ now_playing: ttItem(3) });
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body }) } });
  page.table['PUT /api/playback/pause'] = { status: 200, body: { ...body, seq: 9, playback_paused: true } };
  buttons(page.elements.now, 'pause')[0].fire('click');
  await settle();
  page.press('p');
  await settle();
  assert.equal(pauseCalls(page).length, 2);
  assert.deepEqual(pauseCalls(page)[0].body, { paused: true });
  // Skip still works from the keyboard.
  page.table['POST /api/skip'] = { status: 200, body: { skipped: true } };
  page.press('s');
  await settle();
  assert.equal(page.log.requests.filter((r) => r.path === '/api/skip').length, 1);
});

test('going from a TikTok video to a YouTube one brings the progress and the pause button back', async () => {
  const body = withTiktok({ now_playing: ttItem(3) });
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body }) } });
  page.socket().receive(envelope(6, 'play', { item: item(4), volume: 50 }));
  page.socket().receive(envelope(7, 'queue_updated', { pending: [], approved: [], now_playing: item(4) }));
  assert.ok(descendants(page.elements.now).some((n) => n.tag === 'progress'));
  assert.ok(!pauseButton(page).hidden);
});

test('an item without a source, from an older server, is a YouTube one', () => {
  const clean = core.initialState();
  const state = core.reduce(clean, {
    v: 1, seq: 1, type: 'hello',
    data: { settings: {}, queue: { pending: [{ id: 1, video_id: ITEM_ID, requested_by: { name: 'A', role: 'follower' } }], approved: [], now_playing: null } },
  });
  assert.equal(state.queue.pending[0].source, 'youtube');
  const odd = core.reduce(clean, {
    v: 1, seq: 1, type: 'hello',
    data: { settings: {}, queue: { pending: [{ id: 1, source: 'vimeo', video_id: ITEM_ID }], approved: [], now_playing: null } },
  });
  assert.equal(odd.queue.pending[0].source, 'youtube');
  assert.equal(core.hasUnknownLength({ source: 'tiktok' }), true);
  assert.equal(core.hasUnknownLength({ source: 'youtube' }), false);
  assert.equal(core.hasUnknownLength(null), false);
});

test('a command can be given the TikTok source on the rules page', async () => {
  const page = await ready();
  page.click(page.elements['commands-add']);
  const [first, second] = commandRows(page);
  assert.deepEqual(first.source.children.map((o) => [o.attrs.value, o.textContent]), [['youtube', 'YouTube'], ['tiktok', 'TikTok']]);
  second.name.value = '!vt';
  second.source.value = 'tiktok';
  const saved = withCommands({ prefix: '!', commands: [{ name: 'v', source: 'youtube' }, { name: 'vt', source: 'tiktok' }] });
  page.table['PATCH /api/commands'] = { status: 200, body: saved };
  page.elements['commands-form'].fire('submit');
  await settle();
  assert.deepEqual(commandPatches(page)[0].body.commands, [{ name: 'v', source: 'youtube' }, { name: 'vt', source: 'tiktok' }]);
  assert.deepEqual(typedCommands(page), [['v', 'youtube'], ['vt', 'tiktok']]);
  assert.ok(page.elements['rules-chips'].children.some((chip) => chip.textContent === 'Commands !v !vt'));
});


// --- the look of the overlay ---

const themeInputs = (page) => descendants(page.elements.themes).filter((n) => n.tag === 'input');

test('the overlay page offers every look, marks the current one and saves a choice', async () => {
  const withTheme = (theme) => {
    const body = snapshot();
    body.settings.overlay = { mediashare: { theme } };
    return body;
  };
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body: withTheme('paper') }) } });
  const inputs = themeInputs(page);
  assert.deepEqual(inputs.map((i) => i.attrs.value), core.OVERLAY_THEMES);
  assert.deepEqual(inputs.filter((i) => i.checked).map((i) => i.attrs.value), ['paper']);
  const names = descendants(page.elements.themes).filter((n) => n.classes.has('theme-name')).map((n) => n.textContent);
  assert.equal(names[0], 'Obsidian');
  assert.equal(names[1], 'Paper');

  page.table['PATCH /api/overlay/mediashare'] = { status: 200, body: withTheme('pixel') };
  inputs.find((i) => i.attrs.value === 'pixel').fire('change');
  await settle();
  const patch = page.log.requests.find((r) => r.path === '/api/overlay/mediashare');
  assert.equal(patch.method, 'PATCH');
  assert.deepEqual(patch.body, { theme: 'pixel' });
  assert.deepEqual(themeInputs(page).filter((i) => i.checked).map((i) => i.attrs.value), ['pixel']);
});

test('a server that sends no look shows the first one', async () => {
  const page = await ready();
  assert.deepEqual(themeInputs(page).filter((i) => i.checked).map((i) => i.attrs.value), ['obsidian']);
});

test('the server refusing a look is shown and the choice stays where it was', async () => {
  const page = await ready();
  page.table['PATCH /api/overlay/mediashare'] = { status: 400, body: { error: 'unknown variant' } };
  themeInputs(page).find((i) => i.attrs.value === 'comic').fire('change');
  await settle();
  assert.match(page.elements.error.textContent, /unknown variant/);
});

// --- a newer version ---

const RELEASE = 'https://github.com/fhudint/stream-spectre/releases/tag/v0.2.0';
const updateBody = (extra = {}) => ({ current: '0.1.0-rc1', enabled: true, checked: true, latest: null, ...extra });

test('a newer version is shown in a banner with a link, and nothing when there is none', async () => {
  const none = await ready({ routes: { 'GET /api/update': () => ({ status: 200, body: updateBody() }) } });
  await settle();
  assert.ok(none.elements['update-banner'].hidden);
  assert.equal(none.elements['update-version'].textContent, 'You have version 0.1.0-rc1.');
  assert.ok(none.elements['update-off'].hidden);

  const found = await ready({
    routes: { 'GET /api/update': () => ({ status: 200, body: updateBody({ latest: { version: '0.2.0', url: RELEASE } }) }) },
  });
  await settle();
  assert.ok(!found.elements['update-banner'].hidden);
  assert.equal(found.elements['update-text'].textContent, 'Version 0.2.0 is available.');
  assert.equal(found.elements['update-link'].attrs.href, RELEASE);
  assert.equal(found.elements['update-link'].attrs.target, undefined, 'set in the markup, not by the script');
  assert.match(html, /id="update-link"[^>]*target="_blank"[^>]*rel="noopener noreferrer"/);
});

test('a link that is not this project\'s is not shown, and a server without the endpoint is fine', async () => {
  const evil = await ready({
    routes: { 'GET /api/update': () => ({ status: 200, body: updateBody({ latest: { version: '9.9.9', url: 'https://evil.example/x' } }) }) },
  });
  await settle();
  assert.ok(evil.elements['update-banner'].hidden);
  const old = await ready({ routes: { 'GET /api/update': () => ({ status: 404, body: { error: 'x' } }) } });
  await settle();
  assert.ok(old.elements['update-banner'].hidden);
  assert.equal(old.elements['update-version'].textContent, '');
});

test('when the automatic check is off the page says so', async () => {
  const page = await ready({ routes: { 'GET /api/update': () => ({ status: 200, body: updateBody({ enabled: false, checked: false }) }) } });
  await settle();
  assert.ok(!page.elements['update-off'].hidden);
});

test('the button asks now and says what it found, newer version, none, or no answer', async () => {
  const page = await ready({ routes: { 'GET /api/update': () => ({ status: 200, body: updateBody() }) } });
  await settle();

  page.table['POST /api/update/check'] = { status: 200, body: updateBody({ latest: { version: '0.2.0', url: RELEASE } }) };
  page.click(page.elements['update-check']);
  await settle();
  assert.equal(page.elements['update-status'].textContent, 'Version 0.2.0 is available.');
  assert.ok(!page.elements['update-banner'].hidden, 'the banner follows');

  page.table['POST /api/update/check'] = { status: 200, body: updateBody() };
  page.click(page.elements['update-check']);
  await settle();
  assert.equal(page.elements['update-status'].textContent, 'You have the latest version.');
  assert.ok(page.elements['update-banner'].hidden);

  page.table['POST /api/update/check'] = { status: 502, body: { error: 'cannot ask GitHub', code: 'update_unavailable' } };
  page.click(page.elements['update-check']);
  await settle();
  assert.equal(page.elements['update-status'].textContent, 'Could not check. Is this computer online?');
  assert.equal(page.log.requests.filter((r) => r.method === 'POST' && r.path === '/api/update/check').length, 3);
});

// --- the looks of the two chat overlays ---

const pickerInputs = (page, box) => descendants(page.elements[box]).filter((n) => n.tag === 'input');
const checkedIn = (page, box) => pickerInputs(page, box).filter((i) => i.checked).map((i) => i.attrs.value);

test('each chat overlay has its own address, its own looks and its own choice', async () => {
  const body = snapshot();
  body.settings.overlay = { mediashare: { theme: 'pixel' }, chat_panel: { theme: 'ticker' }, chat_float: { theme: 'notes' } };
  const page = await ready({ stored: 'tok en', routes: { 'GET /api/state': () => ({ status: 200, body }) } });
  assert.equal(page.elements['chat-panel-shown'].textContent, 'localhost:8787/overlay/chat/panel/');
  assert.equal(page.elements['chat-float-shown'].textContent, 'localhost:8787/overlay/chat/float/');
  assert.deepEqual(pickerInputs(page, 'chat-panel-themes').map((i) => i.attrs.value), core.CHAT_PANEL_THEMES);
  assert.deepEqual(pickerInputs(page, 'chat-float-themes').map((i) => i.attrs.value), core.CHAT_FLOAT_THEMES);
  assert.deepEqual(checkedIn(page, 'chat-panel-themes'), ['ticker']);
  assert.deepEqual(checkedIn(page, 'chat-float-themes'), ['notes']);
  assert.deepEqual(checkedIn(page, 'themes'), ['pixel'], 'the mediashare overlay keeps its own');
  const groups = new Set(['themes', 'chat-panel-themes', 'chat-float-themes'].map((box) => pickerInputs(page, box)[0].attrs.name));
  assert.equal(groups.size, 3, 'three groups of radio buttons');

  page.click(page.elements['copy-chat-panel']);
  assert.equal(page.log.clipboard, 'http://localhost:8787/overlay/chat/panel/?token=tok%20en');
  page.click(page.elements['copy-chat-float']);
  assert.equal(page.log.clipboard, 'http://localhost:8787/overlay/chat/float/?token=tok%20en');

  const updated = snapshot();
  updated.settings.overlay = { mediashare: { theme: 'pixel' }, chat_panel: { theme: 'ticker' }, chat_float: { theme: 'comic' } };
  page.table['PATCH /api/overlay/chat/float'] = { status: 200, body: updated };
  pickerInputs(page, 'chat-float-themes').find((i) => i.attrs.value === 'comic').fire('change');
  await settle();
  const patch = page.log.requests.find((r) => r.path === '/api/overlay/chat/float');
  assert.equal(patch.method, 'PATCH');
  assert.deepEqual(patch.body, { theme: 'comic' });
  assert.deepEqual(checkedIn(page, 'chat-float-themes'), ['comic']);
  assert.ok(!page.log.requests.some((r) => r.path === '/api/overlay/mediashare' || r.path === '/api/overlay/chat/panel'));

  const panel = snapshot();
  panel.settings.overlay = { mediashare: { theme: 'pixel' }, chat_panel: { theme: 'paper' }, chat_float: { theme: 'comic' } };
  page.table['PATCH /api/overlay/chat/panel'] = { status: 200, body: panel };
  pickerInputs(page, 'chat-panel-themes').find((i) => i.attrs.value === 'paper').fire('change');
  await settle();
  assert.deepEqual(page.log.requests.find((r) => r.path === '/api/overlay/chat/panel').body, { theme: 'paper' });
  assert.deepEqual(checkedIn(page, 'chat-panel-themes'), ['paper']);
});

// --- the tabs of the Rules and Overlay pages ---

const shownIn = (page, ids) => ids.filter((id) => !page.elements[id].hidden);
const selected = (page, prefix, tabs) => tabs.filter((t) => page.elements[`${prefix}${t}`].attrs['aria-selected'] === 'true');

test('the rules are on three tabs, each with the rules of its own, and one Save for all', async () => {
  const page = await ready({ hash: '#rules' });
  const panels = ['rules-general', 'rules-commands', 'rules-youtube', 'rules-tiktok'];
  assert.deepEqual(selected(page, 'rtab-', core.RULES_TABS), ['general']);
  assert.deepEqual(shownIn(page, panels), ['rules-general', 'rules-commands'], 'the chat commands are general');

  // The rules of each tab, by key (a list of choices is one rule, whatever its boxes).
  const inside = (id) =>
    descendants(page.elements[id])
      .filter((n) => n.attrs.id && /^setting-[a-z_]+$/.test(n.attrs.id))
      .map((n) => n.attrs.id.slice(8));
  assert.deepEqual(inside('rules-general'), ['mode', 'cooldown_secs', 'max_duration_secs', 'max_queue_len', 'max_volume', 'allow_live', 'allow_shorts', 'allowed_categories']);
  assert.deepEqual(inside('rules-youtube'), ['youtube_min_role', 'youtube_superchat_bypass', 'youtube_auto_approve', 'youtube_cooldown_secs', 'youtube_max_waiting']);
  assert.deepEqual(inside('rules-tiktok'), ['tiktok_min_role', 'min_gifter_level', 'min_fan_club_level', 'tiktok_cooldown_secs', 'tiktok_max_waiting']);

  page.click(page.elements['rtab-youtube']);
  assert.deepEqual(selected(page, 'rtab-', core.RULES_TABS), ['youtube']);
  assert.deepEqual(shownIn(page, panels), ['rules-youtube']);
  page.click(page.elements['rtab-tiktok']);
  assert.deepEqual(shownIn(page, panels), ['rules-tiktok']);
  page.elements['rtab-tiktok'].fire('keydown', { key: 'ArrowRight' });
  assert.deepEqual(selected(page, 'rtab-', core.RULES_TABS), ['general'], 'wraps round');
  assert.ok(page.elements['rtab-general'].focused);

  // A change on one tab and another is saved together.
  const updated = snapshot();
  updated.settings.moderation.youtube_min_role = 'member';
  updated.settings.moderation.min_gifter_level = 3;
  page.table['PATCH /api/settings'] = { status: 200, body: updated };
  page.elements['setting-youtube_min_role'].value = 'member';
  page.elements['setting-min_gifter_level'].value = '3';
  page.elements['settings-form'].fire('submit');
  await settle();
  const patch = page.log.requests.find((r) => r.method === 'PATCH' && r.path === '/api/settings');
  assert.deepEqual(patch.body, { youtube_min_role: 'member', min_gifter_level: 3 });
});

test('the new rules are read from their controls and saved, an unset option as null', async () => {
  const body = snapshot();
  Object.assign(body.settings.moderation, { youtube_cooldown_secs: 5, youtube_auto_approve: 'member', allowed_categories: ['music'] });
  const page = await ready({ hash: '#rules', routes: { 'GET /api/state': () => ({ status: 200, body }) } });
  // What the server sent is shown.
  assert.equal(page.elements['setting-youtube_cooldown_secs'].value, '5');
  assert.equal(page.elements['setting-youtube_auto_approve'].value, 'member');
  assert.equal(page.elements['setting-allowed_categories-music'].checked, true);
  assert.equal(page.elements['setting-allowed_categories-gaming'].checked, false);

  const updated = snapshot();
  page.table['PATCH /api/settings'] = { status: 200, body: updated };
  page.elements['setting-youtube_cooldown_secs'].value = '';
  page.elements['setting-youtube_auto_approve'].value = '';
  page.elements['setting-allowed_categories-music'].checked = false;
  page.elements['setting-allowed_categories-gaming'].checked = true;
  page.elements['setting-allowed_categories-film'].checked = true;
  page.elements['setting-allow_live'].checked = true;
  page.elements['setting-tiktok_max_waiting'].value = '4';
  page.elements['setting-tiktok_cooldown_secs'].value = '30';
  page.elements['settings-form'].fire('submit');
  await settle();
  const patch = page.log.requests.find((r) => r.method === 'PATCH' && r.path === '/api/settings');
  assert.deepEqual(patch.body, {
    youtube_cooldown_secs: null,
    youtube_auto_approve: null,
    allowed_categories: ['film', 'gaming'],
    allow_live: true,
    tiktok_max_waiting: 4,
    tiktok_cooldown_secs: 30,
  });
});

test('a rule typed wrongly is named and nothing is sent', async () => {
  const page = await ready({ hash: '#rules' });
  page.elements['setting-youtube_cooldown_secs'].value = 'soon';
  page.elements['settings-form'].fire('submit');
  await settle();
  assert.ok(!page.log.requests.some((r) => r.path === '/api/settings'));
  assert.match(page.elements.error.textContent, /Per-user cooldown for this chat/);
});

test('the YouTube minimum role is named in the words of a YouTube chat', async () => {
  const page = await ready({ hash: '#rules' });
  const select = descendants(page.elements['rules-youtube']).find((n) => n.attrs.id === 'setting-youtube_min_role');
  assert.deepEqual(select.children.map((o) => o.textContent), ['Viewer', 'Member', 'Moderator']);
  const auto = descendants(page.elements['rules-youtube']).find((n) => n.attrs.id === 'setting-youtube_auto_approve');
  assert.deepEqual(auto.children.map((o) => o.textContent), ['Nobody (a moderator decides)', 'Viewer', 'Member', 'Moderator']);
  const tiktok = descendants(page.elements['rules-tiktok']).find((n) => n.attrs.id === 'setting-tiktok_min_role');
  assert.deepEqual(tiktok.children.map((o) => o.textContent), ['Everyone', 'Follower', 'Subscriber', 'Moderator']);
});

test('the overlay page has a tab for the mediashare and one for each chat overlay', async () => {
  const page = await ready({ hash: '#overlay' });
  const panels = ['overlay-mediashare', 'overlay-chat-panel', 'overlay-chat-float'];
  assert.deepEqual(selected(page, 'otab-', core.OVERLAY_TABS), ['mediashare']);
  assert.deepEqual(shownIn(page, panels), ['overlay-mediashare']);
  page.click(page.elements['otab-chat-panel']);
  assert.deepEqual(shownIn(page, panels), ['overlay-chat-panel']);
  page.click(page.elements['otab-chat-float']);
  assert.deepEqual(shownIn(page, panels), ['overlay-chat-float']);
  assert.deepEqual(selected(page, 'otab-', core.OVERLAY_TABS), ['chat-float']);
  page.elements['otab-chat-float'].fire('keydown', { key: 'ArrowLeft' });
  assert.deepEqual(shownIn(page, panels), ['overlay-chat-panel']);
  assert.equal(english['dashboard.overlay.tab.chat_panel'], 'Chat with background');
});

// --- what the viewer wrote after the link ---

test('a viewer\'s message is shown as plain text in the waiting list, on the playing card and in the requests', async () => {
  const words = 'halo <b>ka</b>';
  const state = snapshot({
    queue: { pending: [item(7, { message: words }), item(8)], approved: [], now_playing: item(3, { message: 'sip' }) },
    history: [entry(1, { message: words }), entry(2)],
  });
  const page = await ready({ hash: '#requests', routes: { 'GET /api/state': () => ({ status: 200, body: state }) } });
  const waitingRows = page.elements.pending.children;
  assert.ok(waitingRows[0].textContent.includes(`“${words}”`));
  assert.ok(!waitingRows[1].textContent.includes('“'), 'no message, no quote marks');
  assert.ok(page.elements.now.textContent.includes('“sip”'));
  const rows = page.elements.requests.children;
  assert.ok(rows[1].textContent.includes(`“${words}”`));
  assert.ok(!rows[0].textContent.includes('“'));
  assert.equal(descendants(waitingRows[0]).filter((n) => n.tag === 'b').length, 0, 'never markup');
});

// --- roles are named as the chat they come from names them ---

test('a YouTube viewer is shown as a viewer or a member, a TikTok one as a follower or a subscriber', async () => {
  const state = snapshot({
    queue: {
      pending: [
        item(7, { requested_by: { name: 'Yt', role: 'subscriber', source: 'youtube' } }),
        item(8, { requested_by: { name: 'Tt', role: 'subscriber', source: 'tiktok' } }),
        item(10, { requested_by: { name: 'Yv', role: 'follower', source: 'youtube' } }),
      ],
      approved: [],
      now_playing: null,
    },
  });
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body: state }) } });
  const rows = page.elements.pending.children;
  assert.match(rows[0].textContent, /Member/);
  assert.match(rows[1].textContent, /Subscriber/);
  assert.match(rows[2].textContent, /Viewer/);
});

// --- the account dialog: tabs and the profile ---

const panelsShown = (page) => ['account-profile', 'account-token'].filter((id) => !page.elements[id].hidden);
const selectedTab = (page) =>
  ['profile', 'token'].filter((t) => page.elements[`atab-${t}`].attrs['aria-selected'] === 'true');

test('the dialog opens on the profile tab, and the address can ask for another tab', async () => {
  const page = await ready();
  page.go('#account');
  assert.deepEqual(selectedTab(page), ['profile']);
  assert.deepEqual(panelsShown(page), ['account-profile']);
  page.go('#account/token');
  assert.deepEqual(selectedTab(page), ['token']);
  assert.deepEqual(panelsShown(page), ['account-token']);
});

test('a tab can be chosen, and the arrow keys move between tabs', async () => {
  const page = await ready();
  page.go('#account');
  page.click(page.elements['atab-token']);
  assert.deepEqual(selectedTab(page), ['token']);
  assert.equal(page.elements['atab-token'].attrs.tabindex, '0');
  assert.equal(page.elements['atab-profile'].attrs.tabindex, '-1');
  let prevented = false;
  page.elements['atab-profile'].fire('keydown', { key: 'ArrowRight', preventDefault: () => { prevented = true; } });
  assert.deepEqual(selectedTab(page), ['token']);
  assert.ok(prevented && page.elements['atab-token'].focused);
  page.elements['atab-token'].fire('keydown', { key: 'ArrowRight' });
  assert.deepEqual(selectedTab(page), ['profile'], 'wraps round');
  page.elements['atab-profile'].fire('keydown', { key: 'Tab' });
  assert.deepEqual(selectedTab(page), ['profile'], 'other keys are left alone');
});

test('closing the dialog goes back to the page under it, and opening another page closes it', async () => {
  const page = await ready();
  page.go('#rules');
  page.go('#account');
  assert.deepEqual(shown(page), ['rules', 'account']);
  page.click(page.elements['account-close']);
  assert.equal(page.location.hash, '#rules');
  page.go(page.location.hash);
  assert.deepEqual(shown(page), ['rules']);
  page.go('#account');
  page.go('#overlay');
  assert.deepEqual(shown(page), ['overlay'], 'the dialog goes with the move');
});

test('saving the profile sends the name, and the picture only when it changed', async () => {
  const page = await ready();
  page.go('#account');
  const updated = snapshot();
  updated.settings.dashboard = { chat_history: 20, display_name: 'Rina', avatar: '' };
  page.table['PATCH /api/dashboard'] = { status: 200, body: updated };
  page.elements['profile-name'].value = '  Rina  ';
  page.elements['profile-form'].fire('submit');
  await settle();
  const sent = page.log.requests.filter((r) => r.method === 'PATCH' && r.path === '/api/dashboard');
  assert.deepEqual(sent.map((r) => r.body), [{ display_name: 'Rina' }], 'no avatar key: the saved picture stays');
  assert.equal(page.elements['profile-name'].value, 'Rina');
  assert.equal(page.elements.flash.textContent, 'Profile saved.');
  assert.equal(page.elements['channel-name'].textContent, 'Rina');
  assert.equal(page.elements['channel-initial'].textContent, 'R');
});

test('removing the photo is sent as an empty picture, and the name field shows what is saved on opening', async () => {
  const picture = 'data:image/jpeg;base64,/9j/4AAQ';
  const body = snapshot();
  body.settings.dashboard = { chat_history: 20, display_name: 'Rina', avatar: picture };
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body }) } });
  page.go('#account');
  assert.equal(page.elements['profile-name'].value, 'Rina');
  assert.ok(!page.elements['profile-photo-remove'].hidden, 'there is a photo to remove');
  assert.equal(descendants(page.elements['profile-preview']).filter((n) => n.tag === 'img').length, 1);

  page.click(page.elements['profile-photo-remove']);
  assert.ok(page.elements['profile-photo-remove'].hidden);
  assert.equal(descendants(page.elements['profile-preview']).filter((n) => n.tag === 'img').length, 0);
  assert.equal(page.elements['profile-preview'].textContent, 'R');

  const updated = snapshot();
  updated.settings.dashboard = { chat_history: 20, display_name: 'Rina', avatar: '' };
  page.table['PATCH /api/dashboard'] = { status: 200, body: updated };
  page.elements['profile-form'].fire('submit');
  await settle();
  const sent = page.log.requests.filter((r) => r.method === 'PATCH' && r.path === '/api/dashboard');
  assert.deepEqual(sent.map((r) => r.body), [{ display_name: 'Rina', avatar: '' }]);
  assert.equal(descendants(page.elements['channel-initial']).filter((n) => n.tag === 'img').length, 0);
});

test('a picture chosen but not saved is dropped when the dialog is opened again', async () => {
  const picture = 'data:image/jpeg;base64,/9j/4AAQ';
  const body = snapshot();
  body.settings.dashboard = { chat_history: 20, display_name: '', avatar: picture };
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body }) } });
  page.go('#account');
  page.click(page.elements['profile-photo-remove']);
  page.go('#queue');
  page.go('#account');
  assert.ok(!page.elements['profile-photo-remove'].hidden, 'the saved photo is back');
});

test('the server refusing the profile is shown and nothing is claimed saved', async () => {
  const page = await ready();
  page.go('#account');
  page.table['PATCH /api/dashboard'] = { status: 400, body: { error: 'dashboard.avatar must be a picture' } };
  page.elements['profile-name'].value = 'Rina';
  page.elements['profile-form'].fire('submit');
  await settle();
  assert.match(page.elements.error.textContent, /dashboard\.avatar/);
  assert.notEqual(page.elements.flash.textContent, 'Profile saved.');
});

test('the top bar and the menu have no copy button: each overlay has its own on the Overlay page', () => {
  assert.ok(!html.includes('id="copy-overlay"') && !html.includes('overlay-address'));
  for (const id of ['copy-overlay-page', 'copy-chat-panel', 'copy-chat-float']) assert.ok(html.includes(`id="${id}"`), id);
  assert.ok(html.includes('data-i18n="dashboard.overlay.mediashare_hint"'));
  assert.match(english['dashboard.overlay.mediashare_hint'], /1920 x 1080/);
});

test('the developer notice has a switch on the Settings page, on by default, saved at once', async () => {
  const withNotice = (on) => snapshot({ settings: { ...snapshot().settings, overlay: { mediashare: { theme: 'obsidian', developer_notice: on } } } });
  const page = await ready();
  assert.equal(page.elements['developer-notice'].checked, true, 'on when the server says nothing');

  page.table['PATCH /api/overlay/mediashare'] = { status: 200, body: withNotice(false) };
  page.elements['developer-notice'].checked = false;
  page.elements['developer-notice'].fire('change');
  await settle();
  const patch = page.log.requests.find((r) => r.path === '/api/overlay/mediashare');
  assert.deepEqual(patch.body, { developer_notice: false });
  assert.equal(page.elements['developer-notice'].checked, false);

  const off = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body: withNotice(false) }) } });
  assert.equal(off.elements['developer-notice'].checked, false, 'a saved off is shown as off');
  // It sits above the update card in the page.
  assert.ok(html.indexOf('id="developer-notice"') < html.indexOf('id="update-check"'));
});

// --- the chat toolbar and the queue's side panel ---

const names = (page) => chatItems(page).map((li) => readable(li));

test('the chat can be narrowed to one source, to words, and to video requests, all together', async () => {
  const log = [
    line('hello from tt', { name: 'Rina' }),
    line('hello from yt', { name: 'Budi', source: 'youtube' }),
    line('!v dQw4w9WgXcQ', { name: 'Sari', command: true }),
  ];
  const page = await ready({ routes: { 'GET /api/state': () => ({ status: 200, body: snapshot({ chat_log: log }) }) } });
  assert.equal(chatItems(page).length, 3);

  page.elements['chat-src-youtube'].fire('click');
  assert.deepEqual(names(page), ['Budi (Viewer): hello from yt']);
  assert.equal(page.elements['chat-src-youtube'].attrs['aria-pressed'], 'true');
  assert.equal(page.elements['chat-src-all'].attrs['aria-pressed'], 'false');

  page.elements['chat-src-all'].fire('click');
  page.elements['chat-search'].value = '  RINA ';
  page.elements['chat-search'].fire('input');
  assert.equal(chatItems(page).length, 1, 'the name is searched too, whatever the case');
  page.elements['chat-search'].value = 'hello';
  page.elements['chat-search'].fire('input');
  assert.equal(chatItems(page).length, 2);
  page.elements['chat-commands-only'].checked = true;
  page.elements['chat-commands-only'].fire('change');
  assert.equal(chatItems(page).length, 0, 'both filters hold');
  page.socket().receive(envelope(6, 'chat_message', line('hello live', { name: 'Dewi', source: 'youtube' })));
  assert.equal(chatItems(page).length, 0, 'a new line is filtered like the rest');
});

test('a paused chat is not followed, counts what comes in, and the jump button goes to the newest', async () => {
  const page = await ready();
  const box = page.elements['chat-log'];
  box.scrollHeight = 1000;
  box.clientHeight = 300;
  box.scrollTop = 700;
  assert.ok(page.elements['chat-jump'].classes.has('hidden'));

  page.elements['chat-pause'].fire('click');
  assert.equal(page.elements['chat-pause'].textContent, 'Resume scroll');
  page.socket().receive(envelope(6, 'chat_message', line('a')));
  page.socket().receive(envelope(7, 'chat_message', line('b')));
  assert.equal(box.scrollTop, 700, 'the view stays where it was');
  assert.ok(!page.elements['chat-jump'].classes.has('hidden'));
  assert.equal(page.elements['chat-jump'].textContent, 'Jump to latest · 2 new');

  page.elements['chat-jump'].fire('click');
  assert.equal(box.scrollTop, 1000);
  assert.ok(page.elements['chat-jump'].classes.has('hidden'));
  assert.equal(page.elements['chat-pause'].textContent, 'Pause scroll', 'jumping goes on following');
  page.socket().receive(envelope(8, 'chat_message', line('c')));
  assert.equal(box.scrollTop, 1000);
});

test('lines that come in while the moderator reads older ones are counted, and reaching the bottom clears the count', async () => {
  const page = await ready();
  const box = page.elements['chat-log'];
  box.scrollHeight = 1000;
  box.clientHeight = 300;
  box.scrollTop = 100;
  page.socket().receive(envelope(6, 'chat_message', line('a')));
  assert.equal(page.elements['chat-jump'].textContent, 'Jump to latest · 1 new');
  box.scrollTop = 700;
  box.fire('scroll');
  assert.ok(page.elements['chat-jump'].classes.has('hidden'));
});

test('the chat page says how many messages came in during the last minute', async () => {
  const now = 5_000_000; // the page's clock is the one the test sets
  const log = [line('old', { at_ms: now - 120000 }), line('one', { at_ms: now - 30000 }), line('two', { at_ms: now - 1000 })];
  const page = await ready({ clock: now, routes: { 'GET /api/state': () => ({ status: 200, body: snapshot({ chat_log: log }) }) } });
  assert.equal(page.elements['chat-rate'].textContent, '2 messages/min');
});

test('chat messages that arrive in a burst are drawn once per frame, other events at once', async () => {
  const page = await ready({ frames: true });
  for (let i = 0; i < 5; i += 1) page.socket().receive(envelope(6 + i, 'chat_message', line(`m${i}`)));
  assert.equal(chatItems(page).length, 0, 'nothing is drawn until the frame');
  assert.equal(page.log.frames.length, 1, 'one frame was asked for');
  page.log.frames[0]();
  assert.equal(chatItems(page).length, 5);
  page.socket().receive(envelope(11, 'volume', { volume: 30 }));
  assert.equal(page.log.frames.length, 1, 'another kind of event is drawn without waiting');
});

test('the side panel of the queue page folds away and comes back', async () => {
  const page = await ready();
  assert.equal(page.elements['rail-toggle'].textContent, 'Hide side panel');
  page.elements['rail-toggle'].fire('click');
  assert.ok(page.elements['queue-grid'].classes.has('rail-collapsed'));
  assert.equal(page.elements['rail-toggle'].textContent, 'Show side panel');
  page.elements['rail-toggle'].fire('click');
  assert.ok(!page.elements['queue-grid'].classes.has('rail-collapsed'));
  const css = fs.readFileSync(path.join(root, 'static/dashboard/dashboard.css'), 'utf8');
  assert.ok(css.includes('.rail-collapsed .today'), 'the style that hides the two cards exists');
});

// --- the Rules page: cards, presets, minutes and the save bar ---

test('the rules of a tab are cards by group, the duration is in minutes, and the preset it matches is marked', async () => {
  const page = await ready();
  const heading = (id) => descendants(page.elements[id]).filter((n) => n.tag === 'h2').map((n) => n.textContent);
  assert.deepEqual(heading('rules-general'), ['Who can request', 'Limits', 'Allowed content', 'Categories']);
  assert.deepEqual(heading('rules-youtube'), ['Who can request', 'Limits']);
  assert.deepEqual(heading('rules-tiktok'), ['Who can request', 'Limits']);
  assert.equal(page.elements['setting-max_duration_secs'].value, '10', '600 seconds shown as minutes');
  assert.equal(page.elements['setting-cooldown_secs'].value, '60', 'the cooldown stays in seconds');
  assert.equal(page.elements['preset-balanced'].attrs['aria-pressed'], 'true');
  assert.equal(page.elements['preset-strict'].attrs['aria-pressed'], 'false');
});

test('a preset fills the rules in and asks to be saved, the bar is there only for unsaved changes, and discard goes back', async () => {
  const page = await ready();
  const bar = page.elements['rules-savebar'];
  assert.ok(bar.classes.has('hidden'), 'nothing unsaved yet');

  page.elements['preset-strict'].fire('click');
  assert.equal(page.elements['setting-cooldown_secs'].value, '180');
  assert.equal(page.elements['setting-max_duration_secs'].value, '5');
  assert.equal(page.elements['setting-max_queue_len'].value, '10');
  assert.equal(page.elements['setting-youtube_min_role'].value, 'member');
  assert.ok(!bar.classes.has('hidden'));
  assert.equal(page.elements['preset-strict'].attrs['aria-pressed'], 'true');
  assert.equal(page.log.requests.filter((r) => r.method === 'PATCH').length, 0, 'nothing is sent by a preset');

  page.elements['rules-discard'].fire('click');
  assert.ok(bar.classes.has('hidden'));
  assert.equal(page.elements['setting-cooldown_secs'].value, '60');
  assert.equal(page.elements['preset-balanced'].attrs['aria-pressed'], 'true');

  page.elements['setting-cooldown_secs'].value = '45';
  page.elements['setting-cooldown_secs'].fire('input');
  assert.ok(!bar.classes.has('hidden'));
  assert.equal(page.elements['preset-balanced'].attrs['aria-pressed'], 'false', 'a mix is no preset');
  page.elements['setting-cooldown_secs'].value = '60';
  page.elements['setting-cooldown_secs'].fire('input');
  assert.ok(bar.classes.has('hidden'), 'editing back to what is saved leaves nothing unsaved');
});

test('the minutes typed are saved as seconds, and the bar goes once the server has the rules', async () => {
  const page = await ready();
  const updated = snapshot();
  updated.settings.moderation.max_duration_secs = 90;
  page.table['PATCH /api/settings'] = { status: 200, body: updated };
  page.elements['setting-max_duration_secs'].value = '1.5';
  page.elements['setting-max_duration_secs'].fire('input');
  assert.ok(!page.elements['rules-savebar'].classes.has('hidden'));
  page.elements['settings-form'].fire('submit');
  await settle();
  assert.deepEqual(page.log.requests.find((r) => r.method === 'PATCH').body, { max_duration_secs: 90 });
  assert.ok(page.elements['rules-savebar'].classes.has('hidden'));
  assert.equal(page.elements['setting-max_duration_secs'].value, '1.5');
});

test('a value that cannot be read still counts as unsaved, and is refused on save', async () => {
  const page = await ready();
  page.elements['setting-max_duration_secs'].value = 'ten';
  page.elements['setting-max_duration_secs'].fire('input');
  assert.ok(!page.elements['rules-savebar'].classes.has('hidden'));
  page.elements['settings-form'].fire('submit');
  await settle();
  assert.equal(page.log.requests.filter((r) => r.method === 'PATCH').length, 0);
  assert.ok(!page.elements.error.hidden);
});

// --- the overview ---

const kids = (node) => descendants(node);

test('the overview counts what the queue page counts, and links to it', async () => {
  const body = snapshot({ stats: { since_ms: 0, received: 24, approved: 18, rejected: 3 } });
  const page = await ready({ hash: '', routes: { 'GET /api/state': () => ({ status: 200, body }) } });
  assert.equal(page.elements['ov-waiting'].textContent, '2', 'the two waiting requests of the snapshot');
  assert.equal(page.elements['ov-received'].textContent, '24');
  assert.equal(page.elements['ov-approved'].textContent, '18');
  assert.equal(page.elements['ov-rejected'].textContent, '3');
  page.socket().receive(envelope(6, 'queue_updated', { pending: [item(7)], approved: [], now_playing: null }));
  assert.equal(page.elements['ov-waiting'].textContent, '1', 'it follows the queue');
  assert.ok(html.includes('href="#queue"'));
});

test('each chat source says how it is doing, or that there is nothing to read with a link to Sources', async () => {
  const page = await ready({ hash: '', ...withChats(twoChats()) });
  const rows = () =>
    page.elements['ov-sources'].children.map((li) =>
      kids(li).filter((n) => n.tag !== 'a' && n.textContent).map((n) => n.textContent).join('|')
    );
  assert.deepEqual(rows(), ['YouTube|YouTube waits', 'TikTok|TikTok is connected']);
  assert.ok(kids(page.elements['ov-sources']).every((n) => n.tag !== 'a'), 'no link while both are being read');
  page.socket().receive(envelope(6, 'chat_status', chatView('youtube', 'not_configured', '', '')));
  assert.deepEqual(rows(), ['YouTube|Nothing to read', 'TikTok|TikTok is connected']);
  const link = kids(page.elements['ov-sources']).find((n) => n.tag === 'a');
  assert.equal(link.attrs.href, '#sources');
});

test('the chart counts the messages of each minute as they come, and says its busiest minute', async () => {
  const page = await ready({ hash: '' });
  const bars = () => page.elements['ov-chart'].children.map((n) => n.className);
  assert.equal(bars().length, 15);
  assert.ok(bars().every((c) => c === 'h0'), 'quiet to begin with');
  for (let i = 0; i < 4; i += 1) page.socket().receive(envelope(6 + i, 'chat_message', line(`m${i}`, { at_ms: 1_000_000 })));
  const now = bars();
  assert.equal(now[14], 'h10', 'the newest minute is the busiest');
  assert.equal(now[13], 'h0');
  assert.match(page.elements['ov-chart'].attrs['aria-label'], /at most 4/);
  assert.equal(page.elements['ov-rate'].textContent, '4');
  // A message the snapshot already holds is not counted twice.
  page.socket().receive(envelope(2, 'chat_message', line('old', { at_ms: 1_000_000 })));
  assert.match(page.elements['ov-chart'].attrs['aria-label'], /at most 4/);
});

test('the playing video is shown in miniature, with the pause and skip buttons that work', async () => {
  const page = await ready({ hash: '' });
  const box = page.elements['ov-now'];
  assert.match(box.textContent, /Title 3/);
  assert.match(box.textContent, /Viewer 3/);
  const button = (action) => kids(box).find((n) => n.tag === 'button' && n.attrs['data-action'] === action);
  page.table['PUT /api/playback/pause'] = { status: 200, body: snapshot({ playback_paused: true }) };
  button('pause').fire('click');
  await settle();
  assert.equal(page.log.requests.find((r) => r.path === '/api/playback/pause').body.paused, true);
  assert.equal(button('pause').textContent, 'Resume');
  page.table['POST /api/skip'] = { status: 204 };
  button('skip').fire('click');
  await settle();
  assert.ok(page.log.requests.some((r) => r.path === '/api/skip'));
  page.socket().receive(envelope(7, 'queue_updated', { pending: [], approved: [], now_playing: null }));
  assert.equal(box.textContent, 'Nothing is playing');
});

test('the overview lists the rules in force, like the queue page does', async () => {
  const page = await ready({ hash: '' });
  assert.ok(page.elements['ov-rules'].children.length > 0);
  assert.deepEqual(
    page.elements['ov-rules'].children.map((n) => n.textContent),
    page.elements['rules-chips'].children.map((n) => n.textContent)
  );
});
