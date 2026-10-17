(() => {
  'use strict';
  const main = document.querySelector('main');
  const gutters = [...document.querySelectorAll('.gutter-art')];
  const scene = document.querySelector('.intro-scene');
  if (!main || (!scene && !gutters.length)) return;
  const svg = tag => document.createElementNS('http://www.w3.org/2000/svg', tag);
  let bridge = main.querySelector('.root-bridge');
  if (!bridge) {
    bridge = svg('svg'); bridge.setAttribute('class', 'root-bridge');
    bridge.setAttribute('aria-hidden', 'true'); bridge.setAttribute('focusable', 'false');
    const group = svg('g'); group.setAttribute('class', 'bridge-strands'); bridge.append(group);
    main.append(bridge);
  }
  main.classList.add('mycelium-host');
  const art = scene?.querySelector('.scene-art');
  const strands = bridge.querySelector('.bridge-strands');
  const mushrooms = svg('g'); mushrooms.setAttribute('class', 'bridge-mushrooms'); bridge.append(mushrooms);
  const reduced = matchMedia('(prefers-reduced-motion: reduce)');
  const wide = matchMedia('(min-width: 1101px)');
  const forced = matchMedia('(forced-colors: active)');
  const paused = () => reduced.matches || document.body.classList.contains('motion-paused');
  const grid = n => Math.round(n / 4) * 4;
  const clamp = (n, lo, hi) => Math.max(lo, Math.min(hi, n));
  const angle = n => Math.atan2(Math.sin(n), Math.cos(n));
  const key = ([x, y]) => x + ',' + y;
  const distance = (a, b) => Math.hypot(a[0] - b[0], a[1] - b[1]);
  const random = Math.random;
  const pageSeed = Math.floor(random() * 0xffffffff);
  const sourceRank = (side, tile, index) => {
    let n = pageSeed ^ Math.imul(side + 1, 73856093) ^ Math.imul(tile + 1, 19349663) ^ Math.imul(index + 1, 83492791);
    n = Math.imul(n ^ n >>> 16, 0x45d9f3b); n = Math.imul(n ^ n >>> 16, 0x45d9f3b);
    return ((n ^ n >>> 16) >>> 0) / 0x100000000;
  };
  let frame = 0, last = 0, visible = true, layout = '', originY = 0, width = 0, height = 0, serial = 0;
  let sources = [], emitters = [], roots = [], obstacles = new Map(), occupied = new Map();
  const ready = () => !scene || (scene.dataset.rootsReady === 'true' && !scene.classList.contains('is-leaving') &&
    !art.getAnimations().some(a => a.animationName === 'mushroom-arrive' && a.playState !== 'finished'));
  const nearView = (bounds, margin = 200) => bounds.bottom >= scrollY - originY - margin && bounds.top <= scrollY - originY + innerHeight + margin;

  const trace = points => {
    const cells = [points[0].map(grid)];
    for (const p of points.slice(1)) {
      const a = cells.at(-1), b = p.map(grid), count = Math.max(Math.abs(b[0] - a[0]), Math.abs(b[1] - a[1])) / 4;
      for (let i = 1; i <= count; i++) {
        const x = grid(a[0] + (b[0] - a[0]) * i / count), y = grid(a[1] + (b[1] - a[1]) * i / count);
        const prev = cells.at(-1);
        if (x !== prev[0]) cells.push([x, prev[1]]);
        if (y !== prev[1]) cells.push([x, y]);
      }
    }
    return cells;
  };
  const pathData = points => points.map(([x, y], i) => (i ? 'L' : 'M') + x + ' ' + y).join('');
  const addObstacle = rect => {
    for (let x = Math.floor(rect.left / 128); x <= Math.floor(rect.right / 128); x++) {
      for (let y = Math.floor(rect.top / 128); y <= Math.floor(rect.bottom / 128); y++) {
        const id = x + ',' + y;
        if (!obstacles.has(id)) obstacles.set(id, []);
        obstacles.get(id).push(rect);
      }
    }
  };
  const blocked = ([x, y]) => x < 16 || x > width - 16 || y < 16 || y > height - 16 ||
    (obstacles.get(Math.floor(x / 128) + ',' + Math.floor(y / 128)) || []).some(r => x >= r.left && x <= r.right && y >= r.top && y <= r.bottom);
  const reserve = (root, points, offset) => {
    points.forEach((p, i) => {
      if (root.bounds) { root.bounds.top = Math.min(root.bounds.top, p[1]); root.bounds.bottom = Math.max(root.bounds.bottom, p[1]); }
      const id = key(p);
      if (!occupied.has(id)) occupied.set(id, new Map());
      occupied.get(id).set(root.id, offset + i * 4);
      root.reserved.add(id);
    });
  };
  const free = (p, root, anchor = root.source) => {
    if (blocked(p)) return false;
    for (let dx = -8; dx <= 8; dx += 4) for (let dy = -8; dy <= 8; dy += 4) {
      const entries = occupied.get(key([p[0] + dx, p[1] + dy]));
      if (!entries) continue;
      for (const [owner, at] of entries) {
        if (owner === root.id && at <= root.length && root.length - at < 44) continue;
        if (owner === -1 && distance(p, root.source) <= 20) continue;
        if (distance(p, anchor) <= 20) continue;
        return false;
      }
    }
    return true;
  };

  // Turn in short terraces: between a smooth arc and a sharp elbow. Every cell is
  // checked against content and previous growth before reserving the new segment.
  const arc = (root, turn, span = 44, heading = root.heading, anchor = root.source, cache) => {
    const points = [root.points.at(-1)];
    let [x, y] = points[0];
    const steps = Math.ceil(span / 4);
    for (let i = 1; i <= steps; i++) {
      const terraces = Math.ceil(steps / 3);
      const theta = heading + turn * (Math.floor((i - 1) / 3) + .5) / terraces;
      x += Math.cos(theta) * span / steps; y += Math.sin(theta) * span / steps;
      points.push([x, y]);
    }
    const cells = trace(points);
    return cells.every(p => {
      if (!cache) return free(p, root, anchor);
      const id = key(p);
      if (!cache.has(id)) cache.set(id, free(p, root, anchor));
      return cache.get(id);
    }) ? { cells, heading: heading + turn } : null;
  };
  const chooseGoal = root => {
    const top = clamp(scrollY - originY, 16, height - 32);
    const bottom = Math.min(height - 16, top + innerHeight);
    for (let i = 0; i < 40; i++) {
      const point = [32 + random() * (width - 64), top + random() * (bottom - top)];
      if (!blocked(point) && distance(point, root.points.at(-1)) > 100) return point;
    }
    return [root.points.at(-1)[0], Math.min(height - 24, root.points.at(-1)[1] + 240)];
  };
  const sideBranch = root => {
    const motifs = [
      [[0,0],[2,2],[4,3],[6,3],[8,5],[7,7],[9,9],[12,9]],
      [[0,0],[1,3],[3,4],[3,6],[5,7],[8,6],[9,8],[9,10]],
      [[0,0],[-1,3],[1,5],[1,7],[4,8],[5,7],[7,8],[7,10]],
      [[0,0],[2,1],[4,1],[5,3],[5,6],[7,7],[6,9],[8,11]]
    ];
    const grow = (parent, index, start, side, scale, kind) => {
      const anchor = parent[index], before = parent[Math.max(0, index - 6)], after = parent[Math.min(parent.length - 1, index + 6)];
      const size = distance(before, after) || 1;
      const dx = (after[0] - before[0]) / size, dy = (after[1] - before[1]) / size;
      const motif = motifs[Math.floor(random() * motifs.length)];
      const points = motif.map(([u, v]) => [anchor[0] + (dx * u - dy * v * side) * 4 * scale, anchor[1] + (dy * u + dx * v * side) * 4 * scale]);
      // Chamfer just the corners; retain the irregular hooks and return bends.
      const corners = [points[0]];
      for (let i = 1; i < points.length; i++) {
        const a = points[i - 1], b = points[i];
        corners.push([a[0] * .8 + b[0] * .2, a[1] * .8 + b[1] * .2], [a[0] * .2 + b[0] * .8, a[1] * .2 + b[1] * .8]);
      }
      corners.push(points.at(-1));
      const cells = trace(corners), end = cells.findIndex(p => !free(p, root, anchor));
      const usable = end < 0 ? cells : cells.slice(0, end);
      if (usable.length < 6) return;
      const node = svg('path');
      node.setAttribute('class', 'bridge-strand ' + kind); node.setAttribute('pathLength', '1');
      node.setAttribute('d', pathData(usable)); root.group.append(node);
      const branch = { node, kind, cells: usable, start, length: (usable.length - 1) * 4 };
      root.branches.push(branch); reserve(root, usable, start); return branch;
    };
    const side = random() > .5 ? 1 : -1;
    const b = grow(root.points, root.points.length - 1, root.length, side, .9 + random() * .5, 'bridge-branch') ||
      grow(root.points, root.points.length - 1, root.length, -side, .9, 'bridge-branch');
    if (b) {
      const at = Math.floor(b.cells.length * .45);
      grow(b.cells, at, b.start + at * 4, -side, .5 + random() * .25, 'bridge-tip');
      if (random() > .4) {
        const tip = Math.floor(b.cells.length * .74);
        grow(b.cells, tip, b.start + tip * 4, side, .45, 'bridge-tip');
      }
    }
  };
  const extend = root => {
    if (!root.goal || root.steps % 6 === 0 || distance(root.points.at(-1), root.goal) < 72) root.goal = chooseGoal(root);
    const p = root.points.at(-1);
    const toward = angle(Math.atan2(root.goal[1] - p[1], root.goal[0] - p[0]) - root.heading);
    const desired = clamp(toward, -.8, .8) * .55 + (random() - .5) * .55;
    const candidates = [desired, desired * .4, 0, -.65, .65, -1, 1, -1.35, 1.35];
    // Candidates share the same collision state until the chosen arc is reserved.
    const cache = new Map();
    let result, best = -Infinity;
    for (const turn of candidates) {
      const candidate = arc(root, turn, 44, root.heading, root.source, cache);
      if (!candidate) continue;
      // Look ahead before committing, so the tip turns before reaching a wall.
      const probe = { ...root, points: [candidate.cells.at(-1)], heading: candidate.heading };
      const exits = [-.7, 0, .7].filter(next => arc(probe, next, 36, probe.heading, root.source, cache)).length;
      const score = exits * .8 - Math.abs(turn - desired);
      if (score > best) { best = score; result = candidate; }
    }
    if (!result) return false;
    reserve(root, result.cells.slice(1), root.length + 4);
    if (root.steps > 0) sideBranch(root);
    root.points.push(...result.cells.slice(1));
    root.length = (root.points.length - 1) * 4;
    root.heading = result.heading; root.steps++;
    root.node.setAttribute('d', pathData(root.points));
    return true;
  };
  const spawn = (emitter = emitters[0]) => {
    const active = roots.filter(r => !r.retiring);
    const candidates = sources.filter(s => s.emitter === emitter.id).map(source => ({ source, score: random() * 80 + Math.min(400, ...active.map(r => distance(r.source, source.point))) }));
    candidates.sort((a, b) => b.score - a.score);
    for (const { source } of candidates) {
      if (active.some(r => distance(r.source, source.point) < (emitter.side ? 12 : 28))) continue;
      const group = svg('g'), node = svg('path');
      group.setAttribute('class', 'bridge-growth');
      group.setAttribute('data-root-emitter', emitter.id);
      if (emitter.side) {
        const gradient = svg('radialGradient'), gradientId = 'root-side-' + (serial + 1);
        gradient.setAttribute('id', gradientId); gradient.setAttribute('gradientUnits', 'userSpaceOnUse');
        gradient.setAttribute('cx', source.point[0]); gradient.setAttribute('cy', source.point[1]); gradient.setAttribute('r', '240');
        for (const [offset, color] of [[0, '#16a491'], [.45, '#587c78'], [1, '#694456']]) {
          const stop = svg('stop'); stop.setAttribute('offset', offset); stop.setAttribute('stop-color', color); gradient.append(stop);
        }
        group.append(gradient); group.style.setProperty('--root-stroke', `url(#${gradientId})`);
      }
      node.setAttribute('class', 'bridge-strand'); node.setAttribute('pathLength', '1');
      group.append(node);
      const root = { id: ++serial, emitter, group, node, source: source.point, bounds: { top: source.point[1], bottom: source.point[1] }, points: [source.point], heading: source.heading, length: 0, grown: 0, age: 0, steps: 0, retiring: 0, branches: [], settled: new Map(), reserved: new Set() };
      let initial;
      for (const offset of [0, -.45, .45, -.9, .9, -1.3, 1.3]) {
        root.heading = source.heading + offset;
        initial = arc(root, (random() - .5) * .25, 28);
        if (initial) break;
      }
      if (!initial) continue;
      reserve(root, initial.cells, 0);
      root.points = initial.cells; root.length = (initial.cells.length - 1) * 4; root.heading = initial.heading;
      if (![0, -.7, .7, -1.35, 1.35].some(turn => arc(root, turn))) { remove(root); continue; }
      node.setAttribute('d', pathData(root.points));
      const joint = svg('path'), jointCells = initial.cells.slice(0, 9);
      joint.setAttribute('class', 'bridge-strand bridge-joint'); joint.setAttribute('pathLength', '1');
      joint.setAttribute('d', pathData(jointCells)); group.append(joint);
      root.branches.push({ node: joint, kind: 'bridge-joint', cells: jointCells, start: 0, length: (jointCells.length - 1) * 4 });
      strands.append(group); roots.push(root); return root;
    }
    return null;
  };
  const remove = root => {
    root.group.remove();
    for (const id of root.reserved) {
      const owners = occupied.get(id); owners?.delete(root.id);
      if (!owners?.size) occupied.delete(id);
    }
    roots = roots.filter(r => r !== root);
  };
  const paint = root => {
    root.node.style.visibility = root.grown > 0 ? 'visible' : 'hidden';
    root.node.style.strokeDashoffset = String(1 - clamp(root.grown / Math.max(1, root.length), 0, 1));
    const growing = [], changed = new Set();
    for (const branch of root.branches) {
      const progress = clamp((root.grown - branch.start) / branch.length, 0, 1);
      if (progress === 1) {
        // Keep every completed fork, but animate only the growing tips. The
        // root's 100-step lifetime bounds geometry without a decoration cutoff.
        let batch = root.settled.get(branch.kind);
        if (!batch) {
          const node = svg('path');
          node.setAttribute('class', 'bridge-strand ' + branch.kind);
          node.style.visibility = 'visible'; node.style.strokeDasharray = 'none';
          node.style.strokeDashoffset = '0';
          root.group.append(node);
          batch = { node, data: '' }; root.settled.set(branch.kind, batch);
        }
        batch.data += branch.node.getAttribute('d'); changed.add(batch);
        branch.node.remove();
        continue;
      }
      branch.node.style.visibility = progress > 0 ? 'visible' : 'hidden';
      branch.node.style.strokeDashoffset = String(1 - progress);
      growing.push(branch);
    }
    for (const batch of changed) batch.node.setAttribute('d', batch.data);
    root.branches = growing;
  };
  const stop = () => { cancelAnimationFrame(frame); frame = 0; last = 0; };
  const schedule = () => { if (!frame && wide.matches && !forced.matches && visible && !document.hidden) frame = requestAnimationFrame(draw); };
  const populate = now => {
    // A single budget and animation loop for the hero and every side mushroom.
    // Keep nearby drawings when scrolling; release distant geometry and its cells.
    for (const root of [...roots]) if (!nearView(root.bounds, 600)) remove(root);
    for (const emitter of emitters) {
      if (!nearView(emitter.bounds, 120) || now < emitter.retryAt) continue;
      while (roots.filter(r => !r.retiring && r.emitter === emitter).length < emitter.count &&
        roots.filter(r => !r.retiring).length < 7 && roots.length < 14) {
        const root = spawn(emitter);
        if (!root) { emitter.retryAt = now + 1200; break; }
        if (paused()) {
          for (let step = 0; step < 18 && extend(root); step++) { /* A completed static sample. */ }
          root.grown = root.length; paint(root);
        }
      }
    }
  };
  const draw = now => {
    frame = 0;
    if (!wide.matches || forced.matches || !visible || document.hidden || !sources.length) { last = 0; return; }
    if (paused()) { populate(now); roots.forEach(paint); bridge.classList.toggle('bridge-visible', !!emitters.length); last = 0; return; }
    if (!last) last = now;
    if (now - last < 50) { schedule(); return; }
    const elapsed = Math.min(100, now - last); last = now;
    for (const root of [...roots]) {
      if (!nearView(root.bounds)) continue;
      if (root.retiring) {
        const duration = root.emitter.side ? 3600 : 2400;
        root.retiring += elapsed; root.group.style.opacity = String((root.emitter.side ? .42 : 1) * Math.max(0, 1 - root.retiring / duration));
        if (root.retiring >= duration) remove(root);
        continue;
      }
      root.age += elapsed;
      const belowView = root.points.at(-1)[1] < scrollY - originY - 64;
      root.grown = Math.min(root.length, root.grown + elapsed * (root.emitter.side ? .035 : belowView ? .6 : .1));
      if (root.grown >= root.length) {
        if (root.steps >= 100 || root.age > (root.emitter.side ? 180000 : 90000) || !extend(root)) root.retiring = 1;
      }
      paint(root);
    }
    populate(now);
    bridge.classList.toggle('bridge-visible', mushrooms.children.length > 0 || roots.some(r => r.grown > 0));
    if (roots.some(r => nearView(r.bounds)) || emitters.some(e => nearView(e.bounds, 120))) schedule();
  };

  const measure = () => {
    if (!wide.matches || forced.matches) { stop(); bridge.classList.remove('bridge-ready', 'bridge-visible'); return; }
    const heroReady = !!scene && ready();
    if (heroReady) document.body.classList.add('hero-arrived');
    const mainRect = main.getBoundingClientRect(), origin = { left: 0, top: mainRect.top };
    const artwork = art && (art.querySelector('canvas:not([hidden]), img:not([hidden])') || art).getBoundingClientRect();
    const rectOf = element => {
      let r = element.getBoundingClientRect();
      if (element.matches('h1, h2, h3, h4, p, li, dt, dd, figcaption, .scene-bottom-bar > span')) {
        const range = document.createRange(); range.selectNodeContents(element);
        r = range.getBoundingClientRect();
      }
      const reveal = element.closest('.reveal-item');
      const shift = reveal ? new DOMMatrix(getComputedStyle(reveal).transform).m42 : 0;
      return { left: r.left - origin.left - 16, right: r.right - origin.left + 16, top: r.top - origin.top - shift - 16, bottom: r.bottom - origin.top - shift + 16 };
    };
    // Add data-root-avoid to any future component that needs a protected area.
    const selector = '[data-root-avoid], [data-cms], h1, h2, h3, h4, p, li, dt, dd, figcaption, button, a, fieldset, pre, table, .screen-stage, .routing-demo, .connection-option, .scene-bottom-bar > span, .principles, .feature-notes, .start-mark';
    const protectedRects = [...main.querySelectorAll(selector)].filter(e => !e.closest('.root-bridge, .scene-art, .docs-sidebar') && e.getClientRects().length && !e.matches('.motion-toggle')).map(rectOf);
    protectedRects.push(rectOf(document.querySelector('.site-header')));
    if (scene) {
      const bar = document.querySelector('.scene-bottom-bar').getBoundingClientRect();
      protectedRects.push({ left: bar.right - 196, right: bar.right + 16, top: scene.offsetHeight - 72, bottom: scene.offsetHeight - 8 });
      protectedRects.push({ left: artwork.left - 8, right: artwork.right + 8, top: artwork.top - origin.top - 8, bottom: artwork.top - origin.top + artwork.height * 48 / 72 - 4 });
    }
    width = document.documentElement.clientWidth; height = main.clientHeight; originY = origin.top + scrollY;
    const sidebar = main.querySelector('.docs-sidebar');
    if (sidebar) { const r = rectOf(sidebar); protectedRects.push({ ...r, top: 0, bottom: height }); }
    const candidates = []; let tileMushrooms = 0;
    for (const [side, gutter] of gutters.entries()) {
      const style = getComputedStyle(gutter), r = gutter.getBoundingClientRect();
      if (style.display === 'none' || r.width < 32) continue;
      const period = Number(style.getPropertyValue('--gutter-tile-height'));
      const shapes = style.getPropertyValue('--gutter-sources').trim().split(',').map(s => s.trim().split(/\s+/).map(Number));
      if (!period || shapes.some(s => s.length !== 4)) continue;
      for (let tile = 0; tile * period < r.height; tile++) for (const [index, [x, y, scale, flip]] of shapes.entries()) {
        if (tile * period + y + 7 * scale > r.height) continue;
        tileMushrooms++;
        const cx = r.left + (r.width - 280) / 2 + x, cy = r.top - origin.top + tile * period + y;
        const left = Math.max(r.left + 8, cx - 4 * scale), right = Math.min(r.right - 8, cx + 4 * scale);
        if (right - left < 12) continue;
        const rank = sourceRank(side, tile, index);
        candidates.push({ id: `side-${side}-${tile}-${index}`, cx, cy, scale, flip, left, right, rank,
          clip: { x: r.left, y: r.top - origin.top, width: r.width, height: r.height }, count: sourceRank(side + 17, tile, index) < .5 ? 1 : 2 });
      }
    }
    // About one in twelve, chosen anew per visit. Keep this choice stable while
    // scrolling or resizing, with breathing room between the few awake mushrooms.
    const sideMushrooms = [], count = Math.max(1, Math.floor(tileMushrooms / 12));
    for (const candidate of candidates.sort((a, b) => a.rank - b.rank)) {
      if (sideMushrooms.length >= count) break;
      if (sideMushrooms.some(m => Math.abs(m.cy - candidate.cy) < 480)) continue;
      sideMushrooms.push(candidate);
      const { cx, cy, scale, clip } = candidate;
      protectedRects.push({ left: Math.max(clip.x, cx - 9 * scale) - 4, right: Math.min(clip.x + clip.width, cx + 9 * scale) + 4, top: cy - 8 * scale - 4, bottom: cy + 7 * scale - 6 });
    }
    const signature = JSON.stringify([width, height, heroReady, artwork?.left, artwork && artwork.top - origin.top, artwork?.width, sideMushrooms, ...protectedRects.map(r => Object.values(r).map(grid))]);
    if (signature !== layout) {
      layout = signature; strands.replaceChildren(); mushrooms.replaceChildren(); sources = []; emitters = []; roots = []; occupied = new Map(); obstacles = new Map();
      protectedRects.forEach(addObstacle);
      const rows = window.MukhomorScene?.rows;
      if (heroReady && artwork && rows) {
        const neighbors = (x, y) => {
          const cells = [];
          for (let dy = -1; dy <= 1; dy++) for (let dx = -1; dx <= 1; dx++) if ((dx || dy) && rows[y + dy]?.[x + dx] && rows[y + dy][x + dx] !== '.' && y + dy >= 48) cells.push([x + dx, y + dy]);
          return cells;
        };
        const distances = new Float32Array(80 * 72).fill(Infinity), queue = [];
        for (let x = 0; x < 80; x++) if (rows[48][x] !== '.') { distances[48 * 80 + x] = 0; queue.push([x, 48]); }
        for (let i = 0; i < queue.length; i++) {
          const [x, y] = queue[i];
          for (const [nx, ny] of neighbors(x, y)) {
            const value = distances[y * 80 + x] + Math.hypot(nx - x, ny - y);
            if (value < distances[ny * 80 + nx] - .001) { distances[ny * 80 + nx] = value; queue.push([nx, ny]); }
          }
        }
        const toPage = (x, y) => [grid(artwork.left - origin.left + artwork.width * (x + .5) / 80), grid(artwork.top - origin.top + artwork.height * (y + .5) / 72)];
        const native = { id: -1, reserved: new Set() };
        for (let y = 48; y < 72; y++) for (let x = 0; x < 80; x++) if (rows[y][x] !== '.') {
          reserve(native, [toPage(x, y)], 0);
          const near = neighbors(x, y), value = distances[y * 80 + x];
          if (y < 50 || !near.length || near.length > 3 || value < 8 || !Number.isFinite(value) || near.some(([nx, ny]) => distances[ny * 80 + nx] > value + .001)) continue;
          const dx = x - near.reduce((sum, p) => sum + p[0], 0) / near.length;
          const dy = y - near.reduce((sum, p) => sum + p[1], 0) / near.length;
          if (Math.hypot(dx, dy) > .1) sources.push({ point: toPage(x, y), heading: Math.atan2(dy, dx), emitter: 'hero' });
        }
        emitters.push({ id: 'hero', count: 3, bounds: { top: toPage(0, 48)[1], bottom: toPage(0, 72)[1] }, retryAt: 0 });
        const gradient = bridge.querySelector('linearGradient');
        gradient.setAttribute('x1', '0'); gradient.setAttribute('x2', '0');
        gradient.setAttribute('y1', String(artwork.top - origin.top + artwork.height * .8));
        gradient.setAttribute('y2', String(scene.offsetHeight + 300));
        }
      for (const { id, cx, cy, scale, flip, count, left, right, clip } of sideMushrooms) {
        const clipping = svg('clipPath'), box = svg('rect'), group = svg('g');
        clipping.setAttribute('id', 'clip-' + id);
        for (const [name, value] of Object.entries(clip)) box.setAttribute(name, value);
        clipping.append(box); mushrooms.append(clipping); group.setAttribute('clip-path', `url(#clip-${id})`);
        // Same 20×16 pixels and origin as the static gutter. The stem ends at
        // cy + 7*scale; no pre-drawn roots compete with the animated branches.
        const image = svg('image'); image.setAttribute('href', 'assets/mushroom-background.svg');
        image.setAttribute('x', -10 * scale); image.setAttribute('y', -8 * scale);
        image.setAttribute('width', 20 * scale); image.setAttribute('height', 16 * scale);
        image.style.imageRendering = 'pixelated';
        image.setAttribute('transform', `translate(${cx} ${cy}) scale(${flip} 1)`);
        image.style.opacity = scale >= 7 ? '.24' : '.34';
        image.setAttribute('data-root-mushroom', id); group.append(image); mushrooms.append(group);
        emitters.push({ id, side: true, count, bounds: { top: cy - 7 * scale, bottom: cy + 7 * scale }, retryAt: 0 });
        const middle = (left + right) / 2, spread = Math.min(2 * scale, (right - left) / 2);
        for (const direction of [-1, 1]) sources.push({ emitter: id, point: [grid(middle + direction * spread), grid(cy + 7 * scale)], heading: Math.PI / 2 - direction * .65 });
      }
      bridge.setAttribute('viewBox', `0 0 ${width} ${height}`);
      bridge.style.height = height + 'px'; bridge.style.width = width + 'px'; bridge.style.left = -mainRect.left + 'px';
      populate(performance.now());
    }
    bridge.classList.add('bridge-ready');
    bridge.classList.toggle('bridge-visible', mushrooms.children.length > 0 || roots.some(r => r.grown > 0));
    schedule();
  };
  if ('ResizeObserver' in window) {
    const observer = new ResizeObserver(measure);
    observer.observe(main); if (scene) observer.observe(scene);
  }
  if ('IntersectionObserver' in window) new IntersectionObserver(entries => {
    visible = entries[0].isIntersecting; stop(); if (visible) schedule();
  }).observe(bridge);
  window.addEventListener('resize', measure, { passive: true });
  window.addEventListener('scroll', schedule, { passive: true });
  scene?.addEventListener('mukhomor-roots-ready', measure);
  art?.addEventListener('animationend', measure);
  if (scene) new MutationObserver(measure).observe(scene, { attributes: true, attributeFilter: ['class'] });
  document.addEventListener('visibilitychange', () => { stop(); if (!document.hidden) schedule(); });
  const motionChanged = () => { stop(); measure(); };
  document.addEventListener('mukhomor-motion', motionChanged);
  reduced.addEventListener('change', motionChanged);
  wide.addEventListener('change', measure);
  forced.addEventListener('change', measure);
  document.fonts?.ready.then(measure);
  measure();
})();
