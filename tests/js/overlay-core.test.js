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
const hello = (seq, nowPlaying, volume = 50) =>
  envelope(seq, 'hello', { volume, queue: { pending: [], approved: [], now_playing: nowPlaying } });

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
