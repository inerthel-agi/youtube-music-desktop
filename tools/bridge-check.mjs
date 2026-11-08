// Run from the repository root: node --test tools/bridge-check.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import { runInNewContext } from 'node:vm';

const source = readFileSync(new URL('../assets/bridge.js', import.meta.url), 'utf8');
const RECOVERY_KEY = 'ytm-desktop-recovery';

function eventTarget(afterListener = () => {}) {
  const listeners = new Map();
  return {
    addEventListener(type, fn) {
      if (!listeners.has(type)) listeners.set(type, new Set());
      listeners.get(type).add(fn);
    },
    removeEventListener(type, fn) { listeners.get(type)?.delete(fn); },
    fire(type, event = {}) {
      for (const fn of [...listeners.get(type) || []]) { fn({ type, ...event }); afterListener(); }
    },
  };
}

// Exercise the injected script with a paused track and a deterministic clock.
function page({ loaded = true, hostname = 'music.youtube.com', storage = new Map(), resume = null } = {}) {
  let now = 0;
  const calls = [], microtasks = [], intervals = [], navigations = [], seeks = [];
  const counts = { 'nav-bar': 286, 'player-bar': 288 };
  const visible = { 'nav-bar': true, 'player-bar': true };
  const video = { ...eventTarget(() => flush()), paused: true, ended: false, readyState: loaded ? 1 : 0,
    currentTime: 0, duration: 180, volume: 1, starts: 0,
    pause() { if (!this.paused) { this.paused = true; this.fire('pause'); } },
    play() {
      if (this.paused) {
        this.paused = false;
        this.fire('play');
        if (!this.paused) { this.starts++; this.fire('playing'); }
      }
      return Promise.resolve();
    },
  };
  const mediaSession = { metadata: loaded ? { title: 'Private title', artist: 'Private artist' } : null };
  const element = { getAttribute: () => '180', textContent: '0:00 / 3:00' };
  const player = { id: loaded ? 'private-id' : '', position: 0, ad: false,
    classList: { contains: () => player.ad }, getCurrentTime: () => player.position,
    getVideoData: () => ({ video_id: player.id }),
    seekTo(t) { seeks.push(t); player.position = t; microtasks.push(() => video.fire('seeked')); },
  };
  const document = {
    ...eventTarget(),
    hidden: false,
    getElementById: () => player,
    querySelectorAll: () => [],
    querySelector(selector) {
      if (selector === 'video') return video;
      if (selector === 'ytmusic-nav-bar' || selector === 'ytmusic-player-bar') {
        const part = selector.slice(8);
        return { querySelectorAll: (s) => s === '*' ? Array(counts[part]).fill(element)
          : [{ checkVisibility: () => visible[part] }] };
      }
      return element;
    },
  };
  const window = {
    ...eventTarget(), __ytmResume: resume,
    __TAURI_INTERNALS__: { invoke(command, args) { calls.push({ command, ...args }); return Promise.resolve(); } },
  };
  const sessionStorage = { getItem: (key) => storage.get(key) ?? null,
    setItem(key, value) { storage.set(key, value); }, removeItem(key) { storage.delete(key); } };
  const location = { hostname, origin: 'https://music.youtube.com',
    href: 'https://music.youtube.com/watch?v=private-id&list=queue&index=2',
    replace(url) { navigations.push(url); } };
  window.top = window;
  const flush = () => { while (microtasks.length) microtasks.shift()(); };
  runInNewContext(source, {
    window, document, navigator: { mediaSession }, location, sessionStorage,
    performance: { now: () => now }, URL, innerWidth: 1200, innerHeight: 800,
    MutationObserver: class { observe() {} disconnect() {} },
    queueMicrotask: (fn) => microtasks.push(fn),
    setInterval: (fn) => intervals.push(fn),
  });
  flush();
  return {
    calls, counts, visible, window, document, mediaSession, video, player,
    storage, sessionStorage, navigations, seeks, flush,
    jump(ms) { now += ms; }, // Background WebView timers can be suspended.
    advance(ms) {
      for (let elapsed = 0; elapsed < ms; elapsed += 2000) {
        now += Math.min(2000, ms - elapsed);
        intervals.forEach((fn) => fn());
        flush();
      }
    },
    error(type, data) { window.fire(type, data); flush(); },
    missing: () => JSON.parse(calls.filter((c) => !c.error).at(-1).msg).missing,
  };
}

test('healthy paused bars send no periodic IPC', () => {
  const p = page();
  p.advance(60000);
  assert.equal(p.calls.length, 1);
  assert.deepEqual(p.missing(), []);
});

for (const part of ['nav-bar', 'player-bar']) {
  test(`detects a nearly empty ${part} without playback events`, () => {
    const p = page();
    p.counts[part] = 4; // Layout containers and the app's shadow host can remain.
    p.advance(6000);
    assert.deepEqual(p.missing(), []);
    p.advance(2000);
    assert.deepEqual(p.missing(), [part]);
    p.counts[part] = 100;
    p.advance(2000);
    assert.deepEqual(p.missing(), []);
  });

  test(`detects an invisible ${part} even when its DOM skeleton remains`, () => {
    const p = page();
    p.visible[part] = false;
    p.advance(8000);
    assert.deepEqual(p.missing(), [part]);
  });
}

for (const native of [false, true]) test(`starts a fresh grace period after restoring (${native ? 'native window' : 'page'})`, () => {
  const p = page();
  p.visible['nav-bar'] = false;
  p.advance(4000);
  if (native) p.window.__ytmSetVisible(false);
  else p.document.hidden = true;
  p.advance(60000);
  assert.deepEqual(p.missing(), []);
  if (native) p.window.__ytmSetVisible(true);
  else p.document.hidden = false;
  p.advance(6000);
  assert.deepEqual(p.missing(), []);
  p.advance(2000);
  assert.deepEqual(p.missing(), ['nav-bar']);
  assert.deepEqual(Array.from(p.calls.at(-1).shell), [1200, 800, 286, 288]);
});

test('restoring resets the grace period even if hidden timers never ran', () => {
  const p = page();
  p.visible['player-bar'] = false;
  p.advance(4000);
  p.window.__ytmSetVisible(false);
  p.jump(60000);
  p.window.__ytmSetVisible(true);
  p.advance(6000);
  assert.deepEqual(p.missing(), []);
  p.advance(2000);
  assert.deepEqual(p.missing(), ['player-bar']);
});

test('each bar must stay empty for the full grace period', () => {
  const p = page();
  p.counts['nav-bar'] = 0;
  p.advance(4000);
  p.counts['nav-bar'] = 286;
  p.counts['player-bar'] = 0;
  p.advance(4000);
  assert.deepEqual(p.missing(), []);
  p.counts['nav-bar'] = 0;
  p.advance(4000);
  assert.deepEqual(p.missing(), ['player-bar']);
  p.advance(4000);
  assert.deepEqual(p.missing(), ['nav-bar', 'player-bar']);
});

test('no recovery signal until a track loads, including without Media Session metadata', () => {
  const p = page({ loaded: false });
  p.counts['nav-bar'] = p.counts['player-bar'] = 0;
  p.advance(60000);
  assert.deepEqual(p.missing(), []);
  p.video.readyState = 1;
  p.advance(6000);
  assert.deepEqual(p.missing(), []);
  p.advance(2000);
  assert.deepEqual(p.missing(), ['nav-bar', 'player-bar']);
});

test('uncaught errors send only categories, with a rolling rate limit', () => {
  const p = page();
  p.error('error', { error: new TypeError('https://private.invalid/Private title'), message: 'private' });
  p.error('unhandledrejection', { reason: new Error('Private title') });
  p.error('error', { error: { name: 'https://private.invalid/' }, message: 'private' });
  p.error('error', { error: new TypeError('This document requires TrustedHTML assignment') });
  p.error('error', { error: { get name() { throw new Error('private'); } } });
  for (let i = 0; i < 20; i++) p.error('unhandledrejection', { reason: 'Private title' });
  const diagnostics = p.calls.filter((c) => c.error);
  assert.deepEqual(diagnostics.map((c) => c.error), [
    'error:TypeError', 'unhandledrejection:Error', 'error:Other', 'error:TrustedTypes', 'error:Other',
  ]);
  assert(!JSON.stringify(diagnostics).includes('private'));
  assert(!JSON.stringify(diagnostics).includes('Private'));
  assert.equal(p.calls.filter((c) => !c.error).length, 1);
  p.advance(60000);
  p.error('unhandledrejection', { reason: null });
  assert.equal(p.calls.filter((c) => c.error).length, 6);
});

test('does not instrument other origins', () => {
  const p = page({ hostname: 'accounts.google.com' });
  p.advance(60000);
  p.error('error', { error: new Error('private') });
  assert.equal(p.calls.length, 0);
});

function savedRecovery(playing, extra = {}) {
  return new Map([[RECOVERY_KEY, JSON.stringify({ id: 'private-id', t: 42.75, playing, ...extra })]]);
}

for (const playing of [false, true]) test(`recovery snapshots the current player clock and ${playing ? 'playing' : 'paused'} state`, () => {
  const p = page();
  p.player.position = 42.75;
  p.video.currentTime = 900; // The video clock covers the whole queue.
  p.video.paused = !playing;
  const url = 'https://music.youtube.com/watch?v=private-id&list=queue&index=2';
  p.window.__ytmRecover(url);
  assert.equal(p.video.paused, true); // Navigation must not let the old clock keep running.
  const saved = JSON.parse(p.storage.get(RECOVERY_KEY));
  assert.equal(saved.t, 42.75);
  assert.equal(saved.playing, playing);
  assert.equal(saved.id, 'private-id');
  assert.deepEqual(p.navigations, [url]);
  const next = page({ storage: p.storage, resume: { t: 99 } });
  assert.equal(next.storage.has(RECOVERY_KEY), false);
  assert.equal(next.player.position, 42.75);
  assert.equal(next.video.paused, !playing);
  assert.equal(next.video.starts, playing ? 1 : 0);
  assert.deepEqual(next.seeks, [42.75]);
  next.video.fire('loadedmetadata');
  next.video.fire('timeupdate');
  next.advance(10000);
  assert.deepEqual(next.seeks, [42.75]);
});

test('a recovered paused track blocks repeated autoplay but accepts native Play', () => {
  const p = page({ storage: savedRecovery(false) });
  for (let i = 0; i < 3; i++) { p.video.play(); p.flush(); }
  assert.equal(p.video.paused, true);
  assert.equal(p.video.starts, 0);
  assert.equal(p.player.position, 42.75);
  p.window.__ytmRestorePlayback(true);
  p.video.play();
  p.flush();
  assert.equal(p.video.paused, false);
  assert.equal(p.video.starts, 1);
  p.player.position = 90;
  p.video.fire('seeked');
  p.advance(10000);
  assert.equal(p.player.position, 90);
});

test('paused recovery never reports playing between media event callbacks', () => {
  const p = page({ storage: savedRecovery(false) });
  p.video.paused = false; // Metadata/volume callbacks can run before the queued play event.
  p.video.fire('volumechange');
  p.video.pause();
  p.video.play();
  p.flush();
  assert(p.calls.filter((c) => !c.error).every((c) => JSON.parse(c.msg).playing === false));
});

test('a trusted click on Play releases the paused guard', () => {
  const p = page({ storage: savedRecovery(false) });
  const target = { closest: () => ({}) };
  p.document.fire('pointerdown', { isTrusted: false, target });
  p.video.play();
  assert.equal(p.video.starts, 0);
  p.document.fire('pointerdown', { isTrusted: true, target });
  p.video.play();
  assert.equal(p.video.starts, 1);
});

test('restoration waits for matching content, metadata and the end of an ad', () => {
  const p = page({ loaded: false, storage: savedRecovery(true) });
  p.video.play();
  p.flush();
  assert.equal(p.video.starts, 0);
  p.player.id = 'private-id';
  p.player.ad = true;
  p.video.readyState = 1;
  p.video.play();
  p.flush();
  assert.equal(p.video.paused, false); // A playing track may need its preroll.
  assert.deepEqual(p.seeks, []);
  p.player.ad = false;
  p.video.fire('loadedmetadata');
  p.flush();
  assert.deepEqual(p.seeks, [42.75]);
  assert.equal(p.video.paused, false);
});

test('paused recovery remains paused through an ad and explicit Play keeps the pending position', () => {
  const p = page({ loaded: false, storage: savedRecovery(false) });
  p.player.ad = true;
  p.video.readyState = 1;
  p.video.play();
  p.flush();
  assert.equal(p.video.starts, 0);
  p.window.__ytmRestorePlayback(true);
  p.video.play();
  assert.equal(p.video.paused, false);
  p.player.id = 'private-id';
  p.player.ad = false;
  p.video.fire('loadedmetadata');
  p.flush();
  assert.equal(p.player.position, 42.75);
  assert.equal(p.video.paused, false);
});

test('cannot reload without a saved snapshot', () => {
  for (const playing of [false, true]) {
    const p = page();
    p.video.paused = !playing;
    p.sessionStorage.setItem = () => { throw new Error('storage unavailable'); };
    p.window.__ytmRecover('https://music.youtube.com/watch?v=private-id');
    assert.deepEqual(p.navigations, []);
    assert.equal(p.video.paused, !playing);
  }
});

test('native Pause while loading overrides the saved playing state', () => {
  const p = page({ loaded: false, storage: savedRecovery(true) });
  p.window.__ytmRestorePlayback(false);
  p.player.id = 'private-id';
  p.video.readyState = 1;
  p.video.fire('loadedmetadata');
  p.flush();
  assert.equal(p.video.paused, true);
  assert.equal(p.video.starts, 0);
  assert.equal(p.player.position, 42.75);
});

test('another track leaves the old recovery position and pause guard behind', () => {
  const p = page({ storage: savedRecovery(false) });
  p.player.id = 'next-track';
  p.player.position = 0;
  p.video.play();
  p.flush();
  assert.equal(p.video.paused, false);
  assert.equal(p.player.position, 0);
  assert.deepEqual(p.seeks, [42.75]);
});

test('the startup resume still pauses once when there is no recovery snapshot', () => {
  const p = page({ resume: { t: 21 } });
  p.video.play();
  p.flush();
  assert.equal(p.video.paused, true);
  assert.equal(p.player.position, 21);
  p.video.play();
  p.flush();
  assert.equal(p.video.paused, false);
  assert.deepEqual(p.seeks, [21]);
});

test('malformed or unrelated recovery state is discarded', () => {
  for (const raw of ['{', JSON.stringify({ t: 'bad', playing: false }),
    JSON.stringify({ id: 'other-track', t: 42, playing: false })]) {
    const p = page({ storage: new Map([[RECOVERY_KEY, raw]]) });
    assert.equal(p.storage.has(RECOVERY_KEY), false);
    assert.deepEqual(p.seeks, []);
    assert.equal(p.video.starts, 0);
  }
});
