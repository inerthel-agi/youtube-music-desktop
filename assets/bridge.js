// Injected into music.youtube.com (top frame only). Playback is event-driven:
// reports playback state from the <video> element and the Media Session
// metadata the page already publishes to Windows (same data SMTC shows).
(() => {
  if (window.top !== window || location.hostname !== 'music.youtube.com') return;
  const invoke = window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke;
  if (!invoke) return;

  // Never forward messages, stacks or source URLs: they can contain listening history.
  const errorNames = ['Error', 'TypeError', 'ReferenceError', 'SyntaxError', 'RangeError',
    'EvalError', 'URIError', 'DOMException', 'NotSupportedError', 'SecurityError', 'AbortError'];
  let errorTimes = [];
  const reportError = (kind, error, message) => {
    const now = performance.now();
    errorTimes = errorTimes.filter((t) => now - t < 60000);
    if (errorTimes.length >= 5) return;
    errorTimes.push(now);
    let name = 'Other';
    try {
      if (errorNames.includes(error?.name)) name = error.name;
      if (/Trusted(?:HTML|Script|ScriptURL)|Trusted Types/.test(message || error?.message || '')) name = 'TrustedTypes';
    } catch (_) {}
    invoke('player_event', { msg: '{}', error: `${kind}:${name}` }).catch(() => {});
  };
  window.addEventListener('error', (e) => reportError('error', e.error, e.message));
  window.addEventListener('unhandledrejection', (e) => reportError('unhandledrejection', e.reason));

  // Consume once, and only restore on the matching track's document.
  const RECOVERY_KEY = 'ytm-desktop-recovery';
  const validRecovery = (r) => r && typeof r.id === 'string' && r.id.length > 0 && r.id.length <= 128
    && Number.isFinite(r.t) && r.t >= 0 && r.t <= 86400 && typeof r.playing === 'boolean';
  let recovery = null;
  try {
    const raw = sessionStorage.getItem(RECOVERY_KEY);
    if (raw !== null) {
      sessionStorage.removeItem(RECOVERY_KEY);
      if (raw.length <= 512) {
        const saved = JSON.parse(raw);
        if (validRecovery(saved) && new URL(location.href).searchParams.get('v') === saved.id) recovery = saved;
      }
    }
  } catch (e) { reportError('error', e); }

  let video = null;
  let restorePlayback = null;
  let last = '';
  let queued = false;
  // Last position sent and when, to detect drift (see checkDrift).
  let sent = { position: 0, playing: false, at: 0 };
  let shellMissing = [];
  let nativeVisible = true;
  const emptySince = new Map();
  const trackLoaded = () => !!navigator.mediaSession?.metadata?.title || !!video && video.readyState >= 1;

  // Track / artist / album links (validated again by the app). The video id comes
  // from the player; artist and album from the player bar's byline links.
  const abs = (href) => (href ? new URL(href, location.origin).href : '');
  const links = (player) => {
    let id = '';
    try { id = (player && player.getVideoData && player.getVideoData().video_id) || ''; } catch (_) {}
    const by = [...document.querySelectorAll('ytmusic-player-bar .byline a[href]')];
    const artist = by.find((a) => /channel\/|browse\/UC/.test(a.getAttribute('href')));
    const album = by.find((a) => /browse\/MPRE/.test(a.getAttribute('href')));
    return {
      track_url: id ? `https://music.youtube.com/watch?v=${encodeURIComponent(id)}` : '',
      artist_url: abs(artist && artist.getAttribute('href')),
      album_url: abs(album && album.getAttribute('href')),
      album_name: (album && album.textContent.trim()) || '',
    };
  };

  // Track time. YouTube Music streams the whole queue into one <video>: its
  // currentTime keeps counting across tracks and its duration (like the player's
  // getDuration()) grows while the track loads. The player bar's slider has the
  // real length (aria-valuemax) and getCurrentTime() the position in the track.
  const progressBar = () => document.querySelector('ytmusic-player-bar #progress-bar');
  // Second source: the "0:27 / 2:44" text of the player bar, in seconds.
  const timeText = () => {
    const t = document.querySelector('ytmusic-player-bar .time-info')?.textContent || '';
    const secs = (s) => s.trim().split(':').reduce((a, n) => a * 60 + Number(n), 0);
    const [pos, len] = t.split('/');
    return len ? { position: secs(pos), duration: secs(len) } : null;
  };
  const trackDuration = (player) => {
    let d = Number(progressBar()?.getAttribute('aria-valuemax')) || 0;
    if (!(d > 0)) d = timeText()?.duration || 0;
    if (!(d > 0)) try { d = (player && player.getDuration && player.getDuration()) || 0; } catch (_) {}
    if (!(d > 0) && video && isFinite(video.duration)) d = video.duration;
    return d > 0 ? Math.floor(d) : 0;
  };
  const trackPosition = () => {
    const player = document.getElementById('movie_player');
    try {
      if (player && player.getCurrentTime) return player.getCurrentTime() || 0;
    } catch (_) {}
    const t = timeText();
    if (t && t.position >= 0) return t.position;
    return video ? video.currentTime || 0 : 0;
  };

  // Capture at navigation time, not from the app's extrapolated playback state.
  window.__ytmRecover = (url) => {
    try {
      let target = new URL(url);
      if (target.origin !== location.origin) throw new Error('Invalid recovery origin');
      const track = new URL(links(document.getElementById('movie_player')).track_url);
      const saved = { id: track.searchParams.get('v'), t: trackPosition(),
        playing: !!video && !video.paused && !video.ended };
      if (!validRecovery(saved)) throw new Error('Recovery state unavailable');
      if (target.searchParams.get('v') !== saved.id) target = track;
      sessionStorage.setItem(RECOVERY_KEY, JSON.stringify(saved));
      location.replace(target.href);
      video?.pause(); // Freeze the old clock while navigation waits for the network.
    } catch (e) {
      try { sessionStorage.removeItem(RECOVERY_KEY); } catch (_) {}
      reportError('error', e); // Do not reload if the snapshot could not be saved.
    }
  };
  // YouTube Music rewrites some attributes on every playback tick with the same
  // value; reacting to those would send a message every second. Only real changes count.
  const watchAttr = (el, name) => {
    let value = el.getAttribute(name);
    new MutationObserver(() => {
      const v = el.getAttribute(name);
      if (v !== value) { value = v; schedule(); }
    }).observe(el, { attributes: true, attributeFilter: [name] });
  };
  // The slider's length can arrive after the metadata: watch that one attribute.
  let barEl = null;
  const watchBar = () => {
    const el = progressBar();
    if (el && el !== barEl) {
      barEl = el;
      watchAttr(el, 'aria-valuemax');
    }
  };

  // Rating shown by the player bar: "like", "dislike" or "indifferent". The
  // element can appear after the video, so it is observed once found.
  let likeEl = null;
  const likeStatus = () => {
    const el = document.querySelector('ytmusic-player-bar ytmusic-like-button-renderer');
    if (el && el !== likeEl) {
      likeEl = el;
      watchAttr(el, 'like-status');
    }
    return (el && el.getAttribute('like-status') || 'indifferent').toLowerCase();
  };

  // Page parts the app relies on. Reported (and logged by the app) when one is
  // missing on a loaded track, so a YouTube Music redesign shows up in the log.
  const missingParts = (player) => [
    ['progress-bar', progressBar()],
    ['time-info', document.querySelector('ytmusic-player-bar .time-info')],
    ['player-time', player && player.getCurrentTime],
    ['like', document.querySelector('ytmusic-player-bar #button-shape-like button')],
    ['next', document.querySelector('ytmusic-player-bar .next-button')],
    // Targets of the app's commands (ytm_core::Command::to_js) and of titlebar.js.
    ['previous', document.querySelector('ytmusic-player-bar .previous-button')],
    ['dislike', document.querySelector('ytmusic-player-bar #button-shape-dislike button')],
    ['shuffle', document.querySelector('ytmusic-player-bar .shuffle')],
    ['repeat', document.querySelector('ytmusic-player-bar .repeat')],
  ].filter(([, found]) => !found).map(([name]) => name).concat(shellMissing);

  const send = () => {
    queued = false;
    const md = navigator.mediaSession && navigator.mediaSession.metadata;
    const player = document.getElementById('movie_player');
    const { album_name, ...urls } = links(player);
    const msg = JSON.stringify({
      playing: (!recovery || recovery.playing) && !!video && !video.paused && !video.ended,
      ad: !!player && player.classList.contains('ad-showing'),
      title: (md && md.title) || '',
      artist: (md && md.artist) || '',
      // The page often leaves Media Session album empty; the player bar byline has it.
      album: (md && md.album) || album_name,
      // Largest cover the page published (the app keeps it only from Google image hosts).
      artwork: (md && md.artwork && md.artwork.length && md.artwork[md.artwork.length - 1].src) || '',
      ...urls,
      volume: video ? Math.round(video.volume * 100) : 100,
      // Sent on play/pause/seek only (no timer): the app extrapolates from these.
      position: Math.floor(trackPosition()),
      duration: (watchBar(), trackDuration(player)),
      like: likeStatus(),
      missing: trackLoaded() ? missingParts(player) : [],
    });
    if (msg === last) return; // unchanged: no IPC, no redraw
    last = msg;
    const s = JSON.parse(msg);
    sent = { position: s.position, playing: s.playing, at: performance.now() };
    const shell = shellMissing.length ? [innerWidth, innerHeight,
      ...['nav-bar', 'player-bar'].map((part) => document.querySelector(`ytmusic-${part}`)?.querySelectorAll('*').length || 0)] : null;
    invoke('player_event', { msg, shell }).catch(() => {});
  };
  // The app extrapolates the position from the last message. Some changes fire no
  // play/pause/seek event: the next track starting by itself (the message sent on
  // its metadata still carries the old track's position), buffering stalls, a
  // speed other than 1x. Compare on timeupdate (no IPC) and resend past 2 s.
  const checkDrift = () => {
    if (!video || queued) return;
    const expected = sent.playing ? sent.position + (performance.now() - sent.at) / 1000 : sent.position;
    if (Math.abs(trackPosition() - expected) > 2) schedule();
  };
  // Coalesce bursts (play + loadedmetadata + metadata set) into one message.
  const schedule = () => {
    if (!queued) {
      queued = true;
      queueMicrotask(send);
    }
  };

  const resetShell = () => {
    emptySince.clear();
    if (shellMissing.length) { shellMissing = []; schedule(); }
  };
  window.__ytmSetVisible = (visible) => { nativeVisible = visible; resetShell(); };
  document.addEventListener('visibilitychange', resetShell);

  // Also check while paused, when an empty shell may produce no playback event.
  // Count light-DOM descendants: the app's shadow buttons do not make a bar healthy.
  setInterval(() => {
    restorePlayback?.();
    const now = performance.now();
    const missing = [];
    for (const part of ['nav-bar', 'player-bar']) {
      if (document.hidden || !nativeVisible || !trackLoaded()) {
        emptySince.delete(part);
        continue;
      }
      const bar = document.querySelector(`ytmusic-${part}`);
      const controls = part === 'nav-bar'
        ? 'ytmusic-logo a, ytmusic-search-box input, ytmusic-search-box button, .right-content button'
        : '#play-pause-button, .time-info';
      const healthy = bar && bar.querySelectorAll('*').length >= 10
        && [...bar.querySelectorAll(controls)].some((el) =>
          el.checkVisibility({ checkOpacity: true, checkVisibilityCSS: true }));
      if (healthy) {
        emptySince.delete(part);
      } else {
        if (!emptySince.has(part)) emptySince.set(part, now);
        if (now - emptySince.get(part) >= 6000) missing.push(part);
      }
    }
    if (missing.join() !== shellMissing.join()) {
      shellMissing = missing;
      schedule();
    }
  }, 2000);

  const restoreRecovery = (v, saved) => {
    let playing = saved.playing, positioned = false, requested = false;
    const events = ['play', 'playing', 'loadedmetadata', 'durationchange', 'seeked', 'timeupdate'];
    const finish = () => {
      for (const e of events) v.removeEventListener(e, update);
      document.removeEventListener('pointerdown', intent, true);
      document.removeEventListener('keydown', intent, true);
      delete window.__ytmRestorePlayback;
      restorePlayback = null;
      recovery = null;
    };
    window.__ytmRestorePlayback = (value) => {
      if (typeof value !== 'boolean') return;
      playing = value;
      saved.playing = value;
      if (positioned && playing) finish();
    };
    const intent = (e) => {
      const control = e.type === 'pointerdown' ? e.target?.closest?.('#play-pause-button')
        : [' ', 'k', 'K', 'MediaPlayPause'].includes(e.key)
          && !e.target?.closest?.('input, textarea, [contenteditable="true"]');
      if (e.isTrusted && control) window.__ytmRestorePlayback?.(v.paused);
    };
    const update = () => {
      const player = document.getElementById('movie_player');
      const ad = !!player?.classList.contains('ad-showing');
      let id = '';
      try { id = player?.getVideoData?.().video_id || ''; } catch (_) {}
      if (positioned && id && id !== saved.id && !ad) { finish(); return; }
      // Pause on `play`, before `playing`; keep blocking later autoplay attempts.
      if (!playing || !positioned && !ad) v.pause();
      if (ad) {
        if (playing && v.paused) v.play().catch((e) => reportError('error', e));
        return;
      }
      if (id !== saved.id || v.readyState < 1 || !player?.seekTo) return;
      if (!positioned) {
        if ((requested || saved.t === 0) && Math.abs(trackPosition() - saved.t) < 1) positioned = true;
        else {
          if (!v.seeking) { requested = true; player.seekTo(saved.t, true); }
          return;
        }
      }
      if (playing) {
        finish();
        v.play().catch((e) => reportError('error', e));
      }
    };
    for (const e of events) v.addEventListener(e, update);
    document.addEventListener('pointerdown', intent, true);
    document.addEventListener('keydown', intent, true);
    restorePlayback = update;
    update();
  };

  // Media Session metadata has no change event: wrap its setter.
  const desc = window.MediaSession && Object.getOwnPropertyDescriptor(MediaSession.prototype, 'metadata');
  if (desc && desc.set) {
    Object.defineProperty(navigator.mediaSession, 'metadata', {
      configurable: true,
      get() { return desc.get.call(this); },
      set(m) { desc.set.call(this, m); schedule(); },
    });
  }

  const attach = (v) => {
    video = v;
    // Guard play before state listeners: microtasks can run between event callbacks.
    if (recovery) {
      window.__ytmResume = null; // Recovery supersedes the startup last-track position.
      restoreRecovery(v, recovery);
    }
    for (const e of ['play', 'pause', 'ended', 'volumechange', 'loadedmetadata', 'seeked', 'durationchange']) {
      v.addEventListener(e, schedule);
    }
    v.addEventListener('timeupdate', checkDrift);
    // The byline (artist, album, year) can render after the metadata: watch it too.
    // Only this element, not the whole bar (its time display changes every second).
    const byline = document.querySelector('ytmusic-player-bar .byline');
    if (byline) {
      new MutationObserver(schedule).observe(byline, { childList: true, subtree: true, characterData: true });
    }
    // Ad start/end only shows as a class on the player element (whose class list
    // also changes for buffering, hover...): react to ad-showing only.
    const player = document.getElementById('movie_player');
    if (player) {
      let ad = player.classList.contains('ad-showing');
      new MutationObserver(() => {
        const now = player.classList.contains('ad-showing');
        if (now !== ad) { ad = now; restorePlayback?.(); schedule(); }
      }).observe(player, { attributes: true, attributeFilter: ['class'] });
    }
    // "Resume on start": the app opened the last track; on its first play, stop
    // and go back to where it was (the user presses play).
    const resume = window.__ytmResume;
    if (resume) {
      const once = () => {
        if (document.getElementById('movie_player')?.classList.contains('ad-showing')) return;
        v.removeEventListener('playing', once);
        window.__ytmResume = null;
        v.pause();
        // Seek through the player: the <video> clock is not the track's.
        const player = document.getElementById('movie_player');
        const length = trackDuration(player) || Infinity;
        if (resume.t > 0 && resume.t < length && player && player.seekTo) player.seekTo(resume.t, true);
      };
      v.addEventListener('playing', once);
    }
    watchOutputs();
    schedule();
  };

  // Options > Playback "pause when headphones are unplugged": an audio output
  // disappearing pauses playback. Device count only (no labels, no permission).
  const watchOutputs = () => {
    const md = navigator.mediaDevices;
    if (!md || !md.enumerateDevices) return;
    const count = () => md.enumerateDevices()
      .then((d) => d.filter((x) => x.kind === 'audiooutput').length)
      .catch(() => 0);
    let outputs = 0;
    count().then((n) => { outputs = n; });
    md.addEventListener('devicechange', async () => {
      const n = await count();
      if (n < outputs && window.__ytmPauseOnDeviceChange && video && !video.paused) video.pause();
      outputs = n;
    });
  };

  // YTM creates a single <video> once; observe until it exists, then stop.
  const found = document.querySelector('video');
  if (found) {
    attach(found);
  } else {
    const obs = new MutationObserver(() => {
      const v = document.querySelector('video');
      if (v) {
        obs.disconnect();
        attach(v);
      }
    });
    obs.observe(document, { childList: true, subtree: true });
  }
})();
