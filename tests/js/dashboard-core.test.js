'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const core = require('../../static/dashboard/dashboard-core.js');

const root = path.join(__dirname, '../..');

function item(id, extra = {}) {
  return {
    id,
    video_id: 'dQw4w9WgXcQ',
    title: `Title ${id}`,
    channel: 'Channel',
    duration_secs: 213,
    thumbnail_url: 'https://i.ytimg.com/vi/dQw4w9WgXcQ/mqdefault.jpg',
    requested_by: { name: `Viewer ${id}`, role: 'follower' },
    ...extra,
  };
}

const settings = () => ({
  moderation: { mode: 'manual', min_role: 'follower', cooldown_secs: 60, volume: 50, max_volume: 80 },
  blacklist: { video_ids: [], channels: [], title_words: [] },
});
const envelope = (seq, type, data) => ({ v: 1, seq, ts: 0, type, data });
const hello = (seq, extra = {}) =>
  envelope(seq, 'hello', {
    language: 'en',
    settings: settings(),
    volume: 50,
    queue: { pending: [item(1)], approved: [], now_playing: null },
    chat: { state: 'connected', channel: 'x', text: 'Connected' },
    ...extra,
  });

// --- helpers ---

test('format fills placeholders in one pass', () => {
  assert.equal(core.format('Hi {name}!', { name: 'Budi' }), 'Hi Budi!');
  assert.equal(core.format('{a} {b}', { a: '{b}', b: 'X' }), '{b} X');
  assert.equal(core.format('a {x} b', {}), 'a {x} b');
  assert.equal(core.format('open { brace', {}), 'open { brace');
  assert.equal(core.format('{{name}}', { name: 'v' }), '{v}');
  assert.equal(core.format('{toString}', {}), '{toString}');
});

test('formatDuration matches the server', () => {
  for (const [secs, text] of [[0, '0:00'], [5, '0:05'], [185, '3:05'], [600, '10:00'], [3723, '1:02:03'], [90000, '25:00:00']]) {
    assert.equal(core.formatDuration(secs), text);
  }
  assert.equal(core.formatDuration(-4), '0:00');
  assert.equal(core.formatDuration('oops'), '0:00');
  assert.equal(core.formatDuration(61.9), '1:01');
});

test('only YouTube thumbnail URLs are shown', () => {
  assert.equal(core.thumbnailUrl(item(1)), 'https://i.ytimg.com/vi/dQw4w9WgXcQ/mqdefault.jpg');
  for (const bad of [
    'http://i.ytimg.com/vi/dQw4w9WgXcQ/mqdefault.jpg',
    'https://evil.example/vi/dQw4w9WgXcQ/mqdefault.jpg',
    'https://i.ytimg.com.evil.example/vi/dQw4w9WgXcQ/mqdefault.jpg',
    'https://i.ytimg.com/vi/short/mqdefault.jpg',
    'javascript:alert(1)',
    'https://i.ytimg.com/vi/dQw4w9WgXcQ/mqdefault.jpg?x=1',
    '',
  ]) {
    assert.equal(core.thumbnailUrl({ thumbnail_url: bad }), null, bad);
  }
  assert.equal(core.thumbnailUrl(null), null);
  assert.equal(core.thumbnailUrl({}), null);
});

test('parseLines trims, drops blanks and repeats, keeps order', () => {
  assert.deepEqual(core.parseLines('  a \r\n\nb\na\n  \nc'), ['a', 'b', 'c']);
  assert.deepEqual(core.parseLines(''), []);
  assert.deepEqual(core.parseLines('__proto__\nconstructor\n__proto__'), ['__proto__', 'constructor']);
  assert.equal(core.linesToText(['a', 'b']), 'a\nb');
  assert.equal(core.linesToText(undefined), '');
});

test('diffSettings returns only what changed', () => {
  const current = { mode: 'manual', cooldown_secs: 60, max_queue_len: 20 };
  assert.deepEqual(core.diffSettings(current, { mode: 'auto', cooldown_secs: 60, max_queue_len: 20 }), { mode: 'auto' });
  assert.deepEqual(core.diffSettings(current, { ...current }), {});
  assert.deepEqual(core.diffSettings(null, { mode: 'auto' }), { mode: 'auto' });
});

test('readSettings accepts good values and names the first bad field', () => {
  const good = {
    mode: 'auto', min_role: 'subscriber', min_gifter_level: '3', min_fan_club_level: ' 0 ',
    cooldown_secs: '60', max_duration_secs: '600', max_queue_len: '20', max_volume: '80',
  };
  assert.deepEqual(core.readSettings(good).value, {
    mode: 'auto', min_role: 'subscriber', min_gifter_level: 3, min_fan_club_level: 0,
    cooldown_secs: 60, max_duration_secs: 600, max_queue_len: 20, max_volume: 80,
  });
  for (const [key, bad] of [
    ['mode', 'sometimes'], ['min_role', 'vip'], ['cooldown_secs', ''], ['cooldown_secs', '-1'],
    ['max_queue_len', '1.5'], ['max_volume', 'loud'], ['min_gifter_level', '1e2'], ['max_duration_secs', '0x10'],
  ]) {
    assert.equal(core.readSettings({ ...good, [key]: bad }).error, key, `${key}=${bad}`);
  }
});

test('the settings form edits real configuration keys', () => {
  const example = fs.readFileSync(path.join(root, 'config.example.toml'), 'utf8');
  const section = example.split('[moderation]')[1].split(/\n\[/)[0];
  const keys = [...section.matchAll(/^([a-z_]+)\s*=/gm)].map((m) => m[1]);
  for (const field of core.SETTINGS_FIELDS) assert.ok(keys.includes(field.key), field.key);
  assert.ok(keys.includes('volume'), 'volume has its own slider');

  const blacklist = example.split('[blacklist]')[1];
  for (const field of core.BLACKLIST_FIELDS) assert.match(blacklist, new RegExp(`^${field.key}\\s*=`, 'm'));
});

// --- state ---

test('hello replaces what the server owns and keeps the notices', () => {
  let state = core.initialState();
  state = core.reduce(state, envelope(1, 'notice', { level: 'warning', code: 'x', text: 'careful', sender: 'Budi' }));
  state = core.reduce(state, hello(5));
  assert.equal(state.seq, 5);
  assert.equal(state.language, 'en');
  assert.equal(state.queue.pending.length, 1);
  assert.equal(state.settings.moderation.max_volume, 80);
  assert.equal(state.chat.state, 'connected');
  assert.equal(state.notices.length, 1);
});

test('events update the queue, settings, volume and chat status', () => {
  let state = core.reduce(core.initialState(), hello(10));
  state = core.reduce(state, envelope(11, 'queue_updated', { pending: [], approved: [item(2)], now_playing: item(3) }));
  assert.deepEqual(state.queue.pending, []);
  assert.equal(state.queue.approved[0].id, 2);
  assert.equal(state.queue.now_playing.id, 3);

  state = core.reduce(state, envelope(12, 'volume', { volume: 30 }));
  assert.equal(state.volume, 30);
  state = core.reduce(state, envelope(13, 'chat_status', { state: 'reconnecting', channel: 'x', text: 'Reconnecting' }));
  assert.equal(state.chat.state, 'reconnecting');
  const next = settings();
  next.moderation.mode = 'auto';
  state = core.reduce(state, envelope(14, 'settings_updated', next));
  assert.equal(state.settings.moderation.mode, 'auto');
  assert.equal(state.seq, 14);
});

test('stale, repeated and foreign events change nothing', () => {
  const state = core.reduce(core.initialState(), hello(10));
  for (const message of [
    envelope(10, 'volume', { volume: 1 }),
    envelope(9, 'volume', { volume: 1 }),
    { v: 2, seq: 11, type: 'volume', data: { volume: 1 } },
    { v: 1, type: 'volume', data: { volume: 1 } },
    { v: 1, seq: 'x', type: 'volume', data: { volume: 1 } },
    null, undefined, 5, 'text', [], {},
  ]) {
    assert.equal(core.reduce(state, message), state, JSON.stringify(message));
  }
});

test('events the dashboard does not draw still advance the sequence', () => {
  const state = core.reduce(core.initialState(), hello(1));
  const next = core.reduce(state, envelope(2, 'play', { item: item(1), volume: 50 }));
  assert.equal(next.seq, 2);
  assert.equal(next.queue, state.queue);
});

test('notices are newest first, capped, and sanitized', () => {
  let state = core.initialState();
  for (let i = 1; i <= core.MAX_NOTICES + 5; i += 1) {
    state = core.reduce(state, envelope(i, 'notice', { level: 'warning', code: 'c', text: `n${i}`, sender: null }));
  }
  assert.equal(state.notices.length, core.MAX_NOTICES);
  assert.equal(state.notices[0].text, `n${core.MAX_NOTICES + 5}`);

  state = core.reduce(state, envelope(100, 'notice', { level: 'catastrophe', text: 5, sender: 7 }));
  assert.deepEqual(state.notices[0], { level: 'info', code: '', text: '', sender: null });
});

test('malformed queue data is cleaned, not trusted', () => {
  const state = core.reduce(core.initialState(), hello(1, {
    queue: {
      pending: [item(1), null, 'x', { id: 'a' }, { id: 2 }, { id: 3, video_id: 'abcdefghijk', requested_by: 5, title: 9 }],
      approved: 'nope',
      now_playing: { id: 'x' },
    },
  }));
  assert.deepEqual(state.queue.pending.map((i) => i.id), [1, 3]);
  assert.equal(state.queue.pending[1].title, '');
  assert.equal(state.queue.pending[1].requested_by.name, '');
  assert.deepEqual(state.queue.approved, []);
  assert.equal(state.queue.now_playing, null);
});

test('volume is clamped and junk is ignored', () => {
  let state = core.reduce(core.initialState(), hello(1));
  state = core.reduce(state, envelope(2, 'volume', { volume: 250 }));
  assert.equal(state.volume, 100);
  state = core.reduce(state, envelope(3, 'volume', { volume: 'loud' }));
  assert.equal(state.volume, 100);
  state = core.reduce(state, envelope(4, 'volume', { volume: -3 }));
  assert.equal(state.volume, 0);
});

test('the reducer never mutates the previous state', () => {
  const before = core.reduce(core.initialState(), hello(1));
  const snapshot = JSON.stringify(before);
  core.reduce(before, envelope(2, 'queue_updated', { pending: [], approved: [], now_playing: null }));
  core.reduce(before, envelope(3, 'notice', { level: 'info', text: 'x' }));
  core.reduce(before, hello(9));
  assert.equal(JSON.stringify(before), snapshot);
});
