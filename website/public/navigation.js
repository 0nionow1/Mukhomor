(() => {
  'use strict';
  // Runs before external stylesheets. The critical cover CSS is already inline,
  // so an arriving document is covered before its body or fonts can paint.
  const KEY = 'mukhomor.navigation';
  const TTL = 15000;
  const DURATION = 575;
  const FONT_WAIT = 1800;
  const root = document.documentElement;
  const reduced = matchMedia('(prefers-reduced-motion: reduce)');
  const base = new URL('./', document.currentScript.src);
  const pages = new Set(['', 'index.html', 'docs.html', 'releases.html', 'changelog.html', 'policy.html', 'privacy.html', 'faq.html']);
  const route = url => url.pathname + url.search + url.hash;
  const page = url => url.pathname.replace(/index\.html$/, '');
  let savedPaused = false;
  try { savedPaused = localStorage.getItem('mukhomor-site-motion') === 'paused'; } catch { /* Optional preference. */ }
  const paused = () => reduced.matches || savedPaused;
  const forget = () => { try { sessionStorage.removeItem(KEY); } catch { /* Storage may be disabled. */ } };
  const remember = url => {
    try {
      const value = JSON.stringify({ to: route(url), at: Date.now() });
      sessionStorage.setItem(KEY, value);
      return sessionStorage.getItem(KEY) === value;
    } catch { return false; }
  };

  let incoming = false;
  try {
    const value = JSON.parse(sessionStorage.getItem(KEY));
    const type = performance.getEntriesByType('navigation')[0]?.type;
    const age = Date.now() - value?.at;
    incoming = !paused() && type !== 'reload' && type !== 'back_forward' &&
      value?.to === route(location) && Number.isFinite(age) && age >= 0 && age < TTL;
  } catch { /* Direct navigation is always available. */ }
  forget();
  if (incoming) root.classList.add('navigation-covered');

  let busy = incoming, canvas, context, frame = 0, alarm = 0, generation = 0;
  let width = 0, height = 0, tile = 0, cells = [], cover = 0, finish;
  const stop = () => {
    generation++;
    cancelAnimationFrame(frame);
    clearTimeout(alarm);
    finish = null;
  };
  const reset = () => {
    stop();
    root.classList.remove('navigation-covered');
    canvas?.remove();
    canvas = context = finish = null;
    document.querySelector('.intro-scene')?.classList.remove('is-leaving');
    incoming = busy = false;
  };
  const resize = () => {
    if (!canvas) return;
    width = innerWidth; height = innerHeight; tile = width < 600 ? 20 : 32;
    canvas.width = width; canvas.height = height;
    const columns = Math.ceil(width / tile), rows = Math.ceil(height / tile);
    cells = [];
    for (let y = 0; y < rows; y++) for (let x = 0; x < columns; x++) {
      cells.push({ x, y, threshold: ((rows - 1 - y) / rows) * .57 +
        Math.abs(x / columns - .5) * .35 + ((x * 19 + y * 37) % 17) / 130 });
    }
  };
  const draw = value => {
    cover = value;
    if (!context) return;
    context.clearRect(0, 0, width, height);
    for (const cell of cells) {
      const age = value - cell.threshold;
      if (age <= 0) continue;
      context.fillStyle = age < .035 ? '#16f5d1' : age < .075 ? '#ff52b9' : '#120c18';
      context.fillRect(cell.x * tile, cell.y * tile, tile, tile);
    }
  };
  const prepare = () => {
    try {
      canvas = document.createElement('canvas');
      context = canvas.getContext('2d');
      if (!context) { reset(); return false; }
      canvas.className = 'pixel-transition';
      canvas.setAttribute('aria-hidden', 'true');
      document.body.append(canvas);
      resize();
      return true;
    } catch { reset(); return false; }
  };
  const animate = (from, to, done) => {
    const started = performance.now(), current = generation;
    let ended = false;
    const complete = () => {
      if (ended || current !== generation) return;
      ended = true;
      cancelAnimationFrame(frame);
      clearTimeout(alarm);
      finish = null;
      draw(to);
      done();
    };
    finish = complete;
    const step = now => {
      if (current !== generation) return;
      const progress = Math.min(1, (now - started) / DURATION);
      if (progress === 1 || paused()) { complete(); return; }
      draw(from + (to - from) * progress);
      frame = requestAnimationFrame(step);
    };
    draw(from);
    // A throttled rAF must not strand a keyboard/background-tab navigation.
    alarm = setTimeout(complete, DURATION + 200);
    frame = requestAnimationFrame(step);
  };
  const reveal = () => {
    if (paused()) { reset(); return; }
    const current = generation;
    let started = false;
    // Keep both layers opaque for a rendered frame. Removing the CSS cover in
    // the same task that creates the canvas can expose the document at commit.
    root.classList.add('navigation-covered');
    draw(1);
    const start = () => {
      if (started || current !== generation) return;
      started = true;
      cancelAnimationFrame(frame);
      clearTimeout(alarm);
      root.classList.remove('navigation-covered');
      animate(1, 0, reset);
    };
    finish = start;
    // Background tabs may not get animation frames; do not trap navigation.
    alarm = setTimeout(start, 250);
    frame = requestAnimationFrame(() => { frame = requestAnimationFrame(start); });
  };
  const go = url => {
    // Refresh immediately before navigation, after the closing half finishes.
    if (!remember(url) || paused()) { forget(); reset(); }
    else {
      root.classList.add('navigation-covered');
      // Handles cancelled/unresponsive navigations without trapping the page.
      alarm = setTimeout(() => { forget(); reset(); }, 8000);
    }
    try { location.assign(url.href); } catch { forget(); reset(); }
  };
  const arriveAtStory = () => {
    const target = document.querySelector('#story');
    if (!target) return;
    if (location.hash !== '#story') history.pushState(null, '', '#story');
    window.scrollTo({ top: target.getBoundingClientRect().top + window.scrollY, behavior: 'instant' });
    document.querySelector('#story-heading')?.focus({ preventScroll: true });
  };

  document.addEventListener('click', event => {
    if (event.defaultPrevented || event.button !== 0 || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
    const link = event.target.closest?.('a[href]');
    if (!link || link.hasAttribute('download') || (link.target && link.target !== '_self')) return;
    const url = new URL(link.href, location.href);
    if (url.origin !== base.origin || !url.pathname.startsWith(base.pathname) ||
      !pages.has(url.pathname.slice(base.pathname.length))) return;
    const samePage = page(url) === page(location) && url.search === location.search;
    const story = samePage && url.hash === '#story' && link.hasAttribute('data-enter');
    if (samePage && !story) return; // Native anchors, tabs, and skip links.
    if (busy) { event.preventDefault(); return; }
    if (story && paused()) { event.preventDefault(); arriveAtStory(); return; }
    if (paused() || (!story && !remember(url))) return;
    if (!prepare()) { forget(); return; }
    event.preventDefault();
    busy = true;
    document.querySelector('.intro-scene')?.classList.add('is-leaving');
    animate(0, 1, () => {
      if (story) { arriveAtStory(); reveal(); }
      else go(url);
    });
  });

  const ready = async () => {
    if (!incoming) return;
    const current = generation;
    // Both local fonts settle under the cover, including a cold-cache load.
    // Keep the deadline bounded if a resource fails or the connection stalls.
    let fontAlarm;
    try {
      await Promise.race([document.fonts?.ready, new Promise(resolve => { fontAlarm = setTimeout(resolve, FONT_WAIT); })]);
    } finally { clearTimeout(fontAlarm); }
    if (current !== generation || !incoming) return;
    clearTimeout(alarm);
    if (paused() || !prepare()) { reset(); return; }
    reveal();
  };
  if (incoming) alarm = setTimeout(reset, 8000); // Fail open even if DOM parsing stalls.
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', ready, { once: true });
  else ready();
  window.addEventListener('resize', () => { resize(); draw(cover); }, { passive: true });
  window.addEventListener('pageshow', event => { if (event.persisted) { forget(); reset(); } });
  // The browser can still present the outgoing surface during document commit.
  // Freeze its covered frame; clear it only if that document returns via BFCache.
  window.addEventListener('pagehide', stop);
  // Native history traversal keeps its scroll restoration; drop a pending wipe.
  window.addEventListener('popstate', () => { forget(); reset(); });
  reduced.addEventListener('change', () => { if (paused()) { if (finish) finish(); else reset(); } });
  document.addEventListener('mukhomor-motion', event => {
    savedPaused = event.detail.paused;
    if (paused()) { if (finish) finish(); else reset(); }
  });
})();
