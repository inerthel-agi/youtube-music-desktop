// Frameless window: the page itself is the title bar, so YouTube Music's own
// background gradient reaches the top edge. Adds Windows-style caption buttons
// (isolated in a shadow root) and makes the YTM top bar a drag region.
// DOM APIs only: YouTube enforces Trusted Types, so innerHTML strings throw.
(() => {
  if (window.top !== window || location.hostname !== 'music.youtube.com') return;
  const invoke = window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke;
  if (!invoke) return;

  const BUTTON_W = 46; // Windows caption button size
  const GEAR =
    'M19.14 12.94c.04-.3.06-.61.06-.94 0-.32-.02-.64-.07-.94l2.03-1.58a.49.49 0 0 0 .12-.61l-1.92-3.32a.49.49 0 0 0-.59-.22l-2.39.96a7 7 0 0 0-1.62-.94l-.36-2.54a.48.48 0 0 0-.48-.41h-3.84a.47.47 0 0 0-.47.41l-.36 2.54a7.4 7.4 0 0 0-1.62.94l-2.39-.96a.48.48 0 0 0-.59.22L2.74 8.87a.47.47 0 0 0 .12.61l2.03 1.58c-.05.3-.09.63-.09.94s.02.64.07.94l-2.03 1.58a.49.49 0 0 0-.12.61l1.92 3.32c.12.22.37.29.59.22l2.39-.96c.5.38 1.03.7 1.62.94l.36 2.54c.05.24.24.41.48.41h3.84c.24 0 .44-.17.47-.41l.36-2.54a7 7 0 0 0 1.62-.94l2.39.96c.22.08.47 0 .59-.22l1.92-3.32a.48.48 0 0 0-.12-.61zM12 15.6a3.6 3.6 0 1 1 0-7.2 3.6 3.6 0 0 1 0 7.2z';
  // [action, label, path, viewBox size]
  const BUTTONS = [
    ['options', 'Options', GEAR, 24],
    ['minimize', 'Minimize', 'M0 5h10v1H0z', 10],
    ['toggle_maximize', 'Maximize', 'M0 0v10h10V0zm1 1h8v8H1z', 10],
    ['close', 'Close', 'M.7 0 5 4.3 9.3 0l.7.7L5.7 5 10 9.3l-.7.7L5 5.7.7 10 0 9.3 4.3 5 0 .7z', 10],
  ];
  const SVG = 'http://www.w3.org/2000/svg';

  const mountButtons = () => {
    const host = document.createElement('div');
    host.style.cssText =
      `position:fixed;top:0;right:0;z-index:2147483647;height:32px;width:${BUTTON_W * BUTTONS.length}px`;
    const root = host.attachShadow({ mode: 'closed' });
    const style = document.createElement('style');
    style.textContent = `
      :host{all:initial}
      div{display:flex;height:32px}
      button{width:${BUTTON_W}px;height:32px;border:0;padding:0;background:transparent;color:#f1f1f1;
        display:grid;place-items:center}
      button:hover{background:rgba(255,255,255,.1)}
      button.close:hover{background:#c42b1c;color:#fff}
      svg{width:10px;height:10px;fill:currentColor;fill-rule:evenodd}
      svg.gear{width:15px;height:15px}`;
    const row = document.createElement('div');
    for (const [action, label, d, box] of BUTTONS) {
      const b = document.createElement('button');
      b.className = action === 'close' ? 'close' : '';
      b.setAttribute('aria-label', label);
      b.title = label;
      const svg = document.createElementNS(SVG, 'svg');
      svg.setAttribute('viewBox', `0 0 ${box} ${box}`);
      if (box === 24) svg.setAttribute('class', 'gear');
      const path = document.createElementNS(SVG, 'path');
      path.setAttribute('d', d);
      svg.appendChild(path);
      b.appendChild(svg);
      b.addEventListener('click', () => invoke('window_control', { action }).catch(() => {}));
      row.appendChild(b);
    }
    root.append(style, row);
    document.documentElement.appendChild(host);

    // Keep YTM's own top-right controls (avatar, menu) clear of the caption buttons.
    const pad = document.createElement('style');
    pad.textContent = `ytmusic-nav-bar .right-content{margin-right:${BUTTON_W * BUTTONS.length - 16}px!important}`;
    document.head.appendChild(pad);
  };

  // Tauri's drag script reacts to elements carrying this attribute (not their children),
  // so mark the bar and its layout containers, not the buttons and search box.
  const DRAG = 'data-tauri-drag-region';
  const markNavBar = () => {
    const bar = document.querySelector('ytmusic-nav-bar');
    if (!bar) return false;
    for (const el of [bar, ...bar.querySelectorAll('.left-content, .center-content, .right-content')]) {
      el.setAttribute(DRAG, '');
    }
    return true;
  };

  const start = () => {
    mountButtons();
    if (markNavBar()) return;
    const obs = new MutationObserver(() => markNavBar() && obs.disconnect());
    obs.observe(document.body, { childList: true, subtree: true });
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start, { once: true });
  else start();
})();
