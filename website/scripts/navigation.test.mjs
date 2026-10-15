import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import vm from 'node:vm';

const source = await readFile(new URL('../public/navigation.js', import.meta.url), 'utf8');
const KEY = 'mukhomor.navigation';
const epoch = 1800000000000;
const home = 'https://example.test/Mukhomor/';
const classes = () => {
  const values = new Set();
  return { add: v => values.add(v), remove: v => values.delete(v), contains: v => values.has(v) };
};
const events = () => {
  const listeners = new Map();
  return {
    addEventListener(type, fn) { listeners.set(type, [...listeners.get(type) || [], fn]); },
    emit(type, data = {}) { for (const fn of listeners.get(type) || []) fn(data); }
  };
};

function browser(options = {}) {
  let now = options.now || 0, id = 0;
  const storage = options.storage || new Map(), frames = new Map(), timers = new Map();
  const canvases = [], assignments = [], scrolls = [];
  const root = { classList: classes() }, scene = { classList: classes() };
  const heading = { focus() { heading.focused = true; } };
  const location = new URL(options.url || home);
  location.assign = url => assignments.push({ url, covered: root.classList.contains('navigation-covered'), rects: canvases.at(-1)?.rects.slice() });
  const document = {
    ...events(), documentElement: root, currentScript: { src: home + 'navigation.js' },
    readyState: 'loading', fonts: { ready: options.fonts || Promise.resolve() },
    body: { append(canvas) { canvas.attached = true; } },
    querySelector(selector) {
      return selector === '.intro-scene' ? scene : selector === '#story' ? { getBoundingClientRect: () => ({ top: 900 }) } : heading;
    },
    createElement() {
      const canvas = {
        rects: [], setAttribute() {}, remove() { canvas.attached = false; },
        getContext() {
          if (options.noCanvas) return null;
          return {
            clearRect() { canvas.rects = []; },
            fillRect(x, y, w, h) { canvas.rects.push({ x, y, w, h, color: this.fillStyle }); }
          };
        }
      };
      canvases.push(canvas);
      return canvas;
    }
  };
  const window = { ...events(), scrollTo: value => scrolls.push(value) };
  const reduced = { ...events(), matches: options.reduced || false };
  const context = vm.createContext({
    window, document, location, URL, innerWidth: 1366, innerHeight: 768,
    matchMedia: () => reduced,
    history: { pushState(state, title, hash) { location.hash = hash; } },
    localStorage: { getItem: () => options.paused ? 'paused' : null },
    sessionStorage: {
      getItem(key) { if (options.blocked) throw Error('blocked'); return storage.get(key) ?? null; },
      setItem(key, value) { if (options.blocked || options.quota) throw Error('blocked'); storage.set(key, value); },
      removeItem(key) { if (options.blocked) throw Error('blocked'); storage.delete(key); }
    },
    Date: { now: () => epoch + now },
    performance: { now: () => now, getEntriesByType: () => [{ type: options.type || 'navigate' }] },
    requestAnimationFrame(fn) { frames.set(++id, fn); return id; },
    cancelAnimationFrame(key) { frames.delete(key); },
    setTimeout(fn, ms) { timers.set(++id, { fn, at: now + ms }); return id; },
    clearTimeout(key) { timers.delete(key); }
  });
  vm.runInContext(source, context);
  const click = (href, attrs = {}, flags = {}) => {
    const link = { href: new URL(href, location).href, target: attrs.target, hasAttribute: name => Boolean(attrs[name]) };
    const event = { button: 0, target: { closest: () => link }, preventDefault() { this.defaultPrevented = true; }, ...flags };
    document.emit('click', event);
    return event.defaultPrevented || false;
  };
  const advance = (ms, raf = true) => {
    const end = now + ms;
    while (now < end) {
      now = Math.min(end, now + 25);
      if (raf) for (const [key, fn] of [...frames]) { frames.delete(key); fn(now); }
      for (const [key, timer] of [...timers]) if (timer.at <= now && timers.has(key)) { timers.delete(key); timer.fn(); }
    }
  };
  return {
    storage, root, scene, heading, location, window, document, reduced, canvases, assignments, scrolls, click, advance,
    get now() { return now; },
    get visible() { return root.classList.contains('navigation-covered') || canvases.some(c => c.attached); },
    async ready() { document.readyState = 'interactive'; document.emit('DOMContentLoaded'); await new Promise(setImmediate); },
    resize(w, h) { context.innerWidth = w; context.innerHeight = h; window.emit('resize'); }
  };
}
const handoff = (to = 'docs.html', age = 0) => new Map([[KEY, JSON.stringify({ to: '/Mukhomor/' + to, at: epoch - age })]]);
const assertSolid = rects => {
  assert.ok(rects.length > 0);
  assert.ok(rects.every(r => r.color === '#120c18'), 'handoff must have no transparent or accent cells');
};

test('real document handoff closes fully, consumes once before DOM, then reveals', async () => {
  const from = browser();
  assert.equal(from.click('docs.html#server'), true);
  from.advance(300);
  assert.equal(from.assignments.length, 0, 'do not change documents mid-wipe');
  from.advance(300);
  assert.equal(from.assignments.length, 1);
  assert.equal(from.assignments[0].covered, true);
  assertSolid(from.assignments[0].rects);
  const to = browser({ url: home + 'docs.html#server', storage: from.storage, now: from.now });
  assert.equal(to.visible, true, 'cover must exist while document is still parsing');
  assert.equal(to.canvases.length, 0);
  assert.equal(to.storage.has(KEY), false);
  await to.ready();
  assertSolid(to.canvases[0].rects);
  assert.equal(to.root.classList.contains('navigation-covered'), true, 'canvas must paint before the CSS cover is removed');
  to.advance(650);
  assert.equal(to.visible, false);
  assert.equal(to.location.hash, '#server');
});

test('About uses the same wipe and restores anchor plus keyboard focus', () => {
  const b = browser();
  b.click('#story', { 'data-enter': true });
  b.advance(600);
  assert.equal(b.location.hash, '#story');
  assert.equal(b.heading.focused, true);
  assert.equal(b.scrolls.length, 1);
  assert.equal(b.storage.has(KEY), false);
  b.advance(650);
  assert.equal(b.visible, false);
  assert.equal(b.scene.classList.contains('is-leaving'), false);
});

test('native link behavior is preserved outside the page-transition scope', () => {
  for (const [url, attrs, flags] of [
    ['https://github.com/0nionow1/Mukhomor'], ['#server'], ['./'],
    ['docs.html', { target: '_blank' }], ['docs.html', { download: true }],
    ['assets/app-preview.png'], ['/another-project/docs.html'],
    ...['ctrlKey', 'metaKey', 'shiftKey', 'altKey'].map(flag => ['docs.html', {}, { [flag]: true }]),
    ['docs.html', {}, { button: 1 }], ['docs.html', {}, { defaultPrevented: true }]
  ]) {
    const b = browser();
    const prevented = b.click(url, attrs, flags);
    assert.equal(prevented, Boolean(flags?.defaultPrevented), url);
    assert.equal(b.visible, false);
    assert.equal(b.storage.has(KEY), false);
  }
});

test('paused motion, blocked storage, quota and unavailable canvas degrade to real links', () => {
  for (const option of ['paused', 'reduced', 'blocked', 'quota', 'noCanvas']) {
    const b = browser({ [option]: true });
    assert.equal(b.click('docs.html'), false, option);
    assert.equal(b.visible, false);
    assert.equal(b.storage.has(KEY), false);
  }
  const b = browser({ reduced: true });
  b.click('#story', { 'data-enter': true });
  assert.equal(b.heading.focused, true);
  assert.equal(b.visible, false);
});

test('direct, stale, future, mismatched, reload and history entries never replay an old cover', () => {
  for (const options of [
    {}, { storage: handoff('docs.html', 15001) }, { storage: handoff('docs.html', -1) },
    { storage: handoff('faq.html') }, { storage: handoff(), type: 'reload' },
    { storage: handoff(), type: 'back_forward' }, { storage: handoff(), reduced: true },
    { storage: new Map([[KEY, '{bad-json']]) }
  ]) {
    const b = browser({ url: home + 'docs.html', ...options });
    assert.equal(b.visible, false);
    assert.equal(b.storage.has(KEY), false);
  }
});

test('double clicks cannot produce competing page loads', () => {
  const b = browser();
  b.click('docs.html');
  b.click('faq.html');
  b.advance(600);
  assert.equal(b.assignments.length, 1);
  assert.equal(b.assignments[0].url, home + 'docs.html');
});

test('parsing and font delays are bounded; cancelled load does not trap the old page', async () => {
  const parsing = browser({ url: home + 'docs.html', storage: handoff() });
  parsing.advance(8100);
  assert.equal(parsing.visible, false);
  await parsing.ready();
  assert.equal(parsing.visible, false, 'late ready must not restart the cover');
  const fonts = browser({ url: home + 'docs.html', storage: handoff(), fonts: new Promise(() => {}) });
  const ready = fonts.ready();
  fonts.advance(1850);
  await ready;
  fonts.advance(650);
  assert.equal(fonts.visible, false);
  const load = browser();
  load.click('docs.html');
  load.advance(9000);
  assert.equal(load.visible, false);
  assert.equal(load.storage.has(KEY), false);
  assert.equal(load.click('faq.html'), true);
});

test('BFCache and back during a wipe clear overlays and pending navigation', () => {
  for (const event of ['pageshow', 'popstate']) {
    const b = browser();
    b.click('docs.html');
    b.advance(200);
    b.window.emit(event, { persisted: true });
    b.advance(1000);
    assert.equal(b.assignments.length, 0);
    assert.equal(b.visible, false);
    assert.equal(b.storage.has(KEY), false);
  }
});

test('pagehide freezes the outgoing covered frame until BFCache restores it', () => {
  const b = browser();
  b.click('docs.html');
  b.advance(600);
  b.window.emit('pagehide', { persisted: true });
  b.advance(10000);
  assert.equal(b.root.classList.contains('navigation-covered'), true);
  assert.equal(b.canvases[0].attached, true);
  assertSolid(b.canvases[0].rects);
  assert.equal(b.storage.has(KEY), true, 'the arriving document must still receive its handoff');
  b.window.emit('pageshow', { persisted: true });
  assert.equal(b.visible, false);
  assert.equal(b.storage.has(KEY), false);
});

test('slow fonts stay hidden and the canvas gets a complete frame before reveal', async () => {
  let loaded;
  const b = browser({ url: home + 'docs.html', storage: handoff(), fonts: new Promise(resolve => { loaded = resolve; }) });
  await b.ready();
  b.advance(1200);
  assert.equal(b.root.classList.contains('navigation-covered'), true, 'do not expose fallback fonts at the old 400 ms deadline');
  loaded();
  await new Promise(setImmediate);
  assertSolid(b.canvases[0].rects);
  b.advance(25);
  assert.equal(b.root.classList.contains('navigation-covered'), true, 'first rAF must keep both opaque layers');
  b.advance(25);
  assert.equal(b.root.classList.contains('navigation-covered'), false);
  assertSolid(b.canvases[0].rects);
  b.advance(600);
  assert.equal(b.visible, false);
});

test('restoration cancels a reveal queued between animation frames', async () => {
  const b = browser({ url: home + 'docs.html', storage: handoff() });
  await b.ready();
  b.advance(25);
  b.window.emit('popstate');
  b.advance(1000);
  assert.equal(b.visible, false);
  assert.equal(b.assignments.length, 0);
});

test('motion can be disabled mid-wipe and throttled rAF does not prevent navigation', () => {
  const b = browser();
  b.click('docs.html');
  b.advance(100);
  b.reduced.matches = true;
  b.reduced.emit('change');
  assert.equal(b.assignments.length, 1);
  assert.equal(b.visible, false);
  assert.equal(b.storage.has(KEY), false);
  const background = browser();
  background.click('docs.html');
  background.advance(800, false);
  assert.equal(background.assignments.length, 1);
});

test('resizing a covered incoming page keeps every viewport cell opaque', async () => {
  const b = browser({ url: home + 'docs.html', storage: handoff() });
  await b.ready();
  b.resize(320, 740);
  const canvas = b.canvases[0];
  assert.equal(canvas.width, 320);
  assert.equal(canvas.height, 740);
  assertSolid(canvas.rects);
  assert.equal(canvas.rects.length, 16 * 37);
  b.advance(650);
  assert.equal(b.visible, false);
});

test('all pages paint the critical background and run handoff before external stylesheets', async () => {
  const dir = new URL('../public/', import.meta.url);
  const pages = (await readdir(dir)).filter(n => n.endsWith('.html'));
  assert.equal(pages.length, 7);
  for (const page of pages) {
    const html = await readFile(new URL(page, dir), 'utf8');
    const critical = html.indexOf('<style id="navigation-critical">');
    const script = html.indexOf('<script src="navigation.js"></script>');
    const stylesheet = html.indexOf('<link rel="stylesheet"');
    assert.ok(critical > html.indexOf('Content-Security-Policy') && critical < script && script < stylesheet, page);
    assert.match(html.slice(critical, script), /html \{ background: #120c18; color-scheme: dark;/, page);
    assert.match(html.slice(critical, script), /\.navigation-covered::after/, page);
    assert.equal((html.match(/src="navigation\.js"/g) || []).length, 1, page);
  }
});
