'use strict';

// The chat overlay: chat-core.js on its own, then chat.js against hand-made
// stand-ins for the browser (document, WebSocket, fetch).

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const root = path.join(__dirname, '../..');
const core = require(path.join(root, 'static/overlay/chat/chat-core.js'));
const source = fs.readFileSync(path.join(root, 'static/overlay/chat/chat.js'), 'utf8');
const css = fs.readFileSync(path.join(root, 'static/overlay/chat/chat.css'), 'utf8');
const html = fs.readFileSync(path.join(root, 'static/overlay/chat/index.html'), 'utf8');
const english = JSON.parse(fs.readFileSync(path.join(root, 'locales/en.json'), 'utf8'));
const indonesian = JSON.parse(fs.readFileSync(path.join(root, 'locales/id.json'), 'utf8'));

const say = (extra = {}) => ({ name: 'fajar_gg', role: 'everyone', source: 'tiktok', text: 'halo semua', at_ms: 0, command: false, skipped: 0, ...extra });
const envelope = (seq, type, data) => ({ v: 1, seq, ts: 0, type, data });
const hello = (seq, theme, chat_log = [], kind = 'panel') =>
  envelope(seq, 'hello', { settings: { overlay: { ['chat_' + kind]: { theme } } }, chat_log });
const look = (theme, kind = 'panel') => ({ overlay: 'chat_' + kind, theme });

// --- the logic ---

test('the looks are the twelve of the style sheet, nine with a background and three without', () => {
  assert.equal(core.THEMES.length, 12);
  assert.deepEqual(core.PANEL_THEMES, ['obsidian', 'paper', 'brutal', 'terminal', 'candy', 'pixel', 'receipt', 'messages', 'ticker']);
  assert.deepEqual(core.FLOAT_THEMES, ['outline', 'comic', 'notes']);
  assert.deepEqual(core.themesOf('float'), core.FLOAT_THEMES);
  assert.deepEqual(core.themesOf('panel'), core.PANEL_THEMES);
  assert.deepEqual(core.themesOf('nope'), core.PANEL_THEMES);
  assert.ok(core.isTheme('comic', 'float') && !core.isTheme('comic', 'panel'));
  assert.ok(core.isTheme('obsidian', 'panel') && !core.isTheme('obsidian', 'float'));
  assert.ok(core.isTheme('comic') && core.isTheme('obsidian'), 'either, when no overlay is named');
  for (const name of core.THEMES) {
    assert.ok(core.isTheme(name));
    assert.ok(css.includes(`[data-theme="${name}"]`), `${name} has styles`);
    assert.ok(english[`dashboard.overlay.theme.${name}`] && indonesian[`dashboard.overlay.theme.${name}`], `${name} has a name in both languages`);
  }
  for (const other of ['neon', '', null, undefined, 3, '__proto__', 'Obsidian']) assert.ok(!core.isTheme(other), String(other));
});

test('every text the page asks for exists in both languages', () => {
  const keys = [...source.matchAll(/'(overlay\.chat\.[a-z]+(?:\.[a-z]+)*[a-z])'/g)].map((m) => m[1]);
  for (const key of [...keys, 'overlay.chat.source.youtube', 'overlay.chat.source.tiktok', 'overlay.chat.command', 'overlay.chat.badge.subscriber', 'overlay.chat.badge.member', 'overlay.chat.badge.moderator']) {
    assert.ok(english[key], `${key} (en)`);
    assert.ok(indonesian[key], `${key} (id)`);
  }
});

test('a name always gets the same colour, and the initial skips the @', () => {
  assert.equal(core.tintIndex('fajar_gg'), core.tintIndex('fajar_gg'));
  for (const name of ['a', 'zz', '\u{1F47B}', 'x'.repeat(500)]) {
    const tint = core.tintIndex(name);
    assert.ok(Number.isInteger(tint) && tint >= 0 && tint < core.TINTS, name);
  }
  assert.equal(core.initial('@minho_k'), 'M');
  assert.equal(core.initial('\u{1F600} happy'), '\u{1F600}');
  assert.equal(core.initial('@'), '?');
});

test('a line has the name with an @, a badge for paying and moderating viewers, and a status for a command', () => {
  const plain = core.lineOf(say(), 1, {});
  assert.equal(plain.who, '@fajar_gg');
  assert.equal(plain.badge, '');
  assert.equal(plain.status, '');
  assert.equal(plain.command, false);
  assert.equal(core.lineOf(say({ name: '@already' }), 1, {}).who, '@already', 'one @ only');

  assert.equal(core.lineOf(say({ role: 'moderator' }), 1, {}).badge, 'Mod');
  assert.equal(core.lineOf(say({ role: 'subscriber' }), 1, {}).badge, 'Sub');
  assert.equal(core.lineOf(say({ role: 'subscriber', source: 'youtube' }), 1, {}).badge, 'Member');
  assert.equal(core.lineOf(say({ role: 'follower' }), 1, {}).badge, '', 'a follower is nothing special');
  assert.equal(core.lineOf(say({ role: 'moderator' }), 1, { 'overlay.chat.badge.moderator': 'Moderator' }).badge, 'Moderator');

  const command = core.lineOf(say({ command: true, text: '!vt dQw4w9WgXcQ hi' }), 2, { 'overlay.chat.command': 'Permintaan video' });
  assert.equal(command.status, 'Permintaan video');
  assert.equal(command.command, true);
  assert.equal(core.lineOf(say({ command: true }), 2, {}).status, 'Video request');
});

test('what is not a line is left out, and long texts are cut', () => {
  for (const bad of [null, undefined, 5, 'x', [], {}, say({ name: '' }), say({ name: '@@' }), say({ text: '   ' }), say({ text: 7 }), say({ name: 5 })]) {
    assert.equal(core.lineOf(bad, 1, {}), null, JSON.stringify(bad));
  }
  const long = core.lineOf(say({ text: 'x'.repeat(2000), name: 'n'.repeat(200) }), 1, {});
  assert.ok(Array.from(long.text).length <= 300 && long.text.endsWith('…'));
  assert.ok(Array.from(long.who).length <= 41);
});

test('the time is local, HH:MM:SS, or empty when unknown', () => {
  const at = new Date(2026, 10, 18, 7, 5, 9).getTime();
  assert.equal(core.clock(at), '07:05:09');
  for (const none of [0, undefined, null, 'x', NaN]) assert.equal(core.clock(none), '', String(none));
});

function controller(extra = {}) {
  const calls = [];
  const ui = {
    setTheme: (name) => calls.push(['theme', name]),
    render: (lines, newest) => calls.push(['render', lines.map((l) => l.text), newest]),
  };
  const c = core.createController({ ui, texts: () => ({}), ...extra });
  return { c, calls, last: () => calls.at(-1) };
}

test('a hello gives the look and the lines the server remembers', () => {
  const { c, calls } = controller();
  c.handle(hello(3, 'pixel', [say({ text: 'one' }), say({ text: 'two' })]));
  assert.deepEqual(calls, [['theme', 'pixel'], ['render', ['one', 'two'], 0]]);
  assert.equal(c.state().theme, 'pixel');
});

test('a new line is drawn as the newest, and only the last forty are kept', () => {
  const { c, last } = controller();
  c.handle(hello(1, 'ticker'));
  for (let i = 1; i <= 7; i += 1) c.handle(envelope(1 + i, 'chat_message', say({ text: `m${i}` })));
  const [, texts, newest] = last();
  assert.deepEqual(texts, ['m1', 'm2', 'm3', 'm4', 'm5', 'm6', 'm7'], 'what fits is for the page to decide, not the look');
  assert.ok(newest > 0);
  assert.equal(c.state().lines.at(-1).id, newest);

  for (let i = 8; i <= 60; i += 1) c.handle(envelope(1 + i, 'chat_message', say({ text: `m${i}` })));
  const kept = c.state().lines;
  assert.equal(kept.length, core.MAX_LINES);
  assert.equal(kept[0].text, `m${60 - core.MAX_LINES + 1}`);
  assert.equal(kept.at(-1).text, 'm60');
});

test('a change of look redraws and keeps the lines', () => {
  const { c, calls, last } = controller();
  c.handle(hello(1, 'obsidian'));
  for (let i = 1; i <= 7; i += 1) c.handle(envelope(1 + i, 'chat_message', say({ text: `m${i}` })));
  c.handle(envelope(9, 'overlay_theme', look('messages')));
  assert.deepEqual(calls.at(-2), ['theme', 'messages']);
  assert.equal(last()[1].length, 7, 'none is lost by changing the look');
});

test('the look of another overlay, an unknown look and old events are ignored', () => {
  const { c, calls } = controller();
  c.handle(hello(10, 'obsidian'));
  const before = calls.length;
  c.handle(envelope(11, 'overlay_theme', { overlay: 'mediashare', theme: 'pixel' }));
  c.handle(envelope(11, 'overlay_theme', look('outline', 'float'))); // the other chat overlay's look
  c.handle(envelope(11, 'overlay_theme', look('comic'))); // a look this overlay does not have
  c.handle(envelope(12, 'overlay_theme', look('neon')));
  c.handle(envelope(13, 'chat_message', say({ name: '' })));
  c.handle(envelope(9, 'chat_message', say({ text: 'older than the snapshot' })));
  c.handle(envelope(14, 'volume', { volume: 3 }));
  c.handle({ v: 2, seq: 15, type: 'chat_message', data: say() });
  c.handle(null);
  c.handle('x');
  assert.equal(calls.filter((x) => x[0] === 'render').length, 1, 'nothing was drawn again');
  assert.equal(c.state().theme, 'obsidian');
  assert.equal(c.state().lines.length, 0);
  assert.equal(calls.length, before);
});

test('a look forced in the address wins over the dashboard', () => {
  const { c, calls } = controller({ forcedTheme: 'brutal' });
  c.handle(hello(1, 'obsidian'));
  assert.deepEqual(calls[0], ['theme', 'brutal']);
  c.handle(envelope(2, 'overlay_theme', look('pixel')));
  assert.equal(c.state().theme, 'brutal');
  const bad = controller({ forcedTheme: 'neon' });
  bad.c.handle(hello(1, 'pixel'));
  assert.equal(bad.c.state().theme, 'pixel', 'a look that does not exist is not forced');
  const wrongOverlay = controller({ forcedTheme: 'comic' });
  wrongOverlay.c.handle(hello(1, 'pixel'));
  assert.equal(wrongOverlay.c.state().theme, 'pixel', 'nor a look of the other overlay');
});

test('a new hello starts the list again', () => {
  const { c } = controller();
  c.handle(hello(1, 'obsidian', [say({ text: 'a' })]));
  c.handle(envelope(2, 'chat_message', say({ text: 'b' })));
  c.handle(hello(9, 'obsidian', [say({ text: 'c' })]));
  assert.deepEqual(c.state().lines.map((l) => l.text), ['c']);
});

test('the transparent overlay has its own looks, its own settings and its own events', () => {
  const { c, calls } = controller({ kind: 'float' });
  c.handle(hello(1, 'notes', [say({ text: 'one' })], 'float'));
  assert.deepEqual(calls.slice(0, 1), [['theme', 'notes']]);
  assert.equal(c.state().theme, 'notes');
  c.handle(envelope(2, 'overlay_theme', look('comic', 'float')));
  assert.equal(c.state().theme, 'comic');
  c.handle(envelope(3, 'overlay_theme', look('ticker')));
  assert.equal(c.state().theme, 'comic', 'the panel look is not its own');
  const fresh = controller({ kind: 'float' });
  fresh.c.handle(hello(1, 'obsidian', [], 'float'));
  assert.equal(fresh.c.state().theme, 'outline', 'a panel look in its settings is refused: it keeps its first');
  const nothing = controller({ kind: 'float' });
  nothing.c.handle(envelope(1, 'hello', { settings: {}, chat_log: [] }));
  assert.equal(nothing.c.state().theme, 'outline');
});

test('a gift is a line marked as one, and an ordinary line is not', () => {
  assert.equal(core.lineOf(say({ gift: true, text: 'sent 5 \u00d7 Rose' }), 1, {}).gift, true);
  assert.equal(core.lineOf(say(), 1, {}).gift, false);
  assert.equal(core.lineOf(say({ gift: 'yes' }), 1, {}).gift, false, 'only true counts');
});

test('a gift is drawn with the class of a gift', () => {
  const page = load();
  page.socket().receive(hello(1, 'obsidian', [say({ text: 'hi' }), say({ gift: true, text: 'sent 2 \u00d7 Rose' })]));
  const [plain, gift] = items(page);
  assert.ok(gift.classes.has('gift') && !plain.classes.has('gift'));
  assert.ok(css.includes('.msg.gift .text'));
});

test('a line says which chat it is from', () => {
  assert.equal(core.lineOf(say(), 1, {}).source, 'tiktok');
  assert.equal(core.lineOf(say({ source: 'youtube' }), 1, {}).source, 'youtube');
  assert.equal(core.lineOf(say({ source: undefined }), 1, {}).source, 'tiktok');
});

// --- the page ---

class FakeNode {
  constructor(tag, id = '') {
    this.tag = tag;
    this.id = id;
    this.children = [];
    this.attrs = {};
    this.style = { props: {}, setProperty(name, value) { this.props[name] = value; } };
    this.classes = new Set();
    this._text = '';
  }
  get className() {
    return [...this.classes].join(' ');
  }
  set className(value) {
    this.classes = new Set(String(value).split(/\s+/).filter(Boolean));
  }
  get classList() {
    return { add: (n) => this.classes.add(n), remove: (n) => this.classes.delete(n), contains: (n) => this.classes.has(n) };
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
  appendChild(child) {
    this.children.push(child);
    return child;
  }
  replaceChildren(...nodes) {
    this.children = nodes;
  }
  get firstChild() {
    return this.children[0] || null;
  }
  removeChild(child) {
    this.children = this.children.filter((c) => c !== child);
    return child;
  }
}

const descendants = (node) => node.children.flatMap((child) => [child, ...descendants(child)]);

function load({ search = '', protocol = 'http:', fetchBody = {}, pathname = '/overlay/chat/panel/' } = {}) {
  const log = { sockets: [], fetches: [], timeouts: [] };
  class FakeWebSocket {
    constructor(url) {
      this.url = url;
      log.sockets.push(this);
    }
    open() {
      this.onopen();
    }
    receive(message) {
      this.onmessage({ data: typeof message === 'string' ? message : JSON.stringify(message) });
    }
    close() {
      this.onclose();
    }
  }
  const elements = {};
  for (const m of html.matchAll(/\bid="([^"]+)"/g)) elements[m[1]] = new FakeNode('el', m[1]);
  elements.root.classes.add('hidden');
  const sandbox = {
    window: {},
    document: { getElementById: (id) => elements[id], createElement: (tag) => new FakeNode(tag) },
    location: { search, protocol, host: '127.0.0.1:8787', pathname },
    URLSearchParams,
    WebSocket: FakeWebSocket,
    fetch: (url, options) => {
      log.fetches.push({ url, options });
      return Promise.resolve({ ok: true, json: () => Promise.resolve(fetchBody) });
    },
    setTimeout: (fn, ms) => log.timeouts.push(ms),
    encodeURIComponent,
    JSON,
    Math,
    Object,
    Array,
  };
  sandbox.window.ChatCore = core;
  vm.runInNewContext(source, sandbox);
  return { log, elements, socket: () => log.sockets.at(-1) };
}

const items = (page) => page.elements.list.children;
const part = (li, name) => descendants(li).find((n) => n.classes.has(name));

test('the page connects to the chat socket, with the token encoded, over wss on https', () => {
  const page = load({ search: '?token=a%26b' });
  assert.equal(page.socket().url, 'ws://127.0.0.1:8787/ws/overlay/chat/panel?token=a%26b');
  assert.equal(load({ protocol: 'https:' }).socket().url, 'wss://127.0.0.1:8787/ws/overlay/chat/panel');
  assert.equal(page.log.fetches[0].url, '/api/i18n');
  assert.equal(load({ pathname: '/overlay/chat/float/' }).socket().url, 'ws://127.0.0.1:8787/ws/overlay/chat/float');
  assert.equal(load({ pathname: '/overlay/chat/float' }).socket().url, 'ws://127.0.0.1:8787/ws/overlay/chat/float');
  const headers = (p) => JSON.parse(JSON.stringify(p.log.fetches[0].options.headers));
  assert.deepEqual(headers(page), { Authorization: 'Bearer a&b' });
  assert.deepEqual(headers(load()), {});
});

test('the page is hidden until the first hello, then shows the look and the lines', () => {
  const page = load();
  assert.ok(page.elements.root.classes.has('hidden'));
  page.socket().open();
  page.socket().receive(hello(2, 'terminal', [say({ text: 'one' }), say({ name: 'sari', text: 'two', role: 'moderator' })]));
  assert.ok(!page.elements.root.classes.has('hidden'));
  assert.equal(page.elements.root.attrs['data-theme'], 'terminal');
  assert.equal(items(page).length, 2);
  assert.equal(part(items(page)[1], 'who').textContent, '@sari');
  assert.equal(part(items(page)[1], 'badge').textContent, 'Mod');
  assert.equal(part(items(page)[1], 'text').textContent, 'two');
  assert.equal(part(items(page)[0], 'av').textContent, 'F');
});

test('a viewer\'s words never become markup, and only the newest line is animated', () => {
  const page = load();
  page.socket().receive(hello(1, 'obsidian', [say({ text: 'old' })]));
  const nasty = '<img src=x onerror=alert(1)>';
  page.socket().receive(envelope(2, 'chat_message', say({ text: nasty, name: '<b>Eve</b>' })));
  const [old, fresh] = items(page);
  assert.equal(part(fresh, 'text').textContent, nasty);
  assert.equal(part(fresh, 'who').textContent, '@<b>Eve</b>');
  assert.ok(fresh.classes.has('enter') && !old.classes.has('enter'));
  assert.ok(descendants(fresh).every((n) => n.tag !== 'img'));
  assert.match(fresh.className, /tint-\d/);
  assert.ok(fresh.style.props['--rot'].endsWith('deg'));
});

test('a command line says what it is, and the dashboard\'s look changes at once', async () => {
  const page = load({ fetchBody: { 'overlay.chat.command': 'Permintaan video' } });
  await new Promise((resolve) => setImmediate(resolve));
  page.socket().receive(hello(1, 'obsidian'));
  page.socket().receive(envelope(2, 'chat_message', say({ command: true, text: '!vt dQw4w9WgXcQ' })));
  const line = items(page)[0];
  assert.ok(line.classes.has('cmd'));
  assert.equal(part(line, 'status').textContent, 'Permintaan video');
  page.socket().receive(envelope(3, 'overlay_theme', look('messages')));
  assert.equal(page.elements.root.attrs['data-theme'], 'messages');
});

test('the words around the lines come from the texts, with English defaults', async () => {
  const bare = load();
  assert.equal(bare.elements.title.textContent, 'Live chat');
  assert.equal(bare.elements.thanks.textContent, 'THANKS FOR CHATTING');
  const translated = load({ fetchBody: { 'overlay.chat.title': 'Chat langsung' } });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(translated.elements.title.textContent, 'Chat langsung');
});

test('?theme= forces a look for one source, if the overlay has it', () => {
  const page = load({ search: '?theme=pixel' });
  page.socket().receive(hello(1, 'obsidian'));
  assert.equal(page.elements.root.attrs['data-theme'], 'pixel');
  const wrong = load({ search: '?theme=neon' });
  wrong.socket().receive(hello(1, 'paper'));
  assert.equal(wrong.elements.root.attrs['data-theme'], 'paper');
  const other = load({ search: '?theme=comic' });
  other.socket().receive(hello(1, 'paper'));
  assert.equal(other.elements.root.attrs['data-theme'], 'paper', 'comic is not a look of the panel overlay');
  const float = load({ search: '?theme=comic', pathname: '/overlay/chat/float/' });
  float.socket().receive(hello(1, 'outline', [], 'float'));
  assert.equal(float.elements.root.attrs['data-theme'], 'comic');
});

test('each line has an icon of the chat it is from, as the first part of its head', () => {
  const page = load();
  page.socket().receive(hello(1, 'obsidian', [say(), say({ source: 'youtube', name: 'budi' })]));
  const icons = items(page).map((li) => part(li, 'src'));
  assert.deepEqual(icons.map((n) => n.attrs['aria-label']), ['TikTok', 'YouTube']);
  assert.ok(icons[0].classes.has('source-tiktok') && icons[1].classes.has('source-youtube'));
  assert.ok(icons.every((n) => n.attrs.role === 'img' && n.textContent === ''));
  assert.equal(part(items(page)[0], 'head').children[0], icons[0]);
  assert.ok(css.includes('.source-youtube') && css.includes('.source-tiktok'));
});

test('it reconnects with a growing delay, up to ten seconds', () => {
  const page = load();
  const delays = [];
  for (let i = 0; i < 6; i += 1) {
    page.socket().close();
    delays.push(page.log.timeouts.at(-1));
  }
  assert.deepEqual(delays, [1000, 2000, 4000, 8000, 10000, 10000]);
});

test('old lines go until the rest fits the source, and the newest is marked after that', () => {
  const page = load();
  // A stand-in for layout: each line is 30 px high and the source holds 100.
  const list = page.elements.list;
  Object.defineProperty(list, 'scrollHeight', { get: () => list.children.length * 30 });
  list.clientHeight = 100;
  page.elements.panel.offsetHeight = 0;
  page.elements.root.clientHeight = 1000;
  page.socket().receive(hello(1, 'obsidian', []));
  for (let i = 1; i <= 8; i += 1) page.socket().receive(envelope(1 + i, 'chat_message', say({ text: `m${i}` })));
  const shown = items(page).map((li) => part(li, 'text').textContent);
  assert.deepEqual(shown, ['m6', 'm7', 'm8'], '100 px and 16 slack: three lines of 30 fit, four (120) do not');
  assert.ok(items(page).at(-1).classes.has('enter') && !items(page)[0].classes.has('enter'));

  // A look that grows with its lines is held inside the source instead.
  const grow = load();
  const box = grow.elements.list;
  Object.defineProperty(box, 'scrollHeight', { get: () => box.children.length * 30 });
  box.clientHeight = 100000;
  grow.elements.root.clientHeight = 200;
  Object.defineProperty(grow.elements.panel, 'offsetHeight', { get: () => box.children.length * 30 });
  grow.socket().receive(hello(1, 'komik', [], 'float'));
  for (let i = 1; i <= 12; i += 1) grow.socket().receive(envelope(1 + i, 'chat_message', say({ text: `m${i}` })));
  assert.equal(items(grow).length, 7, '200 px and 16 slack: seven lines of 30');
  assert.equal(part(items(grow).at(-1), 'text').textContent, 'm12', 'the newest is always there');
});

test('the page never assigns markup', () => {
  assert.ok(!/innerHTML|outerHTML|insertAdjacentHTML|document\.write/.test(source));
  assert.ok(!/<script[^>]+src="https?:/.test(html), 'nothing from another site');
});
