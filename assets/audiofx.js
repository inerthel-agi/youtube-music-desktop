// Playback speed and audio processing (Options > Playback / Sound / Equalizer).
// Driven by the app through window.__ytmFx.apply(cfg). The Web Audio graph is
// built only when an effect is first enabled, so it costs nothing by default.
// Ads always play at normal speed: speed and pitch never alter or shorten them.
//
// Graph: media -> preamp -> 10-band EQ -> bass shelf -> (dry + reverb) -> mix
//        -> mono/stereo -> pan (balance + "8D" LFO) -> [compressor] -> [limiter] -> out
(() => {
  if (window.top !== window || location.hostname !== 'music.youtube.com') return;

  const BANDS = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
  let cfg = {
    speed: 1, preserve_pitch: true, reverb: 0, bass_db: 0, normalize: false,
    eq: BANDS.map(() => 0), preamp_db: 0, balance: 0, mono: false, spatial: false, limiter: false,
  };
  let video = null;
  let g = null;

  const adShowing = () => {
    const p = document.getElementById('movie_player');
    return !!p && p.classList.contains('ad-showing');
  };
  const needsGraph = (c) =>
    c.reverb > 0 || c.bass_db > 0 || c.normalize || c.preamp_db !== 0 || c.eq.some((v) => v !== 0)
    || c.balance !== 0 || c.mono || c.spatial || c.limiter;
  const db = (v) => Math.pow(10, v / 20);

  // Synthetic room: decaying stereo noise, about 2.5 s.
  const impulse = (ctx) => {
    const len = Math.floor(ctx.sampleRate * 2.5);
    const buf = ctx.createBuffer(2, len, ctx.sampleRate);
    for (let ch = 0; ch < 2; ch++) {
      const d = buf.getChannelData(ch);
      for (let i = 0; i < len; i++) d[i] = (Math.random() * 2 - 1) * Math.pow(1 - i / len, 3);
    }
    return buf;
  };

  const build = (v) => {
    const ctx = new AudioContext();
    const src = ctx.createMediaElementSource(v); // permanent: the element now plays through ctx
    const pre = ctx.createGain();
    const eq = BANDS.map((f, i) => {
      const b = ctx.createBiquadFilter();
      b.type = i === 0 ? 'lowshelf' : i === BANDS.length - 1 ? 'highshelf' : 'peaking';
      b.frequency.value = f;
      b.Q.value = 1.41; // one octave
      return b;
    });
    const bass = ctx.createBiquadFilter();
    bass.type = 'lowshelf';
    bass.frequency.value = 120;
    const dry = ctx.createGain();
    const wet = ctx.createGain();
    const mix = ctx.createGain();
    const mono = ctx.createGain();
    mono.channelInterpretation = 'speakers';
    const pan = ctx.createStereoPanner();
    const lfo = ctx.createOscillator();
    lfo.frequency.value = 0.08; // one full left-right circle every ~12 s
    const depth = ctx.createGain();
    depth.gain.value = 0.85;
    lfo.connect(depth);
    lfo.start();
    const comp = ctx.createDynamicsCompressor();
    comp.threshold.value = -24;
    comp.knee.value = 24;
    comp.ratio.value = 4;
    const limit = ctx.createDynamicsCompressor();
    limit.threshold.value = -1;
    limit.knee.value = 0;
    limit.ratio.value = 20;
    limit.attack.value = 0.002;
    limit.release.value = 0.06;

    src.connect(pre);
    let node = pre;
    for (const b of eq) node = node.connect(b);
    node.connect(bass);
    bass.connect(dry).connect(mix);
    wet.connect(mix);
    mix.connect(mono).connect(pan);
    g = { ctx, src, pre, eq, bass, dry, wet, conv: null, verb: false, mono, pan, depth, comp, limit,
      tail: null, lfoOn: false, on: true };
  };

  // Output tail: pan -> [compressor] -> [limiter] -> destination; rewired only on change.
  const wireTail = (normalize, limiter) => {
    const key = `${normalize}|${limiter}`;
    if (g.tail === key) return;
    g.pan.disconnect();
    g.comp.disconnect();
    g.limit.disconnect();
    let node = g.pan;
    if (normalize) node = node.connect(g.comp);
    if (limiter) node = node.connect(g.limit);
    node.connect(g.ctx.destination);
    g.tail = key;
  };

  const applyRate = () => {
    if (!video) return;
    const ad = adShowing();
    const rate = ad ? 1 : cfg.speed;
    const keep = ad ? true : cfg.preserve_pitch;
    if (video.preservesPitch !== keep) video.preservesPitch = keep;
    if (Math.abs(video.playbackRate - rate) > 0.001) video.playbackRate = rate;
  };

  const applyAll = () => {
    if (!video) return;
    applyRate();
    if (!g && !needsGraph(cfg)) return;
    if (!g) build(video);
    const on = needsGraph(cfg);
    // The source cannot leave the context once captured: with every effect back
    // to neutral it goes straight to the output and the chain idles.
    if (on !== g.on) {
      g.src.disconnect();
      g.src.connect(on ? g.pre : g.ctx.destination);
      g.on = on;
    }
    if (g.ctx.state === 'suspended') g.ctx.resume().catch(() => {});
    if (!on) return;
    g.pre.gain.value = db(cfg.preamp_db);
    g.eq.forEach((b, i) => { b.gain.value = cfg.eq[i] || 0; });
    g.bass.gain.value = cfg.bass_db;
    g.wet.gain.value = cfg.reverb;
    // The convolver (2.5 s impulse) costs CPU on every sample even at zero mix:
    // created on first use, connected only while reverb is on.
    const verb = cfg.reverb > 0;
    if (verb !== g.verb) {
      if (verb && !g.conv) {
        g.conv = g.ctx.createConvolver();
        g.conv.buffer = impulse(g.ctx);
        g.conv.connect(g.wet);
      }
      if (verb) g.bass.connect(g.conv);
      else g.bass.disconnect(g.conv);
      g.verb = verb;
    }
    g.dry.gain.value = 1 - cfg.reverb * 0.4;
    // Mono: force one channel through this node, then upmix to stereo again.
    g.mono.channelCount = cfg.mono ? 1 : 2;
    g.mono.channelCountMode = cfg.mono ? 'explicit' : 'max';
    g.pan.pan.value = cfg.balance;
    if (cfg.spatial !== g.lfoOn) {
      if (cfg.spatial) g.depth.connect(g.pan.pan);
      else g.depth.disconnect();
      g.lfoOn = cfg.spatial;
    }
    wireTail(cfg.normalize, cfg.limiter);
  };

  const attach = (v) => {
    video = v;
    // YTM resets the rate on each track; ads start/end flip the class below.
    for (const e of ['loadedmetadata', 'ratechange']) v.addEventListener(e, applyRate);
    // An AudioContext created without a user gesture starts suspended: resume on play.
    v.addEventListener('play', applyAll);
    const player = document.getElementById('movie_player');
    if (player) new MutationObserver(applyRate).observe(player, { attributes: true, attributeFilter: ['class'] });
    applyAll();
  };

  window.__ytmFx = {
    // Diagnostics: whether the graph exists and its AudioContext state.
    status: () => ({ graph: !!g, context: g ? g.ctx.state : null }),
    apply(c) {
      cfg = { ...cfg, ...c, eq: Array.isArray(c.eq) && c.eq.length === 10 ? c.eq : cfg.eq };
      applyAll();
    },
  };

  const found = document.querySelector('video');
  if (found) attach(found);
  else {
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
