// Memory: YouTube Music registers a service worker (offline cache, push), which
// costs a separate renderer process. The desktop app does not need it: refuse
// new registrations and remove existing ones. Playback and library are unaffected.
(() => {
  if (window.top !== window || location.hostname !== 'music.youtube.com' || !('serviceWorker' in navigator)) return;
  const sw = navigator.serviceWorker;
  try {
    Object.defineProperty(sw, 'register', {
      configurable: true,
      value: () => Promise.reject(new DOMException('Service workers are disabled in this app', 'NotSupportedError')),
    });
  } catch (_) {}
  sw.getRegistrations().then((regs) => regs.forEach((r) => r.unregister())).catch(() => {});
})();
