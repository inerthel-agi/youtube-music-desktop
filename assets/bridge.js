// Injected into music.youtube.com (top frame only). Event-driven, no polling:
// reports playback state from the <video> element and the Media Session
// metadata the page already publishes to Windows (same data SMTC shows).
(() => {
  if (window.top !== window || location.hostname !== 'music.youtube.com') return;
  const invoke = window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke;
  if (!invoke) return;

  let video = null;
  let last = '';
  let queued = false;
  // Last position sent and when, to detect drift (see checkDrift).
  let sent = { position: 0, playing: false, at: 0 };

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

  // The player knows the track length as soon as the track is chosen; the <video>
  // (MSE) often reports NaN/0 for a moment, which made Discord show "0:00" instead
  // of the progress bar.
  const trackDuration = (player) => {
    let d = 0;
    try { d = (player && player.getDuration && player.getDuration()) || 0; } catch (_) {}
    if (!(d > 0) && video && isFinite(video.duration)) d = video.duration;
    return d > 0 ? Math.floor(d) : 0;
  };

  // Rating shown by the player bar: "like", "dislike" or "indifferent". The
  // element can appear after the video, so it is observed once found.
  let likeEl = null;
  const likeStatus = () => {
    const el = document.querySelector('ytmusic-player-bar ytmusic-like-button-renderer');
    if (el && el !== likeEl) {
      likeEl = el;
      new MutationObserver(schedule).observe(el, { attributes: true, attributeFilter: ['like-status'] });
    }
    return (el && el.getAttribute('like-status') || 'indifferent').toLowerCase();
  };

  const send = () => {
    queued = false;
    const md = navigator.mediaSession && navigator.mediaSession.metadata;
    const player = document.getElementById('movie_player');
    const { album_name, ...urls } = links(player);
    const msg = JSON.stringify({
      playing: !!video && !video.paused && !video.ended,
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
      position: video ? Math.floor(video.currentTime || 0) : 0,
      duration: trackDuration(player),
      like: likeStatus(),
    });
    if (msg === last) return; // unchanged: no IPC, no redraw
    last = msg;
    const s = JSON.parse(msg);
    sent = { position: s.position, playing: s.playing, at: performance.now() };
    invoke('player_event', { msg }).catch(() => {});
  };
  // The app extrapolates the position from the last message. Some changes fire no
  // play/pause/seek event: the next track starting by itself (the message sent on
  // its metadata still carries the old track's position), buffering stalls, a
  // speed other than 1x. Compare on timeupdate (no IPC) and resend past 2 s.
  const checkDrift = () => {
    if (!video || queued) return;
    const expected = sent.playing ? sent.position + (performance.now() - sent.at) / 1000 : sent.position;
    if (Math.abs(video.currentTime - expected) > 2) schedule();
  };
  // Coalesce bursts (play + loadedmetadata + metadata set) into one message.
  const schedule = () => {
    if (!queued) {
      queued = true;
      queueMicrotask(send);
    }
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
    // Ad start/end only shows as a class on the player element.
    const player = document.getElementById('movie_player');
    if (player) {
      new MutationObserver(schedule).observe(player, { attributes: true, attributeFilter: ['class'] });
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
        if (resume.t > 0 && resume.t < (v.duration || Infinity)) v.currentTime = resume.t;
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
