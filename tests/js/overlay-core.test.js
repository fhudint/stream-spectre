'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const core = require('../../static/overlay/overlay-core.js');

const ID_A = 'aaaaaaaaaaa';
const ID_B = 'bbbbbbbbbbb';

function item(id, videoId = ID_A, extra = {}) {
  return {
    id,
    video_id: videoId,
    title: `Title ${id}`,
    requested_by: { name: `Viewer ${id}`, role: 'follower' },
    ...extra,
  };
}

function setup({ connected = true, texts = {} } = {}) {
  const calls = [];
  const sent = [];
  let open = connected;
  let position = null;
  const player = {
    position: () => position,
    play: (videoId, volume) => calls.push(['play', videoId, volume]),
    stop: () => calls.push(['stop']),
    setVolume: (volume) => calls.push(['setVolume', volume]),
    pause: () => calls.push(['pause']),
    resume: () => calls.push(['resume']),
  };
  const ui = {
    show: (info) => calls.push(['show', info]),
    hide: () => calls.push(['hide']),
  };
  const send = (message) => {
    if (!open) return false;
    sent.push(message);
    return true;
  };
  const controller = core.createController({ player, ui, send, text: (key) => texts[key] });
  return {
    controller,
    calls,
    sent,
    setConnected: (value) => {
      open = value;
    },
    setPosition: (value) => {
      position = value;
    },
    names: () => calls.map((c) => c[0]),
  };
}

const envelope = (seq, type, data) => ({ v: 1, seq, ts: 0, type, data });
const hello = (seq, nowPlaying, volume = 50, paused = false) =>
  envelope(seq, 'hello', { volume, playback_paused: paused, queue: { pending: [], approved: [], now_playing: nowPlaying } });

// --- helpers ---

test('format fills placeholders in one pass', () => {
  assert.equal(core.format('Hi {name}!', { name: 'Budi' }), 'Hi Budi!');
  assert.equal(core.format('{a} {b}', { a: '{b}', b: 'X' }), '{b} X');
  assert.equal(core.format('a {x} b', {}), 'a {x} b');
  assert.equal(core.format('open { brace', {}), 'open { brace');
  assert.equal(core.format('close } only', {}), 'close } only');
  assert.equal(core.format('{{name}}', { name: 'v' }), '{v}');
  assert.equal(core.format('', { a: 'b' }), '');
  assert.equal(core.format('{}', {}), '{}');
  assert.equal(core.format('{toString}', {}), '{toString}');
  assert.equal(core.format('{name}', { name: '<img src=x onerror=alert(1)>' }), '<img src=x onerror=alert(1)>');
});

test('isVideoId accepts exactly eleven url-safe characters', () => {
  for (const ok of ['dQw4w9WgXcQ', 'a_b-C1d2E3f', '___________']) assert.ok(core.isVideoId(ok), ok);
  for (const bad of ['', 'short', 'dQw4w9WgXcQQ', 'dQw4w9WgXc!', 'dQw4w9WgXc\n', null, undefined, 12345678901, {}, ['dQw4w9WgXcQ']]) {
    assert.ok(!core.isVideoId(bad), String(bad));
  }
});

test('sanitizeCode keeps a short safe token', () => {
  assert.equal(core.sanitizeCode('150'), '150');
  assert.equal(core.sanitizeCode(101), '101');
  // Letters and digits only, at most 16 of them.
  assert.equal(core.sanitizeCode('<script>alert(1)</script>'), 'scriptalert1scri');
  assert.equal(core.sanitizeCode(''), 'unknown');
  assert.equal(core.sanitizeCode('!!!'), 'unknown');
  assert.equal(core.sanitizeCode(undefined), 'undefined');
});

// --- hello ---

test('hello with a video playing starts it with the caption and volume', () => {
  const t = setup({ texts: { 'overlay.requested_by': 'Diminta oleh {name}' } });
  t.controller.handle(hello(7, item(3), 40));
  assert.deepEqual(t.calls, [
    ['play', ID_A, 40],
    ['show', { requestedBy: 'Diminta oleh Viewer 3', title: 'Title 3' }],
  ]);
  assert.equal(t.controller.state().lastSeq, 7);
  assert.equal(t.controller.state().current.id, 3);
});

test('hello with nothing playing clears the screen and sets the volume', () => {
  const t = setup();
  t.controller.handle(hello(1, null, 25));
  assert.deepEqual(t.names(), ['stop', 'hide', 'setVolume']);
  assert.deepEqual(t.calls[2], ['setVolume', 25]);
});

test('a repeated hello for the same video does not restart it', () => {
  const t = setup();
  t.controller.handle(hello(1, item(3)));
  t.calls.length = 0;
  t.controller.handle(hello(9, item(3), 30));
  assert.deepEqual(t.calls, [['setVolume', 30]]);
  assert.equal(t.controller.state().lastSeq, 9);
});

test('a hello for a different video switches to it', () => {
  const t = setup();
  t.controller.handle(hello(1, item(3)));
  t.calls.length = 0;
  t.controller.handle(hello(2, item(4, ID_B)));
  assert.deepEqual(t.names(), ['play', 'show']);
  assert.deepEqual(t.calls[0], ['play', ID_B, 50]);
});

test('the default caption is used when no translation is available', () => {
  const t = setup();
  t.controller.handle(hello(1, item(3)));
  assert.equal(t.calls[1][1].requestedBy, 'Requested by Viewer 3');
});

// --- events ---

test('play, skip, stop and volume events drive the player', () => {
  const t = setup();
  t.controller.handle(hello(0, null));
  t.calls.length = 0;

  t.controller.handle(envelope(1, 'play', { item: item(5), volume: 60 }));
  assert.deepEqual(t.calls[0], ['play', ID_A, 60]);
  t.controller.handle(envelope(2, 'volume', { volume: 20 }));
  assert.deepEqual(t.calls.at(-1), ['setVolume', 20]);
  t.controller.handle(envelope(3, 'skip', { item_id: 5, reason: 'moderator' }));
  assert.deepEqual(t.calls.slice(-2).map((c) => c[0]), ['stop', 'hide']);
  t.controller.handle(envelope(4, 'play', { item: item(6, ID_B), volume: 20 }));
  t.controller.handle(envelope(5, 'stop', {}));
  assert.deepEqual(t.calls.slice(-2).map((c) => c[0]), ['stop', 'hide']);
  assert.equal(t.controller.state().current, null);
});

test('a skip for another video is ignored', () => {
  const t = setup();
  t.controller.handle(hello(0, item(5)));
  t.calls.length = 0;
  t.controller.handle(envelope(1, 'skip', { item_id: 99, reason: 'moderator' }));
  assert.deepEqual(t.calls, []);
  assert.equal(t.controller.state().current.id, 5);
});

test('events already covered by the snapshot, or repeated, are ignored', () => {
  const t = setup();
  t.controller.handle(hello(10, item(5)));
  t.calls.length = 0;
  t.controller.handle(envelope(10, 'stop', {}));
  t.controller.handle(envelope(9, 'play', { item: item(6, ID_B), volume: 10 }));
  assert.deepEqual(t.calls, []);
  t.controller.handle(envelope(11, 'stop', {}));
  assert.deepEqual(t.names(), ['stop', 'hide']);
  t.calls.length = 0;
  t.controller.handle(envelope(11, 'stop', {}));
  assert.deepEqual(t.calls, []);
});

test('volume is clamped and junk volumes are ignored', () => {
  const t = setup();
  t.controller.handle(hello(0, null, 50));
  t.controller.handle(envelope(1, 'volume', { volume: 250 }));
  assert.equal(t.controller.state().volume, 100);
  t.controller.handle(envelope(2, 'volume', { volume: -5 }));
  assert.equal(t.controller.state().volume, 0);
  t.controller.handle(envelope(3, 'volume', { volume: 'loud' }));
  assert.equal(t.controller.state().volume, 0);
  t.controller.handle(envelope(4, 'volume', { volume: NaN }));
  assert.equal(t.controller.state().volume, 0);
  t.controller.handle(envelope(5, 'volume', { volume: 33.6 }));
  assert.equal(t.controller.state().volume, 34);
});

test('events the overlay does not use, and other protocol versions, are ignored', () => {
  const t = setup();
  t.controller.handle(hello(0, null));
  t.calls.length = 0;
  t.controller.handle(envelope(1, 'queue_updated', { pending: [] }));
  t.controller.handle(envelope(2, 'notice', { text: 'x' }));
  t.controller.handle(envelope(3, 'something_new', {}));
  t.controller.handle({ v: 2, seq: 4, ts: 0, type: 'stop', data: {} });
  assert.deepEqual(t.calls, []);
});

test('garbage input never throws', () => {
  const t = setup();
  const junk = [null, undefined, 0, 1, 'text', [], {}, { v: 1 }, { v: 1, type: 'play' }, { v: 1, seq: 'x', type: 'stop' },
    { v: 1, seq: 1, type: 'play', data: null }, { v: 1, seq: 2, type: 'play', data: { item: null } },
    { v: 1, seq: 3, type: 'play', data: { item: 'str' } }, { v: 1, seq: 4, type: 'play', data: { item: { id: 'x' } } },
    { v: 1, type: 'hello' }, { v: 1, type: 'hello', data: 5 }, { v: 1, type: 'hello', data: { queue: 5 } }];
  for (const message of junk) assert.doesNotThrow(() => t.controller.handle(message), JSON.stringify(message));
});

test('the caption and title are handed over as plain strings', () => {
  const t = setup();
  const evil = '<img src=x onerror=alert(1)>';
  t.controller.handle(hello(0, item(5, ID_A, { title: evil, requested_by: { name: evil, role: 'follower' } })));
  const shown = t.calls.find((c) => c[0] === 'show')[1];
  assert.equal(shown.title, evil);
  assert.equal(shown.requestedBy, `Requested by ${evil}`);
  assert.equal(typeof shown.title, 'string');
});

test('missing or odd item fields fall back to empty text', () => {
  const t = setup();
  t.controller.handle(envelope(1, 'play', { item: { id: 1, video_id: ID_A, title: 5, requested_by: null }, volume: 50 }));
  assert.deepEqual(t.calls.find((c) => c[0] === 'show')[1], { requestedBy: 'Requested by ', title: '' });
});

test('an item with an invalid video id is reported, not played', () => {
  const t = setup();
  t.controller.handle(envelope(1, 'play', { item: item(8, 'not-valid'), volume: 50 }));
  assert.ok(!t.names().includes('play'));
  assert.deepEqual(t.sent, [{ type: 'player_error', item_id: 8, code: 'invalid' }]);
  // A second report for it is not sent.
  t.controller.onPlayerError('2');
  assert.equal(t.sent.length, 1);
});

// --- reports to the server ---

test('the end of a video is reported once', () => {
  const t = setup();
  t.controller.handle(hello(0, item(5)));
  t.controller.onPlayerEnded();
  t.controller.onPlayerEnded();
  assert.deepEqual(t.sent, [{ type: 'ended', item_id: 5 }]);
});

test('a player error is reported once with a clean code', () => {
  const t = setup();
  t.controller.handle(hello(0, item(5)));
  t.controller.onPlayerError('<b>150</b>');
  t.controller.onPlayerEnded();
  assert.deepEqual(t.sent, [{ type: 'player_error', item_id: 5, code: 'b150b' }]);
});

test('reports with nothing playing are dropped', () => {
  const t = setup();
  t.controller.onPlayerEnded();
  t.controller.onPlayerError('2');
  assert.deepEqual(t.sent, []);
});

test('each new video can be reported again', () => {
  const t = setup();
  t.controller.handle(hello(0, item(5)));
  t.controller.onPlayerEnded();
  t.controller.handle(envelope(1, 'play', { item: item(6, ID_B), volume: 50 }));
  t.controller.onPlayerEnded();
  assert.deepEqual(t.sent, [{ type: 'ended', item_id: 5 }, { type: 'ended', item_id: 6 }]);
});

test('a report made while disconnected is sent when the socket reopens', () => {
  const t = setup();
  t.controller.handle(hello(0, item(5)));
  t.setConnected(false);
  t.controller.onPlayerEnded();
  assert.deepEqual(t.sent, []);
  assert.deepEqual(t.controller.state().unsent, { type: 'ended', item_id: 5 });

  t.setConnected(true);
  t.controller.onSocketOpen();
  assert.deepEqual(t.sent, [{ type: 'player_ready' }, { type: 'ended', item_id: 5 }]);
  assert.equal(t.controller.state().unsent, null);
});

test('after a reconnect the same finished video is not restarted', () => {
  const t = setup();
  t.controller.handle(hello(0, item(5)));
  t.setConnected(false);
  t.controller.onPlayerEnded();
  t.setConnected(true);
  t.controller.onSocketOpen();
  t.calls.length = 0;
  t.controller.handle(hello(4, item(5)));
  assert.ok(!t.names().includes('play'));
});

test('a stale unsent report is dropped when the server moved on', () => {
  const t = setup();
  t.controller.handle(hello(0, item(5)));
  t.setConnected(false);
  t.controller.onPlayerEnded();
  t.setConnected(true);
  t.controller.handle(hello(9, item(6, ID_B)));
  assert.equal(t.controller.state().unsent, null);
  t.controller.onSocketOpen();
  assert.deepEqual(t.sent, [{ type: 'player_ready' }]);
});

test('player_ready is announced when the socket opens', () => {
  const t = setup();
  t.controller.onSocketOpen();
  assert.deepEqual(t.sent, [{ type: 'player_ready' }]);
});

// --- playback progress ---

test('tick reports the whole second the video has reached', () => {
  const t = setup();
  t.controller.handle(hello(0, item(5)));
  t.setPosition(12.9);
  t.controller.tick();
  assert.deepEqual(t.sent.at(-1), { type: 'progress', item_id: 5, position_secs: 12 });
  t.setPosition(0);
  t.controller.tick();
  assert.deepEqual(t.sent.at(-1), { type: 'progress', item_id: 5, position_secs: 0 });
});

test('tick says nothing without a video, once it ended or failed, or without a usable position', () => {
  const t = setup();
  t.setPosition(5);
  t.controller.tick();
  assert.deepEqual(t.sent, [], 'nothing is playing');

  t.controller.handle(hello(0, item(5)));
  for (const bad of [null, undefined, NaN, Infinity, -1, '5']) {
    t.setPosition(bad);
    t.controller.tick();
  }
  assert.deepEqual(t.sent, [], 'no usable position');

  t.setPosition(30);
  t.controller.onPlayerEnded();
  t.controller.tick();
  assert.deepEqual(t.sent, [{ type: 'ended', item_id: 5 }], 'already reported as ended');
});

test('tick follows the current video and stops when the screen is cleared', () => {
  const t = setup();
  t.controller.handle(hello(0, item(5)));
  t.setPosition(3);
  t.controller.handle(envelope(1, 'play', { item: item(6, ID_B), volume: 50 }));
  t.controller.tick();
  assert.deepEqual(t.sent.at(-1), { type: 'progress', item_id: 6, position_secs: 3 });
  t.controller.handle(envelope(2, 'stop', {}));
  const count = t.sent.length;
  t.controller.tick();
  assert.equal(t.sent.length, count);
});

test('tick is harmless when the player cannot tell its position', () => {
  const calls = [];
  const controller = core.createController({
    player: { play() {}, stop() {}, setVolume() {} },
    ui: { show() {}, hide() {} },
    send: (m) => calls.push(m),
  });
  controller.handle(hello(0, item(5)));
  controller.tick();
  assert.deepEqual(calls, []);
});

// --- pausing ---

const paused = (seq, itemId, flag) => envelope(seq, 'playback_paused', { item_id: itemId, paused: flag });

test('the video on screen is paused and resumed when the server says so', () => {
  const t = setup();
  t.controller.handle(hello(0, item(5)));
  t.calls.length = 0;
  t.controller.handle(paused(1, 5, true));
  assert.deepEqual(t.calls, [['pause']]);
  t.controller.handle(paused(2, 5, false));
  assert.deepEqual(t.calls, [['pause'], ['resume']]);
});

test('saying the same thing twice does not touch the player again', () => {
  const t = setup();
  t.controller.handle(hello(0, item(5)));
  t.calls.length = 0;
  t.controller.handle(paused(1, 5, true));
  t.controller.handle(paused(2, 5, true));
  t.controller.handle(paused(3, 5, false));
  t.controller.handle(paused(4, 5, false));
  assert.deepEqual(t.names(), ['pause', 'resume']);
});

test('a pause for a video that is not the one on screen is ignored', () => {
  const t = setup();
  t.controller.handle(hello(0, item(5)));
  t.calls.length = 0;
  t.controller.handle(paused(1, 99, true));
  assert.deepEqual(t.calls, []);
  const idle = setup();
  idle.controller.handle(paused(1, 5, true));
  assert.deepEqual(idle.calls, []);
});

test('a video that starts after a pause plays, and can be paused again', () => {
  const t = setup();
  t.controller.handle(hello(0, item(5)));
  t.controller.handle(paused(1, 5, true));
  t.calls.length = 0;
  t.controller.handle(envelope(2, 'play', { item: item(6, ID_B), volume: 50 }));
  assert.ok(!t.names().includes('pause'));
  t.controller.handle(paused(3, 6, true));
  assert.equal(t.names().at(-1), 'pause');

  // Clearing the screen forgets the pause too.
  t.controller.handle(envelope(4, 'stop', {}));
  t.controller.handle(envelope(5, 'play', { item: item(7), volume: 50 }));
  t.calls.length = 0;
  t.controller.handle(paused(6, 7, true));
  assert.deepEqual(t.names(), ['pause'], 'it is not already paused');
});

test('a page that opens in the middle of a pause starts the video and pauses it', () => {
  const t = setup();
  t.controller.handle(hello(7, item(3), 40, true));
  assert.deepEqual(t.names().filter((n) => ['play', 'pause'].includes(n)), ['play', 'pause']);
});

test('after a reconnect a pause is kept, and a resume that was missed is applied', () => {
  const t = setup();
  t.controller.handle(hello(0, item(5)));
  t.controller.handle(paused(1, 5, true));
  t.calls.length = 0;
  t.controller.handle(hello(9, item(5), 50, true));
  assert.ok(!t.names().includes('pause') && !t.names().includes('play'), 'already paused: left alone');
  t.controller.handle(hello(12, item(5), 50, false));
  assert.deepEqual(t.names().filter((n) => n === 'resume' || n === 'play'), ['resume']);
});

test('a hello without a video clears a pause', () => {
  const t = setup();
  t.controller.handle(hello(0, item(5)));
  t.controller.handle(paused(1, 5, true));
  t.controller.handle(hello(5, null));
  t.controller.handle(hello(6, item(8, ID_B)));
  t.calls.length = 0;
  t.controller.handle(paused(7, 8, true));
  assert.deepEqual(t.names(), ['pause']);
});

test('a player that cannot pause is not a problem', () => {
  const calls = [];
  const controller = core.createController({
    player: { play() {}, stop() {}, setVolume() {} },
    ui: { show() {}, hide() {} },
    send: (m) => calls.push(m),
  });
  controller.handle(hello(0, item(5)));
  assert.doesNotThrow(() => controller.handle(paused(1, 5, true)));
  assert.doesNotThrow(() => controller.handle(paused(2, 5, false)));
});

test('isVideoRef checks the ID against the source it is for', () => {
  for (const source of [undefined, 'youtube']) {
    assert.ok(core.isVideoRef(source, 'dQw4w9WgXcQ'));
    assert.ok(!core.isVideoRef(source, '7700124541145599252'));
  }
  assert.ok(core.isVideoRef('tiktok', '7700124541145599252'));
  assert.ok(core.isVideoRef('tiktok', '1'.repeat(15)));
  assert.ok(core.isVideoRef('tiktok', '9'.repeat(20)));
  for (const bad of ['', '1'.repeat(14), '1'.repeat(21), 'dQw4w9WgXcQ', '77001245411455992a2', '７７００１２４５４１１４５５９９２５２', '7700124541145599252\n', null, undefined, 7700124541145599252, {}]) {
    assert.ok(!core.isVideoRef('tiktok', bad), String(bad));
  }
  // A source the overlay does not know is not played, whatever the ID.
  for (const source of ['vimeo', 'TikTok', '', null, 1, {}]) assert.ok(!core.isVideoRef(source, 'dQw4w9WgXcQ'), String(source));
});

test('the player is told which source the video is from', () => {
  const calls = [];
  const controller = core.createController({
    player: { play: (...args) => calls.push(args), stop() {}, setVolume() {}, pause() {}, resume() {}, position: () => null },
    ui: { show() {}, hide() {} },
    send: () => true,
  });
  const base = { requested_by: { name: 'A', role: 'follower' }, title: 't' };
  controller.handle(envelope(1, 'play', { volume: 30, item: { id: 1, video_id: 'dQw4w9WgXcQ', ...base } }));
  controller.handle(envelope(2, 'play', { volume: 30, item: { id: 2, source: 'youtube', video_id: 'dQw4w9WgXcQ', ...base } }));
  controller.handle(envelope(3, 'play', { volume: 30, item: { id: 3, source: 'tiktok', video_id: '7700124541145599252', ...base } }));
  assert.deepEqual(calls, [
    ['dQw4w9WgXcQ', 30, 'youtube'],
    ['dQw4w9WgXcQ', 30, 'youtube'],
    ['7700124541145599252', 30, 'tiktok'],
  ]);
});

test('an ID that does not fit its source is reported and never played', () => {
  const { controller, calls, sent } = setup();
  const base = { requested_by: { name: 'A', role: 'follower' }, title: 't' };
  const bad = [
    { id: 1, source: 'tiktok', video_id: 'dQw4w9WgXcQ' },
    { id: 2, source: 'youtube', video_id: '7700124541145599252' },
    { id: 3, source: 'vimeo', video_id: 'dQw4w9WgXcQ' },
    { id: 4, source: 'tiktok', video_id: '77001245411455992<script>' },
  ];
  bad.forEach((entry, index) => controller.handle(envelope(index + 1, 'play', { volume: 50, item: { ...entry, ...base } })));
  assert.ok(!calls.some((c) => c[0] === 'play'));
  assert.deepEqual(sent.map((m) => [m.type, m.item_id, m.code]), [
    ['player_error', 1, 'invalid'],
    ['player_error', 2, 'invalid'],
    ['player_error', 3, 'invalid'],
    ['player_error', 4, 'invalid'],
  ]);
});

// --- sound of TikTok's player ---

function tiktokSound(volume = 50) {
  const posts = [];
  let now = 1000;
  let level = volume;
  const sound = core.createTiktokSound({ post: (c) => posts.push(c), volume: () => level, now: () => now });
  const say = (type, value) => sound.handle({ 'x-tiktok-player': true, type, value });
  return { sound, posts, say, tick: (ms) => { now += ms; }, setLevel: (v) => { level = v; } };
}
const ASK = (value) => [{ type: 'setVolume', value }, { type: 'unMute' }];

test('the player is asked for sound with the volume as soon as it speaks', () => {
  const { say, posts } = tiktokSound(35);
  assert.deepEqual(posts, [], 'nothing before the player says something');
  say('onCurrentTime', { currentTime: 0.1, duration: 23 });
  assert.deepEqual(posts, ASK(35));
});

test('it keeps asking while the player stays muted, at most six times', () => {
  const { say, posts, sound } = tiktokSound();
  for (let i = 0; i < 20; i += 1) say(i % 2 ? 'onMute' : 'onCurrentTime', i % 2 ? true : { currentTime: i });
  assert.equal(posts.length, 6 * 2);
  assert.equal(sound.state().asks, 6);
});

test('once the player says it is unmuted, nothing more is asked', () => {
  const { say, posts } = tiktokSound();
  say('onCurrentTime', {});
  say('onMute', false);
  const asked = posts.length;
  for (let i = 0; i < 5; i += 1) say('onCurrentTime', { currentTime: i });
  say('onVolumeChange', 50);
  assert.equal(posts.length, asked);
});

test('a pause right after asking is the browser refusing sound: mute and play again', () => {
  const { say, posts, sound, tick } = tiktokSound();
  say('onCurrentTime', {});
  tick(120);
  say('onMute', false);
  say('onStateChange', 2);
  assert.deepEqual(posts.slice(2), [{ type: 'mute' }, { type: 'play' }]);
  assert.equal(sound.state().silent, true);
  // From then on it stays muted and quiet: no more asking, whatever it reports.
  const count = posts.length;
  say('onMute', true);
  say('onCurrentTime', {});
  say('onStateChange', 2);
  sound.setVolume();
  assert.equal(posts.length, count);
});

test('a pause long after asking is not a refusal', () => {
  const { say, posts, tick } = tiktokSound();
  say('onCurrentTime', {});
  say('onMute', false);
  tick(10000); // the video ran, perhaps to its end
  say('onStateChange', 2);
  assert.equal(posts.length, 2, 'only the first ask');
});

test('a pause before sound was ever asked for is left alone', () => {
  const { say, posts } = tiktokSound();
  say('onStateChange', 2);
  // It is the player speaking, so sound is asked for, but nothing is muted.
  assert.deepEqual(posts, ASK(50));
});

test('what is not the player speaking is ignored', () => {
  const { sound, posts } = tiktokSound();
  for (const data of [null, undefined, 'onMute', 5, [], {}, { type: 'onMute' }, { 'x-tiktok-player': false, type: 'onMute' }, { 'x-tiktok-player': 'true', type: 'onMute' }, { 'x-tiktok-player': true }, { 'x-tiktok-player': true, type: 5 }]) {
    sound.handle(data);
  }
  assert.deepEqual(posts, []);
  assert.equal(sound.state().asks, 0);
});

test('a volume change reaches the player only while it is unmuted and not refused', () => {
  const { say, posts, sound, setLevel, tick } = tiktokSound(50);
  sound.setVolume();
  assert.deepEqual(posts, [], 'not known to be unmuted yet');
  say('onCurrentTime', {});
  say('onMute', false);
  posts.length = 0;
  setLevel(20);
  sound.setVolume();
  assert.deepEqual(posts, [{ type: 'setVolume', value: 20 }]);

  tick(50);
  say('onStateChange', 2); // refused
  posts.length = 0;
  sound.setVolume();
  assert.deepEqual(posts, [], 'it is muted on purpose now');
});

test('each video starts over', () => {
  const { say, posts, sound, tick } = tiktokSound();
  say('onCurrentTime', {});
  tick(10);
  say('onStateChange', 2);
  assert.equal(sound.state().silent, true);
  sound.begin();
  assert.deepEqual(sound.state(), { muted: null, asks: 0, silent: false });
  posts.length = 0;
  say('onCurrentTime', {});
  assert.deepEqual(posts, ASK(50));
});

test('TikTok\'s player says a video is over with state 0, and fails with onPlayerError', () => {
  const say = (type, value, flag = true) => core.tiktokOutcome({ 'x-tiktok-player': flag, type, value });
  assert.deepEqual(say('onStateChange', 0), { ended: true });
  for (const state of [1, 2, 3, -1, '0', null, undefined, false, [0]]) assert.equal(say('onStateChange', state), null, String(state));

  assert.deepEqual(say('onPlayerError', { errorCode: 1001, errorType: 'INVALID_VIDEO' }), { ended: false, code: '1001' });
  assert.deepEqual(say('onPlayerError', { errorType: 'INVALID_VIDEO' }), { ended: false, code: 'INVALID_VIDEO' });
  for (const value of [undefined, null, 'oops', 5, {}, { errorCode: '1001' }, { errorCode: 1.5 }, { errorType: 7 }]) {
    assert.deepEqual(say('onPlayerError', value), { ended: false, code: 'tiktok' }, JSON.stringify(value));
  }
  // The code goes through sanitizeCode on its way to the server.
  assert.equal(core.sanitizeCode(say('onPlayerError', { errorType: '<b>x</b> a very long error type' }).code), 'bxbaverylongerro');

  // Everything else is not about how the video went.
  for (const type of ['onError', 'onPlayerReady', 'onCurrentTime', 'onMute', 'onVolumeChange', 'onStateChange']) {
    assert.equal(say(type, { errorCode: 1001 }), null, type);
  }
  for (const data of [null, undefined, 'onStateChange', 0, [], {}, { type: 'onStateChange', value: 0 }]) assert.equal(core.tiktokOutcome(data), null, JSON.stringify(data));
  assert.equal(say('onStateChange', 0, false), null);
  assert.equal(say('onStateChange', 0, 'true'), null);
});

test('how much of a video is done is a percentage that is 0 when it cannot be told', () => {
  assert.equal(core.progressPercent(50, 200), 25);
  assert.equal(core.progressPercent(200, 200), 100);
  assert.equal(core.progressPercent(250, 200), 100, 'never past the end');
  for (const [position, duration] of [[0, 200], [-5, 200], [50, 0], [50, -1], [NaN, 200], [50, Infinity], ['5', 20], [5, '20'], [undefined, 20], [null, null]]) {
    assert.equal(core.progressPercent(position, duration), 0, `${position} of ${duration}`);
  }
});

test('a TikTok video has started once it plays or its clock ticks, and not before', () => {
  const say = (type, value, flag = true) => core.tiktokStarted({ 'x-tiktok-player': flag, type, value });
  assert.equal(say('onStateChange', 1), true);
  assert.equal(say('onCurrentTime', { currentTime: 0.2, duration: 23 }), true);
  // Ready, paused, buffering, muted or ended is not yet playing.
  for (const [type, value] of [['onPlayerReady', 1], ['onStateChange', 2], ['onStateChange', 3], ['onStateChange', 0], ['onMute', true], ['onPlayerError', {}]]) {
    assert.equal(say(type, value), false, type + value);
  }
  for (const data of [null, undefined, 'onCurrentTime', 1, [], {}, { type: 'onCurrentTime' }]) assert.equal(core.tiktokStarted(data), false, JSON.stringify(data));
  assert.equal(say('onCurrentTime', {}, false), false);
  assert.equal(core.TIKTOK_START_MS, 15000);
});

test('the pause at the very end of a short video is not the browser refusing sound', () => {
  const { say, posts, tick, sound } = tiktokSound();
  say('onCurrentTime', { currentTime: 0.1, duration: 2 });
  tick(1500);
  say('onCurrentTime', { currentTime: 1.8, duration: 2 });
  say('onStateChange', 2); // the player pauses just before it says 0
  assert.equal(sound.state().silent, false);
  assert.ok(!posts.some((c) => c.type === 'mute' || c.type === 'play'));
});

test('a pause in the middle of a video right after asking is still a refusal', () => {
  const { say, posts, tick } = tiktokSound();
  say('onCurrentTime', { currentTime: 0.4, duration: 23.3 });
  tick(300);
  say('onStateChange', 2);
  assert.deepEqual(posts.slice(2).map((c) => c.type), ['mute', 'play']);
});

test('a pause is a refusal too when the length is not known', () => {
  const { say, posts, tick } = tiktokSound();
  say('onCurrentTime', {});
  tick(300);
  say('onStateChange', 2);
  assert.deepEqual(posts.slice(2).map((c) => c.type), ['mute', 'play']);
  const odd = tiktokSound();
  odd.say('onCurrentTime', { currentTime: 'x', duration: null });
  odd.tick(300);
  odd.say('onStateChange', 2);
  assert.deepEqual(odd.posts.slice(2).map((c) => c.type), ['mute', 'play']);
});

test('begin forgets how far the last video had got', () => {
  const { say, posts, tick, sound } = tiktokSound();
  say('onCurrentTime', { currentTime: 1.9, duration: 2 });
  sound.begin();
  posts.length = 0;
  say('onCurrentTime', {});
  tick(300);
  say('onStateChange', 2);
  assert.deepEqual(posts.slice(2).map((c) => c.type), ['mute', 'play']);
});

