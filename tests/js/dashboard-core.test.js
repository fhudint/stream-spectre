'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const core = require('../../static/dashboard/dashboard-core.js');

const root = path.join(__dirname, '../..');
const english = JSON.parse(fs.readFileSync(path.join(root, 'locales/en.json'), 'utf8'));
const indonesian = JSON.parse(fs.readFileSync(path.join(root, 'locales/id.json'), 'utf8'));

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
  moderation: { mode: 'manual', tiktok_min_role: 'follower', cooldown_secs: 60, volume: 50, max_volume: 80 },
  blacklist: { video_ids: [], channels: [], title_words: [], senders: [] },
  dashboard: { chat_history: 20 },
});
const line = (text, extra = {}) => ({ name: 'Budi', role: 'follower', text, command: false, skipped: 0, ...extra });
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

const goodRules = () => ({
  mode: 'auto', cooldown_secs: '60', max_duration_secs: '600', max_queue_len: '20', max_volume: '80',
  allow_live: true, allow_shorts: false, allowed_categories: ['gaming', 'music'],
  youtube_min_role: 'member', youtube_superchat_bypass: true, youtube_auto_approve: 'moderator',
  youtube_cooldown_secs: ' 10 ', youtube_max_waiting: '3',
  tiktok_min_role: 'subscriber', min_gifter_level: '3', min_fan_club_level: ' 0 ',
  tiktok_cooldown_secs: '', tiktok_max_waiting: '0',
});

test('readSettings accepts good values and names the first bad field', () => {
  assert.deepEqual(core.readSettings(goodRules()).value, {
    mode: 'auto', cooldown_secs: 60, max_duration_secs: 600, max_queue_len: 20, max_volume: 80,
    allow_live: true, allow_shorts: false, allowed_categories: ['music', 'gaming'],
    youtube_min_role: 'member', youtube_superchat_bypass: true, youtube_auto_approve: 'moderator',
    youtube_cooldown_secs: 10, youtube_max_waiting: 3,
    tiktok_min_role: 'subscriber', min_gifter_level: 3, min_fan_club_level: 0,
    tiktok_cooldown_secs: null, tiktok_max_waiting: 0,
  }, 'categories in the order of the list; an empty optional number is not set');
  for (const [key, bad] of [
    ['mode', 'sometimes'], ['tiktok_min_role', 'vip'], ['youtube_min_role', 'follower'], ['youtube_min_role', 'everyone'],
    ['youtube_auto_approve', 'vip'], ['cooldown_secs', ''], ['cooldown_secs', '-1'],
    ['max_queue_len', '1.5'], ['max_volume', 'loud'], ['min_gifter_level', '1e2'], ['max_duration_secs', '0x10'],
    ['youtube_cooldown_secs', '-5'], ['youtube_cooldown_secs', '1.5'], ['tiktok_max_waiting', 'many'],
    ['allow_live', 'true'], ['allow_shorts', 1], ['allowed_categories', 'music'], ['allowed_categories', ['cooking']],
    ['allowed_categories', null],
  ]) {
    assert.equal(core.readSettings({ ...goodRules(), [key]: bad }).error, key, `${key}=${bad}`);
  }
  const off = core.readSettings({ ...goodRules(), youtube_auto_approve: '' }).value;
  assert.equal(off.youtube_auto_approve, null, 'nobody is not set');
  assert.equal(core.readSettings({ ...goodRules(), tiktok_cooldown_secs: ' 0 ' }).value.tiktok_cooldown_secs, 0, '0 is a cooldown of none, not "not set"');
});

test('diffSettings compares lists and unset options', () => {
  const now = { mode: 'manual', allowed_categories: ['music'], youtube_cooldown_secs: 5 };
  assert.deepEqual(core.diffSettings(now, { mode: 'manual', allowed_categories: ['music'], youtube_cooldown_secs: 5 }), {});
  assert.deepEqual(core.diffSettings(now, { allowed_categories: ['music', 'gaming'] }), { allowed_categories: ['music', 'gaming'] });
  assert.deepEqual(core.diffSettings(now, { allowed_categories: [] }), { allowed_categories: [] });
  assert.deepEqual(core.diffSettings(now, { youtube_cooldown_secs: null }), { youtube_cooldown_secs: null }, 'clearing is a change');
  // A server leaves out what is not set.
  assert.deepEqual(core.diffSettings({ mode: 'manual' }, { tiktok_cooldown_secs: null }), {});
  assert.deepEqual(core.diffSettings({ mode: 'manual' }, { tiktok_cooldown_secs: 7 }), { tiktok_cooldown_secs: 7 });
  assert.deepEqual(core.diffSettings(null, { mode: 'auto' }), { mode: 'auto' });
});

test('every rule is on the tab of the rules it belongs to', () => {
  assert.deepEqual(core.RULES_TABS, ['general', 'youtube', 'tiktok']);
  const on = (tab) => core.SETTINGS_FIELDS.filter((f) => f.tab === tab).map((f) => f.key);
  assert.deepEqual(on('general'), ['mode', 'cooldown_secs', 'max_duration_secs', 'max_queue_len', 'max_volume', 'allow_live', 'allow_shorts', 'allowed_categories']);
  assert.deepEqual(on('youtube'), ['youtube_min_role', 'youtube_superchat_bypass', 'youtube_auto_approve', 'youtube_cooldown_secs', 'youtube_max_waiting']);
  assert.deepEqual(on('tiktok'), ['tiktok_min_role', 'min_gifter_level', 'min_fan_club_level', 'tiktok_cooldown_secs', 'tiktok_max_waiting']);
  for (const field of core.SETTINGS_FIELDS) assert.ok(core.RULES_TABS.includes(field.tab), field.key);
  const youtube = core.SETTINGS_FIELDS.find((f) => f.key === 'youtube_min_role');
  assert.deepEqual(youtube.options.map((o) => o.value), ['viewer', 'member', 'moderator']);
  assert.ok(youtube.options.every((o) => o.label.startsWith('role.youtube.')), 'a YouTube chat has its own words for the roles');
  const auto = core.SETTINGS_FIELDS.find((f) => f.key === 'youtube_auto_approve');
  assert.deepEqual(auto.options.map((o) => o.value), ['', 'viewer', 'member', 'moderator']);
  assert.ok(!core.SETTINGS_FIELDS.some((f) => f.key.startsWith('tiktok_auto')), 'TikTok has no automatic approval');
});

test('the categories are the server\'s, each with a name in both languages', () => {
  const server = fs.readFileSync(path.join(root, 'src/config/model.rs'), 'utf8');
  const block = server.split('pub enum VideoCategory {')[1].split('}')[0];
  const names = [...block.matchAll(/^\s+([A-Z][a-z]+),/gm)].map((m) => m[1].toLowerCase());
  assert.deepEqual(core.VIDEO_CATEGORIES, names);
  for (const name of names) {
    assert.ok(english[`dashboard.category.${name}`] && indonesian[`dashboard.category.${name}`], name);
  }
});

test('the settings form edits real configuration keys', () => {
  const example = fs.readFileSync(path.join(root, 'config.example.toml'), 'utf8');
  const section = example.split('[moderation]')[1].split(/\n\[/)[0];
  // A setting that is not set by default is in the example as a comment.
  const keys = [...section.matchAll(/^#?\s*([a-z_]+)\s*=/gm)].map((m) => m[1]);
  for (const field of core.SETTINGS_FIELDS) assert.ok(keys.includes(field.key), field.key);
  assert.ok(keys.includes('volume'), 'volume has its own slider');

  const blacklist = example.split('[blacklist]')[1];
  for (const field of core.BLACKLIST_FIELDS) assert.match(blacklist, new RegExp(`^${field.key}\\s*=`, 'm'));
});

test('readChatHistory accepts whole numbers up to the limit only', () => {
  assert.equal(core.readChatHistory('20'), 20);
  assert.equal(core.readChatHistory(' 0 '), 0);
  assert.equal(core.readChatHistory(String(core.MAX_CHAT_HISTORY)), core.MAX_CHAT_HISTORY);
  for (const bad of ['', ' ', '-1', '1.5', '1e2', '0x10', 'many', String(core.MAX_CHAT_HISTORY + 1), '99999999999999999999']) {
    assert.equal(core.readChatHistory(bad), null, bad);
  }
});

test('the chat history setting is a real configuration key', () => {
  const example = fs.readFileSync(path.join(root, 'config.example.toml'), 'utf8');
  assert.match(example.split('[dashboard]')[1], /^chat_history\s*=/m);
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

// --- chat ---

test('hello replaces the chat lines, even when the page already had some', () => {
  let state = core.reduce(core.initialState(), hello(1, { chat_log: [line('one'), line('two')] }));
  assert.deepEqual(state.messages.map((m) => m.text), ['one', 'two']);
  state = core.reduce(state, envelope(2, 'chat_message', line('three')));
  assert.equal(state.messages.length, 3);

  state = core.reduce(state, hello(10, { chat_log: [line('fresh')] }));
  assert.deepEqual(state.messages.map((m) => m.text), ['fresh']);
  state = core.reduce(state, hello(11, {}));
  assert.deepEqual(state.messages, [], 'a hello without a log means an empty log');
});

test('chat messages are appended in order, with the flags kept', () => {
  let state = core.reduce(core.initialState(), hello(1));
  state = core.reduce(state, envelope(2, 'chat_message', line('hi')));
  state = core.reduce(state, envelope(3, 'chat_message', line('!v dQw4w9WgXcQ', { command: true, skipped: 4, role: 'moderator' })));
  assert.deepEqual(state.messages[1], {
    name: 'Budi', role: 'moderator', source: 'tiktok', text: '!v dQw4w9WgXcQ', at_ms: 0, command: true, gift: false, developer: false, skipped: 4,
  });
  assert.equal(state.seq, 3);
  state = core.reduce(state, envelope(4, 'chat_message', line('sent 5 \u00d7 Rose', { gift: true })));
  assert.equal(state.messages[2].gift, true, 'a gift is marked');
  state = core.reduce(state, envelope(5, 'chat_message', line('x', { gift: 'yes' })));
  assert.equal(state.messages[3].gift, false, 'only true counts');
  state = core.reduce(state, envelope(6, 'chat_message', line('x', { developer: true })));
  assert.equal(state.messages[4].developer, true, 'the developer is marked');
  state = core.reduce(state, envelope(7, 'chat_message', line('x', { developer: 'yes' })));
  assert.equal(state.messages[5].developer, false, 'only true counts');
});

test('chat lines are capped at the newest ones', () => {
  let state = core.reduce(core.initialState(), hello(1));
  for (let i = 2; i < core.MAX_CHAT_LINES + 12; i += 1) {
    state = core.reduce(state, envelope(i, 'chat_message', line(`m${i}`)));
  }
  assert.equal(state.messages.length, core.MAX_CHAT_LINES);
  assert.equal(state.messages[core.MAX_CHAT_LINES - 1].text, `m${core.MAX_CHAT_LINES + 11}`);
  assert.equal(state.messages[0].text, 'm12');

  const big = Array.from({ length: core.MAX_CHAT_LINES + 50 }, (_, i) => line(`h${i}`));
  state = core.reduce(state, hello(999, { chat_log: big }));
  assert.equal(state.messages.length, core.MAX_CHAT_LINES);
  assert.equal(state.messages[core.MAX_CHAT_LINES - 1].text, `h${core.MAX_CHAT_LINES + 49}`);
});

test('chat lines are sanitized and junk is ignored', () => {
  let state = core.reduce(core.initialState(), hello(1));
  state = core.reduce(state, envelope(2, 'chat_message', { name: 7, role: {}, text: 'ok', command: 'yes', skipped: -3 }));
  assert.deepEqual(state.messages[0], { name: '', role: 'everyone', source: 'tiktok', text: 'ok', at_ms: 0, command: false, gift: false, developer: false, skipped: 0 });
  state = core.reduce(state, envelope(3, 'chat_message', { name: 'x' }));
  state = core.reduce(state, envelope(4, 'chat_message', 'text'));
  state = core.reduce(state, envelope(5, 'chat_message', { name: 'x', text: 5 }));
  assert.equal(state.messages.length, 1);
  state = core.reduce(state, hello(6, { chat_log: [null, 'x', { text: 'kept' }, { name: 'n' }] }));
  assert.deepEqual(state.messages.map((m) => m.text), ['kept']);
  state = core.reduce(state, hello(7, { chat_log: 'nope' }));
  assert.deepEqual(state.messages, []);
});

test('a chat line already counted in a hello is not added twice', () => {
  let state = core.reduce(core.initialState(), hello(10, { chat_log: [line('known')] }));
  state = core.reduce(state, envelope(10, 'chat_message', line('known')));
  state = core.reduce(state, envelope(9, 'chat_message', line('older')));
  assert.equal(state.messages.length, 1);
});

test('settings_updated keeps the dashboard settings for the form', () => {
  let state = core.reduce(core.initialState(), hello(1));
  assert.equal(state.settings.dashboard.chat_history, 20);
  const changed = { ...settings(), dashboard: { chat_history: 5 } };
  state = core.reduce(state, envelope(2, 'settings_updated', changed));
  assert.equal(state.settings.dashboard.chat_history, 5);
});

// --- the redesigned page: counters, progress, waiting times and so on ---

const TIMES = { seconds: '{seconds} s', minutes_seconds: '{minutes} min {seconds} s', hours_minutes: '{hours} h {minutes} min' };

test('requests and chat lines keep when they were made, and junk becomes zero', () => {
  let state = core.reduce(core.initialState(), hello(1, { queue: { pending: [item(1, { requested_at_ms: 1700000000123.9 })], approved: [item(2, { requested_at_ms: -5 })], now_playing: item(3, { requested_at_ms: 'soon' }) } }));
  assert.equal(state.queue.pending[0].requested_at_ms, 1700000000123);
  assert.equal(state.queue.approved[0].requested_at_ms, 0);
  assert.equal(state.queue.now_playing.requested_at_ms, 0);
  state = core.reduce(state, envelope(2, 'chat_message', line('hi', { at_ms: 1700000000999 })));
  state = core.reduce(state, envelope(3, 'chat_message', line('hi', { at_ms: 'x' })));
  assert.deepEqual(state.messages.map((m) => m.at_ms), [1700000000999, 0]);
});

test('the counters come with hello and stats_updated, and bad values are zero', () => {
  let state = core.reduce(core.initialState(), hello(1));
  assert.deepEqual(state.stats, { since_ms: 0, received: 0, approved: 0, rejected: 0 });
  state = core.reduce(state, hello(2, { stats: { since_ms: 5, received: 4, approved: 3, rejected: 1 } }));
  assert.deepEqual(state.stats, { since_ms: 5, received: 4, approved: 3, rejected: 1 });
  state = core.reduce(state, envelope(3, 'stats_updated', { since_ms: 5, received: 9, approved: 6, rejected: 2 }));
  assert.equal(state.stats.received, 9);
  state = core.reduce(state, envelope(4, 'stats_updated', { since_ms: 'x', received: -1, approved: null, rejected: 1.9 }));
  assert.deepEqual(state.stats, { since_ms: 0, received: 0, approved: 0, rejected: 1 });
  // An event the snapshot already covers is ignored.
  const old = core.reduce(state, envelope(4, 'stats_updated', { received: 100 }));
  assert.equal(old, state);
});

const request = (id, extra = {}) => ({
  id,
  source: 'youtube',
  video_id: 'dQw4w9WgXcQ',
  title: `Video ${id}`,
  channel: 'Channel',
  requested_by: 'Budi',
  requested_at_ms: 5,
  status: 'waiting',
  reason: '',
  ...extra,
});

test('each chat has its own status: from hello, or from events that name their source', () => {
  const tiktok = { source: 'tiktok', state: 'connected', channel: 'a', text: 'T' };
  const youtube = { source: 'youtube', state: 'waiting_for_live', channel: 'v', text: 'Y' };
  let state = core.reduce(core.initialState(), hello(1, { chat: tiktok, chats: [tiktok, youtube] }));
  assert.deepEqual(state.chats, { tiktok, youtube });
  assert.equal(state.chat, tiktok, 'the TikTok one, as before');

  // A server that only knew TikTok sent just `chat`.
  state = core.reduce(core.initialState(), hello(1, { chat: { state: 'connected', text: 'old' } }));
  assert.equal(state.chats.tiktok.text, 'old');
  assert.equal(state.chats.youtube, null);

  state = core.reduce(state, envelope(2, 'chat_status', { ...youtube, state: 'connected', text: 'Y2' }));
  assert.equal(state.chats.youtube.text, 'Y2');
  assert.equal(state.chats.tiktok.text, 'old', 'the other chat is left alone');
  assert.equal(state.chat.text, 'old');
  state = core.reduce(state, envelope(3, 'chat_status', { state: 'reconnecting', text: 'no source' }));
  assert.equal(state.chats.tiktok.text, 'no source', 'no source means TikTok');
  assert.equal(state.chat.text, 'no source');
  const before = state.chats;
  state = core.reduce(state, envelope(4, 'chat_status', 'nonsense'));
  assert.equal(state.chats, before);
  // A source that is not one of ours is nobody's status.
  state = core.reduce(core.initialState(), hello(1, { chat: null, chats: [{ source: 'twitch', text: 'x' }, null, 5] }));
  assert.deepEqual(state.chats, { tiktok: null, youtube: null });
});

test('the history comes with hello, and history_entry adds a request or updates it', () => {
  let state = core.reduce(core.initialState(), hello(1));
  assert.deepEqual(state.history, []);
  state = core.reduce(state, hello(2, { history: [request(1), request(2)] }));
  assert.deepEqual(state.history.map((e) => e.id), [1, 2]);
  assert.equal(state.history[0].thumbnail_url, 'https://i.ytimg.com/vi/dQw4w9WgXcQ/mqdefault.jpg');

  state = core.reduce(state, envelope(3, 'history_entry', request(3)));
  state = core.reduce(state, envelope(4, 'history_entry', request(1, { status: 'played' })));
  assert.deepEqual(state.history.map((e) => `${e.id}:${e.status}`), ['1:played', '2:waiting', '3:waiting']);

  // Garbage changes nothing; an event the snapshot covers is ignored.
  const before = state.history;
  state = core.reduce(state, envelope(5, 'history_entry', { id: 'x' }));
  assert.equal(state.history, before);
  state = core.reduce(state, envelope(5, 'history_entry', request(9)));
  assert.equal(state.history.length, 3);
});

test('a request is cleaned up: unknown status and source, no picture for TikTok, a capped list', () => {
  const state = core.reduce(core.initialState(), hello(1, { history: [request(1, { status: 'odd', source: 'tiktok', video_id: '7700124541145599252' }), request(2, { video_id: '<x>' })] }));
  assert.equal(state.history[0].status, 'refused');
  assert.equal(state.history[0].thumbnail_url, '');
  assert.equal(state.history[1].thumbnail_url, '', 'only a real ID builds a picture address');
  let history = [];
  for (let id = 1; id <= core.MAX_HISTORY + 2; id += 1) {
    history = core.reduce({ ...core.initialState(), history, seq: id - 1 }, envelope(id, 'history_entry', request(id))).history;
  }
  assert.equal(history.length, core.MAX_HISTORY);
  assert.equal(history[0].id, 3);
});

test('what can be blacklisted: the video when its ID is acceptable, the channel or account when known', () => {
  const none = { video_ids: [], channels: [], title_words: [] };
  const labels = (item, lists = none) => core.blacklistChoices(item, lists).map((c) => `${c.key}=${c.value}:${c.listed}`);
  assert.deepEqual(labels(request(1)), ['video_ids=dQw4w9WgXcQ:false', 'channels=Channel:false']);
  // Already listed (a channel matches ignoring case, an ID exactly).
  assert.deepEqual(
    labels(request(1), { video_ids: ['dQw4w9WgXcQ'], channels: [' channel '] }),
    ['video_ids=dQw4w9WgXcQ:true', 'channels=Channel:true']
  );
  assert.deepEqual(labels(request(1), { video_ids: ['dqw4w9wgxcq'] })[0], 'video_ids=dQw4w9WgXcQ:false');
  // TikTok: the account is what follows the @ in the title.
  const tiktok = { source: 'tiktok', video_id: '7700124541145599252', title: '@cherlynn052', channel: 'TikTok' };
  assert.deepEqual(labels(tiktok), ['video_ids=7700124541145599252:false', 'channels=cherlynn052:false']);
  assert.deepEqual(core.blacklistChoices(tiktok, none)[1].label, 'dashboard.action.blacklist_account');
  // A short code and an unknown account give nothing to add; so does a YouTube lookalike on TikTok.
  assert.deepEqual(labels({ source: 'tiktok', video_id: 'ZSpGELffb', title: '', channel: '' }), []);
  assert.deepEqual(labels({ source: 'tiktok', video_id: 'dQw4w9WgXcQ', title: '7700124541145599252', channel: 'TikTok' }), []);
  assert.deepEqual(labels(request(1, { channel: '' })), ['video_ids=dQw4w9WgXcQ:false']);
  assert.equal(core.blacklistChoices(request(1), undefined).length, 2, 'before the settings are known');
});

test('a request links to its video: YouTube, TikTok with or without the account, or a short link', () => {
  const link = (extra) => core.videoLink({ source: 'youtube', video_id: 'dQw4w9WgXcQ', title: '', channel: '', ...extra });
  assert.equal(link({}), 'https://www.youtube.com/watch?v=dQw4w9WgXcQ');
  const tiktok = { source: 'tiktok', video_id: '7700124541145599252' };
  assert.equal(link({ ...tiktok, title: '@cher.lynn_05' }), 'https://www.tiktok.com/@cher.lynn_05/video/7700124541145599252');
  // Account unknown, or not a name we would put in an address: any name opens the right video.
  assert.equal(link({ ...tiktok, title: tiktok.video_id }), 'https://www.tiktok.com/@_/video/7700124541145599252');
  assert.equal(link({ ...tiktok, title: '@a/../b?x=1' }), 'https://www.tiktok.com/@_/video/7700124541145599252');
  // Turned away before the short link was opened: the code is all there is.
  assert.equal(link({ source: 'tiktok', video_id: 'ZSpGELffb' }), 'https://vt.tiktok.com/ZSpGELffb/');
  for (const bad of [{ video_id: '<x>' }, { video_id: 'short' }, { video_id: 'a/b/c/d/e' }, { source: 'tiktok', video_id: '../x' }, { source: 'tiktok', video_id: 'abc' }]) {
    assert.equal(link(bad), '', JSON.stringify(bad));
  }
});

test('a request shows its time today, and the date too on another day', () => {
  const at = new Date(2026, 0, 5, 19, 51, 24).getTime();
  const sameDay = new Date(2026, 0, 5, 23, 0, 0).getTime();
  const later = new Date(2026, 0, 7, 8, 0, 0).getTime();
  assert.equal(core.formatWhen(at, sameDay), '19:51:24');
  assert.equal(core.formatWhen(at, later), '2026-01-05 19:51:24');
  assert.equal(core.formatWhen(0, later), '');
});

test('playback reports are kept, checked, and dropped by a new hello', () => {
  let state = core.reduce(core.initialState(), hello(1, { queue: { pending: [], approved: [], now_playing: item(7) } }));
  assert.equal(state.playback, null);
  state = core.reduce(state, envelope(2, 'playback', { item_id: 7, position_secs: 42 }));
  assert.deepEqual(state.playback, { item_id: 7, position_secs: 42, duration_secs: 0 });
  state = core.reduce(state, envelope(3, 'playback', { item_id: 7, position_secs: 43, duration_secs: 213 }));
  assert.deepEqual(state.playback, { item_id: 7, position_secs: 43, duration_secs: 213 });
  // A length that is not a whole number above zero is no length.
  for (const odd of [0, -4, 1.5, '20', null]) {
    state = core.reduce(state, envelope(state.seq + 1, 'playback', { item_id: 7, position_secs: 44, duration_secs: odd }));
    assert.equal(state.playback.duration_secs, 0, String(odd));
  }
  const before = state.playback;
  for (const bad of [{ item_id: '7', position_secs: 1 }, { item_id: 7, position_secs: -1 }, { item_id: 7, position_secs: 1.5 }, { item_id: 7 }, null]) {
    state = core.reduce(state, envelope(state.seq + 1, 'playback', bad));
    assert.deepEqual(state.playback, before, JSON.stringify(bad));
  }
  state = core.reduce(state, hello(50, { queue: { pending: [], approved: [], now_playing: item(7) } }));
  assert.equal(state.playback, null);
});

test('the position shown is only for the video that is playing, and never past its end', () => {
  const base = core.reduce(core.initialState(), hello(1, { queue: { pending: [], approved: [], now_playing: item(7) } }));
  assert.equal(core.playbackPosition(base), 0, 'nothing reported yet');
  const at = (p) => core.reduce(base, envelope(2, 'playback', p));
  assert.equal(core.playbackPosition(at({ item_id: 7, position_secs: 60 })), 60);
  assert.equal(core.playbackPosition(at({ item_id: 8, position_secs: 60 })), 0, 'another video');
  assert.equal(core.playbackPosition(at({ item_id: 7, position_secs: 9999 })), 213, 'the length of the video');
  const idle = core.reduce(core.initialState(), hello(1));
  assert.equal(core.playbackPosition(idle), 0);
});

test('waiting times are measured on the given clock', () => {
  const a = { requested_at_ms: 1_000_000 };
  assert.equal(core.waitingSeconds(a, 1_252_900), 252);
  assert.equal(core.waitingSeconds(a, 900_000), 0, 'a clock that is behind never gives a negative time');
  assert.equal(core.waitingSeconds({ requested_at_ms: 0 }, 5), null, 'unknown time');
  assert.equal(core.waitingSeconds(null, 5), null);
  const items = [{ requested_at_ms: 1_200_000 }, { requested_at_ms: 1_000_000 }, { requested_at_ms: 0 }];
  assert.equal(core.oldestWaiting(items, 1_300_000), 300);
  assert.equal(core.oldestWaiting([{ requested_at_ms: 0 }], 1), null);
  assert.equal(core.oldestWaiting([], 1), null);
});

test('formatWaiting uses the right template for seconds, minutes and hours', () => {
  assert.equal(core.formatWaiting(0, TIMES), '0 s');
  assert.equal(core.formatWaiting(59, TIMES), '59 s');
  assert.equal(core.formatWaiting(60, TIMES), '1 min 0 s');
  assert.equal(core.formatWaiting(252, TIMES), '4 min 12 s');
  assert.equal(core.formatWaiting(3599, TIMES), '59 min 59 s');
  assert.equal(core.formatWaiting(3600, TIMES), '1 h 0 min');
  assert.equal(core.formatWaiting(7384, TIMES), '2 h 3 min');
  assert.equal(core.formatWaiting(-5, TIMES), '0 s');
  assert.equal(core.formatWaiting('oops', TIMES), '0 s');
});

test('a viewer always gets the same colour, within the palette', () => {
  const seen = new Set();
  for (const name of ['Budi', '@minho_k', 'Sari', '', 'x'.repeat(500), '\u{1F600}']) {
    const tint = core.tintIndex(name);
    assert.ok(Number.isInteger(tint) && tint >= 0 && tint < core.TINTS, name);
    assert.equal(core.tintIndex(name), tint);
    seen.add(tint);
  }
  assert.ok(seen.size > 1, 'the names are not all the same colour');
});

test('the avatar letter skips @, keeps whole characters, and has a fallback', () => {
  assert.equal(core.initial('@minho_k'), 'M');
  assert.equal(core.initial('budi'), 'B');
  assert.equal(core.initial('  sari'), 'S');
  assert.equal(core.initial('\u{1F600} happy'), '\u{1F600}');
  for (const none of ['', '   ', '@', '@@']) assert.equal(core.initial(none), '?', JSON.stringify(none));
});

test('the clock time is HH:MM:SS in local time, or empty when unknown', () => {
  assert.equal(core.formatClock(new Date(2026, 0, 5, 19, 51, 24).getTime()), '19:51:24');
  assert.equal(core.formatClock(new Date(2026, 0, 5, 3, 4, 5).getTime()), '03:04:05');
  assert.equal(core.formatClock(0), '');
  assert.equal(core.formatClock(NaN), '');
  assert.equal(core.formatClock(1e20), '');
});

test('the rules in force are listed as short texts', () => {
  const say = (key, values) => `${key}${JSON.stringify(values)}`;
  const moderation = { mode: 'manual', tiktok_min_role: 'subscriber', youtube_min_role: 'viewer', min_gifter_level: 0, min_fan_club_level: 0, cooldown_secs: 90, max_duration_secs: 600, max_queue_len: 20 };
  const lists = { video_ids: ['a', 'b'], channels: ['c'], title_words: [] };
  assert.deepEqual(core.ruleChips({ moderation, blacklist: lists }, say), [
    'dashboard.rules.youtube_everyone{}',
    'dashboard.rules.tiktok_role{"role":"role.subscriber{}"}',
    'dashboard.rules.cooldown{"seconds":90}',
    'dashboard.rules.max_duration{"duration":"10:00"}',
    'dashboard.rules.max_queue{"count":20}',
    'dashboard.rules.blocked{"count":3}',
  ]);
  const all = core.ruleChips({ moderation: { ...moderation, mode: 'auto', youtube_min_role: 'member', tiktok_min_role: 'everyone', min_gifter_level: 3, min_fan_club_level: 2, cooldown_secs: 0 }, blacklist: { video_ids: [], channels: [], title_words: [] } }, say);
  assert.deepEqual(all, [
    'dashboard.rules.youtube_role{"role":"role.youtube.subscriber{}"}',
    'dashboard.rules.tiktok_everyone{}',
    'dashboard.rules.mode_auto{}',
    'dashboard.rules.gifter{"level":3}',
    'dashboard.rules.fan_club{"level":2}',
    'dashboard.rules.no_cooldown{}',
    'dashboard.rules.max_duration{"duration":"10:00"}',
    'dashboard.rules.max_queue{"count":20}',
  ]);
  assert.deepEqual(core.ruleChips(null, say), []);
  assert.deepEqual(core.ruleChips({}, say), []);
});

test('the address bar picks a page, and anything else means the queue', () => {
  for (const view of core.VIEWS) assert.equal(core.parseView('#' + view), view);
  assert.deepEqual(core.VIEWS, ['queue', 'chat', 'requests', 'blacklist', 'sources', 'rules', 'overlay', 'settings', 'account']);
  // The account dialog is reached from the avatar, so it is not in the side menu.
  assert.deepEqual(core.MENU_VIEWS, ['queue', 'chat', 'requests', 'blacklist', 'sources', 'rules', 'overlay', 'settings']);
  // What used to be tabs of the account dialog is the Sources page; old addresses lead there.
  for (const part of ['tiktok', 'youtube', 'youtube-chat']) assert.equal(core.parseView('#account/' + part), 'sources', part);
  assert.equal(core.parseView('#account/token'), 'account');
  assert.equal(core.parseView('#queue/anything'), 'queue');
  for (const other of ['', '#', '#token=abc', '#nope', '#QUEUE', '#queue?x=1', undefined, null, '#__proto__', '#constructor']) {
    assert.equal(core.parseView(other), 'queue', String(other));
  }
});

test('moving the choice stays inside the list', () => {
  const ids = [10, 20, 30];
  assert.equal(core.selectAfter(ids, 10, 1), 20);
  assert.equal(core.selectAfter(ids, 20, 1), 30);
  assert.equal(core.selectAfter(ids, 30, 1), 30, 'stops at the end');
  assert.equal(core.selectAfter(ids, 10, -1), 10, 'stops at the start');
  assert.equal(core.selectAfter(ids, 99, 1), 10, 'an unknown choice starts at the top');
  assert.equal(core.selectAfter(ids, null, -1), 10);
  assert.equal(core.selectAfter([], 10, 1), null);
});

test('only plain presses of the shortcut keys are commands', () => {
  const press = (key, extra = {}) => core.keyCommand({ key, ...extra });
  assert.equal(press('ArrowUp'), 'up');
  assert.equal(press('ArrowDown'), 'down');
  for (const key of ['a', 'A']) assert.equal(press(key), 'approve');
  for (const key of ['r', 'R']) assert.equal(press(key), 'reject');
  for (const key of ['s', 'S']) assert.equal(press(key), 'skip');
  for (const key of ['p', 'P']) assert.equal(press(key), 'pause');
  for (const key of ['a', 'r', 's', 'ArrowDown']) {
    for (const modifier of ['ctrlKey', 'metaKey', 'altKey']) {
      assert.equal(press(key, { [modifier]: true }), null, `${modifier}+${key}`);
    }
  }
  for (const key of ['x', 'Enter', ' ', 'Tab', 'ArrowLeft', '']) assert.equal(press(key), null, key);
  assert.equal(core.keyCommand(null), null);
});

test('the overlay address uses localhost for a loopback IP and carries the token', () => {
  assert.equal(core.overlayUrl('http:', '127.0.0.1:8787', ''), 'http://localhost:8787/overlay/mediashare/');
  assert.equal(core.overlayUrl('http:', '[::1]:8787', ''), 'http://localhost:8787/overlay/mediashare/');
  assert.equal(core.overlayUrl('http:', '127.0.0.1', ''), 'http://localhost/overlay/mediashare/');
  assert.equal(core.overlayUrl('http:', 'localhost:8787', ''), 'http://localhost:8787/overlay/mediashare/');
  assert.equal(core.overlayUrl('https:', 'stream.example:8443', 'a b&c'), 'https://stream.example:8443/overlay/mediashare/?token=a%20b%26c');
  // Only a bare loopback host is rewritten, never something that merely starts like one.
  assert.equal(core.overlayUrl('http:', '127.0.0.10:8787', ''), 'http://127.0.0.10:8787/overlay/mediashare/');
  assert.equal(core.overlayUrl('http:', '127.0.0.1.evil.example', ''), 'http://127.0.0.1.evil.example/overlay/mediashare/');
});

test('the username is read like the server reads it', () => {
  for (const [typed, value] of [['name', 'name'], ['  @name ', 'name'], ['@@name', 'name'], ['a.b_c1', 'a.b_c1'], ['', ''], ['   ', ''], ['x'.repeat(64), 'x'.repeat(64)]]) {
    assert.deepEqual(core.readTiktokUsername(typed), { value }, JSON.stringify(typed));
  }
  for (const bad of ['@', ' @ ', '@@', 'a/b', 'a b', 'a?b', 'a#b', 'héllo', 'x'.repeat(65), '@x y', 'name\n2', 'https://www.tiktok.com/@name']) {
    assert.deepEqual(core.readTiktokUsername(bad), { error: true }, JSON.stringify(bad));
  }
  assert.deepEqual(core.readTiktokUsername(5), { value: '5' });
});

test('the part of the account page is read from the address, and only for the account page', () => {
  for (const part of core.ACCOUNT_SECTIONS) assert.equal(core.parseSection('#account/' + part), part);
  assert.deepEqual(core.ACCOUNT_SECTIONS, ['profile', 'tiktok', 'youtube-chat', 'youtube', 'token']);
  for (const other of ['#account', '#account/', '#account/nope', '#account/youtube/x', '#queue/youtube', '#youtube', '', undefined, null, '#account/__proto__']) {
    assert.equal(core.parseSection(other), '', String(other));
  }
});

test('the key is read like the server reads it', () => {
  assert.deepEqual(core.readYoutubeKey('  AIzaSyA-123_abc  '), { value: 'AIzaSyA-123_abc' });
  assert.deepEqual(core.readYoutubeKey('x'.repeat(200)), { value: 'x'.repeat(200) });
  for (const empty of ['', '   ', '\n\t ']) assert.deepEqual(core.readYoutubeKey(empty), { error: 'empty' });
  for (const bad of ['two words', 'line\nbreak', 'tab\there', 'café', '\u0000', 'a b', 'x'.repeat(201), '​hidden']) {
    assert.deepEqual(core.readYoutubeKey(bad), { error: 'format' }, JSON.stringify(bad));
  }
});

test('the pause comes with hello and playback_paused, and any new video or empty screen clears it', () => {
  let state = core.reduce(core.initialState(), hello(1, { playback_paused: true, queue: { pending: [], approved: [], now_playing: item(7) } }));
  assert.equal(state.paused, true);
  state = core.reduce(state, envelope(2, 'playback_paused', { item_id: 7, paused: false }));
  assert.equal(state.paused, false);
  state = core.reduce(state, envelope(3, 'playback_paused', { item_id: 7, paused: true }));
  assert.equal(state.paused, true);
  for (const type of ['play', 'skip', 'stop']) {
    let s = core.reduce(core.initialState(), hello(1, { playback_paused: true }));
    assert.equal(s.paused, true);
    s = core.reduce(s, envelope(2, type, type === 'play' ? { item: item(8), volume: 50 } : { item_id: 7 }));
    assert.equal(s.paused, false, type);
  }
  // Missing or odd values mean "not paused".
  for (const junk of [undefined, null, 'yes', 1, {}]) {
    assert.equal(core.reduce(core.initialState(), hello(1, { playback_paused: junk })).paused, false, String(junk));
  }
  assert.equal(core.reduce(state, envelope(4, 'playback_paused', { paused: 'yes' })).paused, false);
  // An event the snapshot already covers is ignored.
  assert.equal(core.reduce(state, envelope(3, 'playback_paused', { item_id: 7, paused: false })), state);
});

test('the commands form is read the way the server will take it', () => {
  const row = (name, source = 'youtube') => ({ name, source });
  assert.deepEqual(core.readCommands({ prefix: '!', commands: [row('v')] }), { value: { prefix: '!', commands: [row('v')] } });
  // Spaces around are dropped, and so is a prefix typed in front of a word.
  assert.deepEqual(core.readCommands({ prefix: ' ? ', commands: [row(' ?vt '), row('yt')] }), {
    value: { prefix: '?', commands: [row('vt'), row('yt')] },
  });
  assert.deepEqual(core.readCommands({ prefix: '!?', commands: [row('a_1')] }).value.prefix, '!?');
  assert.deepEqual(core.readCommands({ prefix: '😀', commands: [row('v')] }).value.prefix, '😀');

  for (const prefix of ['', '   ', 'a b', '12345', 'a\tb', 'a\u0000']) {
    assert.deepEqual(core.readCommands({ prefix, commands: [row('v')] }), { error: 'prefix' }, JSON.stringify(prefix));
  }
  assert.deepEqual(core.readCommands({ prefix: '!', commands: [] }), { error: 'count' });
  assert.deepEqual(core.readCommands({ prefix: '!', commands: undefined }), { error: 'count' });
  const nine = Array.from({ length: 9 }, (_, n) => row('c' + n));
  assert.deepEqual(core.readCommands({ prefix: '!', commands: nine }), { error: 'count' });
  assert.equal(core.readCommands({ prefix: '!', commands: nine.slice(0, 8) }).value.commands.length, 8);
  assert.equal(core.MAX_COMMANDS, 8);

  for (const name of ['', '  ', 'two words', 'vídeo', 'v!', 'x'.repeat(33), '!', '!!v']) {
    assert.deepEqual(core.readCommands({ prefix: '!', commands: [row('ok'), row(name)] }), { error: 'name', index: 1 }, JSON.stringify(name));
  }
  assert.equal(core.readCommands({ prefix: '!', commands: [row('x'.repeat(32))] }).value.commands[0].name.length, 32);
  assert.deepEqual(core.readCommands({ prefix: '!', commands: [row('v', 'twitch')] }), { error: 'source', index: 0 });
  assert.deepEqual(core.readCommands({ prefix: '!', commands: [row('yt'), row('v'), row('YT')] }), { error: 'duplicate', index: 2 });
  // Typed with the prefix, it is the same word.
  assert.deepEqual(core.readCommands({ prefix: '!', commands: [row('v'), row('!V')] }), { error: 'duplicate', index: 1 });
  // A name that only starts like another is fine.
  assert.ok(core.readCommands({ prefix: '!', commands: [row('v'), row('vt')] }).value);
});

test('the commands are shown the way viewers type them', () => {
  assert.deepEqual(core.commandWords({ prefix: '!', commands: [{ name: 'v' }, { name: 'yt' }] }), ['!v', '!yt']);
  assert.deepEqual(core.commandWords(null), []);
  assert.deepEqual(core.commandWords({ prefix: '!' }), []);
  const say = (key, values) => `${key}${JSON.stringify(values)}`;
  const moderation = { mode: 'manual', min_role: 'everyone', min_gifter_level: 0, min_fan_club_level: 0, cooldown_secs: 0, max_duration_secs: 600, max_queue_len: 20 };
  const chips = core.ruleChips({ moderation, blacklist: {}, command: { prefix: '!', commands: [{ name: 'v' }, { name: 'vt' }] } }, say);
  assert.equal(chips[0], 'dashboard.rules.commands{"list":"!v !vt"}');
});

test('a TikTok video has a position once the overlay has told its length, the server\'s length wins otherwise', () => {
  const state = (source, duration, playback) => ({
    queue: { now_playing: { id: 3, source, duration_secs: duration } },
    playback: { item_id: 3, position_secs: 40, duration_secs: 0, ...playback },
  });
  assert.equal(core.playbackPosition(state('youtube', 213)), 40);
  assert.equal(core.playbackPosition(state('youtube', 30)), 30, 'never past its length');
  assert.equal(core.playbackDuration(state('youtube', 213)), 213);
  // TikTok: nothing until its player has said how long the video is.
  assert.equal(core.playbackPosition(state('tiktok', 0)), 0);
  assert.equal(core.playbackDuration(state('tiktok', 0)), 0);
  assert.equal(core.playbackPosition(state('tiktok', 0, { duration_secs: 60 })), 40);
  assert.equal(core.playbackPosition(state('tiktok', 0, { duration_secs: 15 })), 15, 'never past its length');
  assert.equal(core.playbackDuration(state('tiktok', 0, { duration_secs: 15 })), 15);
  // What the player says is the length of the video it plays: it wins over the queue item's.
  assert.equal(core.playbackDuration(state('youtube', 213, { duration_secs: 91 })), 91);
  assert.equal(core.playbackPosition(state('youtube', 213, { position_secs: 100, duration_secs: 91 })), 91);
  // A length told for another video does not replace this one's.
  assert.equal(core.playbackDuration(state('youtube', 213, { item_id: 4, duration_secs: 91 })), 213);
  // A length told for another video is not this one's.
  assert.equal(core.playbackDuration(state('tiktok', 0, { item_id: 4, duration_secs: 15 })), 0);
  assert.equal(core.playbackDuration({ queue: { now_playing: null }, playback: null }), 0);
});


test('the look of the overlay is read from the settings, the first when there is none', () => {
  assert.equal(core.OVERLAY_THEMES.length, 10);
  assert.equal(core.overlayTheme({ overlay: { mediashare: { theme: 'pixel' } } }), 'pixel');
  for (const bad of [undefined, null, {}, { overlay: null }, { overlay: {} }, { overlay: { mediashare: {} } }, { overlay: { theme: 'pixel' } }, { overlay: { mediashare: { theme: 'neon' } } }, { overlay: { mediashare: { theme: 3 } } }]) {
    assert.equal(core.overlayTheme(bad), 'obsidian', JSON.stringify(bad));
  }
});

test('a message in an item or a request is kept only when it is text', () => {
  const item = { id: 1, video_id: 'dQw4w9WgXcQ' };
  const queue = (message) => core.reduce(core.initialState(), { v: 1, seq: 1, type: 'hello', data: { queue: { pending: [{ ...item, message }], approved: [], now_playing: null }, history: [{ ...item, source: 'youtube', status: 'played', message }] } });
  const good = queue('halo ka');
  assert.equal(good.queue.pending[0].message, 'halo ka');
  assert.equal(good.history[0].message, 'halo ka');
  for (const bad of [5, null, {}, undefined]) {
    const state = queue(bad);
    assert.equal(state.queue.pending[0].message, '');
    assert.equal(state.history[0].message, '');
  }
});

test('a role is named as the chat it was said in names it', () => {
  assert.equal(core.roleKey('follower', 'youtube'), 'role.youtube.follower');
  assert.equal(core.roleKey('subscriber', 'youtube'), 'role.youtube.subscriber');
  assert.equal(core.roleKey('moderator', 'youtube'), 'role.youtube.moderator');
  assert.equal(core.roleKey('follower', 'tiktok'), 'role.follower');
  assert.equal(core.roleKey('everyone', 'youtube'), 'role.everyone', 'a role YouTube does not rename');
  assert.equal(core.roleKey('follower', undefined), 'role.follower');
  const line = core.reduce(core.initialState(), { v: 1, seq: 1, type: 'hello', data: { chat_log: [{ name: 'a', text: 'x', role: 'subscriber', source: 'youtube' }, { name: 'b', text: 'y', source: 'weird' }] } }).messages;
  assert.deepEqual(line.map((l) => l.source), ['youtube', 'tiktok']);
});

test('every part of the account address belongs to a tab, and the profile is the default', () => {
  assert.deepEqual(core.ACCOUNT_TABS, ['profile', 'token']);
  for (const part of core.ACCOUNT_SECTIONS) assert.ok(core.ACCOUNT_TABS.includes(core.accountTab('#account/' + part)), part);
  assert.equal(core.accountTab('#account/youtube'), 'profile', 'the YouTube parts moved to the Sources page');
  assert.equal(core.accountTab('#account/token'), 'token');
  for (const other of ['#account', '#account/nope', '', undefined, null]) assert.equal(core.accountTab(other), 'profile', String(other));
});

test('the avatar picture is cut from the middle as a square', () => {
  assert.deepEqual(core.squareCrop(400, 300), { x: 50, y: 0, side: 300 });
  assert.deepEqual(core.squareCrop(300, 400), { x: 0, y: 50, side: 300 });
  assert.deepEqual(core.squareCrop(128, 128), { x: 0, y: 0, side: 128 });
  assert.deepEqual(core.squareCrop(101, 100), { x: 0, y: 0, side: 100 });
});

test('the display name is one trimmed line of at most 40 letters', () => {
  assert.equal(core.cleanDisplayName('  Rina  '), 'Rina');
  assert.equal(core.cleanDisplayName('two\nlines\tof it'), 'two lines of it');
  assert.equal(core.cleanDisplayName('x'.repeat(60)), 'x'.repeat(core.MAX_DISPLAY_NAME));
  assert.equal(core.cleanDisplayName('\u{1F47B}'.repeat(50)), '\u{1F47B}'.repeat(core.MAX_DISPLAY_NAME), 'by letters, not by code units');
  assert.equal(core.cleanDisplayName('   '), '');
});

test('the profile comes from the dashboard settings, and only a safe picture is taken', () => {
  const picture = 'data:image/jpeg;base64,/9j/4AAQ+x==';
  assert.deepEqual(core.profileOf({ dashboard: { display_name: 'Rina', avatar: picture } }), { name: 'Rina', avatar: picture });
  assert.deepEqual(core.profileOf({ dashboard: { chat_history: 20, display_name: '', avatar: '' } }), { name: '', avatar: '' });
  for (const avatar of ['https://example.com/a.png', 'data:image/svg+xml;base64,PHN2Zz4=', 'javascript:alert(1)', 'data:image/png;base64,a"onerror="x', 7, null]) {
    assert.equal(core.profileOf({ dashboard: { avatar } }).avatar, '', String(avatar));
  }
  for (const settings of [null, undefined, {}, { dashboard: null }, { dashboard: { display_name: 5 } }]) {
    assert.deepEqual(core.profileOf(settings), { name: '', avatar: '' });
  }
});

test('the two chat overlays have their own addresses and their own looks', () => {
  assert.equal(core.overlayUrl('http:', '127.0.0.1:8787', '', 'chat-panel'), 'http://localhost:8787/overlay/chat/panel/');
  assert.equal(core.overlayUrl('http:', '127.0.0.1:8787', '', 'chat-float'), 'http://localhost:8787/overlay/chat/float/');
  assert.equal(core.overlayUrl('https:', 'x.example', 'a b', 'chat-float'), 'https://x.example/overlay/chat/float/?token=a%20b');
  assert.equal(core.overlayUrl('http:', 'x', '', 'nope'), 'http://x/overlay/mediashare/', 'an unknown overlay is the mediashare one');
  assert.deepEqual(core.CHAT_PANEL_THEMES, ['obsidian', 'paper', 'brutal', 'terminal', 'candy', 'pixel', 'receipt', 'messages', 'ticker']);
  assert.deepEqual(core.CHAT_FLOAT_THEMES, ['outline', 'comic', 'notes']);
  assert.equal(core.chatPanelTheme({ overlay: { chat_panel: { theme: 'ticker' } } }), 'ticker');
  assert.equal(core.chatFloatTheme({ overlay: { chat_float: { theme: 'notes' } } }), 'notes');
  for (const bad of [null, {}, { overlay: {} }, { overlay: { chat_panel: { theme: 'comic' } } }, { overlay: { chat_panel: { theme: 'neon' } } }]) {
    assert.equal(core.chatPanelTheme(bad), 'obsidian', JSON.stringify(bad));
  }
  for (const bad of [null, {}, { overlay: { chat_float: { theme: 'obsidian' } } }, { overlay: { chat_panel: { theme: 'pixel' } } }]) {
    assert.equal(core.chatFloatTheme(bad), 'outline', JSON.stringify(bad));
  }
  assert.deepEqual(core.OVERLAY_TABS, ['mediashare', 'chat-panel', 'chat-float']);
});

test('what the server says about updates is read, and only this project\'s links are taken', () => {
  const link = 'https://github.com/fhudint/stream-spectre/releases/tag/v0.2.0';
  assert.deepEqual(
    core.updateOf({ current: '0.1.0', enabled: true, checked: true, latest: { version: '0.2.0', url: link } }),
    { current: '0.1.0', enabled: true, checked: true, latest: { version: '0.2.0', url: link } }
  );
  assert.deepEqual(core.updateOf({ current: '0.1.0', enabled: false, checked: false, latest: null }), {
    current: '0.1.0', enabled: false, checked: false, latest: null,
  });
  for (const url of ['https://evil.example/fhudint/stream-spectre/', 'http://github.com/fhudint/stream-spectre/x', 'javascript:alert(1)', 'https://github.com/other/repo/', 5, null]) {
    assert.equal(core.updateOf({ current: '0.1.0', latest: { version: '9', url } }).latest, null, String(url));
  }
  assert.equal(core.updateOf({ current: '0.1.0', latest: { version: 9, url: link } }).latest, null);
  for (const bad of [null, undefined, 'x', [], {}, { current: 1 }]) assert.equal(core.updateOf(bad), null, JSON.stringify(bad));
});

test('chat lines are narrowed by command, source and words, and the rate counts the last minute only', () => {
  const l = (extra) => ({ name: 'Budi', text: 'Hello there', source: 'tiktok', command: false, at_ms: 0, ...extra });
  assert.equal(core.chatMatches(l(), { source: 'all', query: '' }), true);
  assert.equal(core.chatMatches(l(), { source: 'youtube', query: '' }), false);
  assert.equal(core.chatMatches(l({ source: 'youtube' }), { source: 'youtube', query: '' }), true);
  assert.equal(core.chatMatches(l(), { source: 'all', query: 'budi' }), true, 'the name counts');
  assert.equal(core.chatMatches(l(), { source: 'all', query: 'there' }), true);
  assert.equal(core.chatMatches(l(), { source: 'all', query: 'nope' }), false);
  assert.equal(core.chatMatches(l(), { commandsOnly: true }), false);
  assert.equal(core.chatMatches(l({ command: true }), { commandsOnly: true }), true);

  const now = 1_700_000_000_000;
  const messages = [l({ at_ms: now - 61000 }), l({ at_ms: now - 59000 }), l({ at_ms: now - 1 }), l({ at_ms: 0 })];
  assert.equal(core.chatRate(messages, now), 2);
  assert.equal(core.chatRate([], now), 0);
});

test('the presets are told apart, and minutes and seconds convert both ways', () => {
  const base = { mode: 'manual', youtube_min_role: 'viewer', tiktok_min_role: 'follower', cooldown_secs: 60, max_duration_secs: 600, max_queue_len: 20 };
  assert.equal(core.presetOf(base), 'balanced');
  assert.equal(core.presetOf({ ...base, ...core.PRESETS.strict }), 'strict');
  assert.equal(core.presetOf({ ...base, ...core.PRESETS.open }), 'open');
  assert.equal(core.presetOf({ ...base, cooldown_secs: 61 }), '', 'a mix is none');
  assert.equal(core.presetOf(null), '');
  // Every key a preset sets is a rule of the page, and every rule of the page is in a card.
  const keys = new Set(core.SETTINGS_FIELDS.map((f) => f.key));
  for (const preset of Object.values(core.PRESETS)) for (const key of Object.keys(preset)) assert.ok(keys.has(key), key);
  for (const field of core.SETTINGS_FIELDS) assert.ok(core.RULE_GROUPS.includes(field.group), field.key);

  assert.equal(core.secondsToMinutes(600), '10');
  assert.equal(core.secondsToMinutes(90), '1.5');
  assert.equal(core.secondsToMinutes(0), '0');
  assert.equal(core.secondsToMinutes(undefined), '');
  assert.equal(core.minutesToSeconds('10'), '600');
  assert.equal(core.minutesToSeconds(' 1.5 '), '90');
  assert.equal(core.minutesToSeconds('ten'), 'ten', 'what is not a number goes on to be refused');
  assert.equal(core.minutesToSeconds('-2'), '-2');
  assert.equal(core.minutesToSeconds(''), '');
});
