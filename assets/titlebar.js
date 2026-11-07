// Frameless window: the page itself is the title bar, so YouTube Music's own
// background gradient reaches the top edge. Adds Windows-style caption buttons
// (isolated in a shadow root), back / forward / home buttons after the logo,
// and makes the YTM top bar a drag region.
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
  // Material icons, 24x24.
  const NAV = [
    ['back', 'Back', 'M20 11H7.83l5.59-5.59L12 4l-8 8 8 8 1.41-1.41L7.83 13H20v-2z'],
    ['forward', 'Forward', 'M12 4l-1.41 1.41L16.17 11H4v2h12.17l-5.58 5.59L12 20l8-8z'],
    ['home', 'Home', 'M12 5.69l5 4.5V18h-2v-6H9v6H7v-7.81l5-4.5M12 3 2 12h3v8h6v-6h2v6h6v-8h3L12 3z'],
  ];
  const SVG = 'http://www.w3.org/2000/svg';

  const makeButton = (label, d, box, onClick) => {
    const b = document.createElement('button');
    b.setAttribute('aria-label', label);
    b.title = label;
    const svg = document.createElementNS(SVG, 'svg');
    svg.setAttribute('viewBox', `0 0 ${box} ${box}`);
    const path = document.createElementNS(SVG, 'path');
    path.setAttribute('d', d);
    svg.appendChild(path);
    b.appendChild(svg);
    b.addEventListener('click', onClick);
    return b;
  };

  const shadowStyle = (root, css) => {
    const style = document.createElement('style');
    style.textContent = css;
    root.appendChild(style);
  };

  const mountButtons = () => {
    const host = document.createElement('div');
    host.style.cssText =
      `position:fixed;top:0;right:0;z-index:2147483647;height:32px;width:${BUTTON_W * BUTTONS.length}px`;
    const root = host.attachShadow({ mode: 'closed' });
    shadowStyle(root, `
      :host{all:initial}
      div{display:flex;height:32px}
      button{width:${BUTTON_W}px;height:32px;border:0;padding:0;background:transparent;color:#f1f1f1;
        display:grid;place-items:center}
      button:hover{background:rgba(255,255,255,.1)}
      button.close:hover{background:#c42b1c;color:#fff}
      svg{width:10px;height:10px;fill:currentColor;fill-rule:evenodd}
      svg.gear{width:15px;height:15px}`);
    const row = document.createElement('div');
    for (const [action, label, d, box] of BUTTONS) {
      const b = makeButton(label, d, box, () => invoke('window_control', { action }).catch(() => {}));
      if (action === 'close') b.className = 'close';
      if (box === 24) b.firstChild.setAttribute('class', 'gear');
      row.appendChild(b);
    }
    root.appendChild(row);
    document.documentElement.appendChild(host);

    // Keep YTM's own top-right controls (avatar, menu) clear of the caption buttons.
    // YTM gives its left part a fixed width (148 px with the side menu collapsed):
    // let it grow to fit the back / forward / home buttons instead of covering the
    // search box. Below 936 px the logo gives them its place (home does what the
    // logo link does).
    const pad = document.createElement('style');
    pad.textContent = `ytmusic-nav-bar .right-content{margin-right:${BUTTON_W * BUTTONS.length - 16}px!important}
      ytmusic-nav-bar .left-content{min-width:max-content!important}
      @media (max-width:935px){ytmusic-nav-bar ytmusic-logo{display:none!important}.ytm-nav{margin-left:0!important}}`;
    document.head.appendChild(pad);
  };

  // Back / forward follow the page's own history (YTM navigates with pushState).
  const goHome = () => {
    const logo = document.querySelector('ytmusic-nav-bar ytmusic-logo a');
    if (logo) logo.click(); // in-page navigation, playback continues
    else location.assign('/');
  };
  const NAV_ACTIONS = { back: () => history.back(), forward: () => history.forward(), home: goHome };

  let navHost = null;
  const mountNav = (bar) => {
    if (navHost && navHost.isConnected) return;
    const left = bar.querySelector('.left-content');
    navHost = document.createElement('div');
    navHost.className = 'ytm-nav';
    navHost.style.cssText = 'display:flex;align-items:center;flex:none;margin:0 8px 0 4px';
    const root = navHost.attachShadow({ mode: 'closed' });
    shadowStyle(root, `
      button{width:32px;height:32px;border:0;padding:0;border-radius:50%;background:transparent;
        color:#fff;display:grid;place-items:center;cursor:pointer}
      button:hover:not(:disabled){background:rgba(255,255,255,.1)}
      button:disabled{opacity:.35;cursor:default}
      svg{width:22px;height:22px;fill:currentColor}
      @media (max-width:935px){button{width:30px}}`);
    const buttons = {};
    for (const [action, label, d] of NAV) {
      buttons[action] = makeButton(label, d, 24, NAV_ACTIONS[action]);
      root.appendChild(buttons[action]);
    }
    const nav = window.navigation;
    const sync = () => {
      buttons.back.disabled = !nav.canGoBack;
      buttons.forward.disabled = !nav.canGoForward;
    };
    if (nav) {
      sync();
      nav.addEventListener('currententrychange', sync);
    }
    left.appendChild(navHost);
  };

  // Tauri's drag script reacts to elements carrying this attribute (not their children),
  // so mark the bar and its layout containers, not the buttons and search box.
  const DRAG = 'data-tauri-drag-region';
  const markNavBar = () => {
    const bar = document.querySelector('ytmusic-nav-bar');
    if (!bar || !bar.querySelector('.left-content')) return false;
    for (const el of [bar, ...bar.querySelectorAll('.left-content, .center-content, .right-content')]) {
      el.setAttribute(DRAG, '');
    }
    mountNav(bar);
    return true;
  };

  const start = () => {
    mountButtons();
    if (markNavBar()) return;
    const obs = new MutationObserver(() => markNavBar() && obs.disconnect());
    obs.observe(document.body, { childList: true, subtree: true });
    // The bar renders within seconds. Still missing after a minute, it is gone from
    // the page (bridge.js reports "nav-bar" to the log): stop watching every change.
    setTimeout(() => obs.disconnect(), 60000);
  };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start, { once: true });
  else start();
})();
