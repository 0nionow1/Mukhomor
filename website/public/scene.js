(() => {
  'use strict';
  const scene = document.querySelector('.intro-scene');
  if (!scene) return;
  const reduced = matchMedia('(prefers-reduced-motion: reduce)');
  const paused = () => reduced.matches || document.body.classList.contains('motion-paused');
  // Publish actual canvas growth, not a wall-clock approximation of its duration.
  const rootsReady = ready => {
    if (scene.dataset.rootsReady === String(ready)) return;
    scene.dataset.rootsReady = String(ready);
    scene.dispatchEvent(new CustomEvent('mukhomor-roots-ready', { detail: { ready } }));
  };
  rootsReady(false);
  const source = window.MukhomorScene;
  const fieldCanvas = document.querySelector('#living-field');
  const vignetteCanvas = document.querySelector('#scene-vignette');
  const vignetteContext = vignetteCanvas?.getContext('2d');
  if (vignetteContext) {
    // Match object-fit: cover exactly. A source-image fade alone would be cropped
    // away on wide/tall screens, exposing the animation at the page boundary.
    const smooth = (start, end, value) => {
      const t = Math.max(0, Math.min(1, (value - start) / (end - start)));
      return t * t * (3 - 2 * t);
    };
    const mask = vignetteContext.createImageData(vignetteCanvas.width, vignetteCanvas.height);
    const drawVignette = () => {
      const width = scene.clientWidth, height = scene.clientHeight;
      if (!width || !height) return;
      const columns = vignetteCanvas.width, rows = vignetteCanvas.height;
      const scale = Math.max(width / columns, height / rows);
      const cropX = (columns * scale - width) / 2, cropY = (rows * scale - height) / 2;
      for (let y = 0; y < rows; y++) for (let x = 0; x < columns; x++) {
        const nx = ((x + .5) * scale - cropX) / width;
        const ny = ((y + .5) * scale - cropY) / height;
        const edge = Math.min(nx, 1 - nx);
        const shoulders = 1 - smooth(0, .46, edge);
        const top = .94 * (1 - smooth(0, .30 + shoulders * .06, ny));
        const bottom = smooth(.62 - shoulders * .10, .96, ny);
        const sides = .28 * (1 - smooth(0, .22, edge));
        const alpha = 1 - (1 - top) * (1 - bottom) * (1 - sides);
        const offset = (y * columns + x) * 4;
        mask.data[offset] = 18; mask.data[offset + 1] = 12; mask.data[offset + 2] = 24;
        // Discrete tones keep the curved vignette pixel-stepped, without grain or motion.
        mask.data[offset + 3] = Math.round(alpha * 24) / 24 * 255;
      }
      vignetteContext.putImageData(mask, 0, 0);
      vignetteCanvas.hidden = false;
      vignetteCanvas.parentElement.classList.add('vignette-ready');
    };
    drawVignette();
    if ('ResizeObserver' in window) new ResizeObserver(drawVignette).observe(scene);
    else window.addEventListener('resize', drawVignette, { passive: true });
  }
  const mushroomCanvas = document.querySelector('#living-mushroom');
  const fieldContext = fieldCanvas.getContext('2d', { alpha: false });
  const mushroomContext = mushroomCanvas.getContext('2d');
  let visible = true, frameId = 0, time = 0, last = 0;
  const schedule = () => { if (!frameId && visible && !document.hidden && !paused()) frameId = requestAnimationFrame(tick); };
  let draw = () => {};
  if (source && fieldContext && mushroomContext) {
    const field = Uint8Array.from(atob(source.field), c => c.charCodeAt(0));
    const size = source.width * source.height;
    const pixels = fieldContext.createImageData(source.width, source.height);
    const mushroom = mushroomContext.createImageData(80, 72);
    const pieces = [];
    const distance = new Float32Array(80 * 72).fill(Infinity);
    const pending = [];
    // Travel along connected root pixels, beginning at the stem.
    for (let x = 0; x < 80; x++) if (source.rows[48][x] !== '.') {
      distance[48 * 80 + x] = Math.abs(x - 39) * .18;
      pending.push([x, 48]);
    }
    for (let n = 0; n < pending.length; n++) {
      const [x, y] = pending[n];
      for (let dy = -1; dy <= 1; dy++) for (let dx = -1; dx <= 1; dx++) {
        const nx = x + dx, ny = y + dy;
        if ((!dx && !dy) || nx < 0 || nx >= 80 || ny < 48 || ny >= 72 || source.rows[ny][nx] === '.') continue;
        const next = distance[y * 80 + x] + (dx && dy ? 1.41 : 1);
        if (next < distance[ny * 80 + nx]) { distance[ny * 80 + nx] = next; pending.push([nx, ny]); }
      }
    }
    source.rows.forEach((row, y) => [...row].forEach((glyph, x) => {
      const pathDistance = Number.isFinite(distance[y * 80 + x]) ? distance[y * 80 + x] : Math.hypot(x - 39, y - 47) + 10;
      if (glyph !== '.') pieces.push({ x, y, glyph, rgb: source.toneColors[source.tones[y][x] - 14], offset: (y * 80 + x) * 4, distance: pathDistance });
    }));
    const longest = Math.max(...pieces.filter(p => p.y >= 48 && Number.isFinite(p.distance)).map(p => p.distance));
    let rootGrowth = 0;
    draw = millis => {
      const position = (millis % 32000) / 32000 * source.frames;
      const a = Math.floor(position), b = (a + 1) % source.frames, blend = position - a;
      for (let i = 0; i < size; i++) {
        const one = source.palette[field[a * size + i]], two = source.palette[field[b * size + i]], p = i * 4;
        pixels.data[p] = one[0] + (two[0] - one[0]) * blend;
        pixels.data[p + 1] = one[1] + (two[1] - one[1]) * blend;
        pixels.data[p + 2] = one[2] + (two[2] - one[2]) * blend;
        pixels.data[p + 3] = 255;
      }
      fieldContext.putImageData(pixels, 0, 0);
      mushroom.data.fill(0);
      // Once revealed, roots stay grown when motion is paused and resumed.
      rootGrowth = Math.max(rootGrowth, paused() ? 1 : Math.min(1, Math.max(0, (millis - 250) / 2000)));
      for (const p of pieces) {
        if (p.y >= 48 && p.distance > rootGrowth * (longest + 3)) continue;
        let glow = 0;
        if ('WVQ'.includes(p.glyph)) glow = (.5 + .5 * Math.sin(millis * .0023 + p.x * .075)) * .34;
        if (p.y >= 48) glow = Math.max(0, Math.sin(millis * .0018 - p.distance * .18)) * .28;
        for (let c = 0; c < 3; c++) mushroom.data[p.offset + c] = p.rgb[c] + (255 - p.rgb[c]) * glow;
        mushroom.data[p.offset + 3] = 255;
      }
      mushroomContext.putImageData(mushroom, 0, 0);
      rootsReady(rootGrowth === 1);
    };
    document.querySelector('.scene-art-fallback').hidden = true;
    mushroomCanvas.hidden = false;
    document.body.classList.add('scene-ready');
    draw(paused() ? 2400 : 0);
  } else {
    const fallback = document.querySelector('.scene-art-fallback');
    if (fallback.complete && fallback.naturalWidth) rootsReady(true);
    else fallback.addEventListener('load', () => rootsReady(true), { once: true });
  }
  function tick(now) {
    frameId = 0;
    if (!visible || document.hidden || paused()) { last = 0; return; }
    if (!last) last = now;
    if (now - last >= 50) { time += Math.min(100, now - last); last = now; draw(time); }
    schedule();
  }
  schedule();
  if ('IntersectionObserver' in window) {
    new IntersectionObserver(entries => { visible = entries[0].isIntersecting; if (visible) { last = 0; schedule(); } }, { threshold: 0 }).observe(scene);
    const reveal = new IntersectionObserver(entries => {
      for (const entry of entries) if (entry.isIntersecting) { entry.target.classList.add('is-visible'); reveal.unobserve(entry.target); }
    }, { threshold: .12 });
    document.querySelectorAll('.story-intro, .principles, .section-heading, .scenario-copy, .routing-demo, .feature-notes, .ecosystem-intro, .connection-option, .start-section').forEach((element, i) => {
      element.classList.add('reveal-item'); element.style.setProperty('--reveal-delay', `${(i % 3) * 70}ms`); reveal.observe(element);
    });
    document.body.classList.add('reveal-ready');
  }
  document.addEventListener('visibilitychange', () => { last = 0; schedule(); });
  document.addEventListener('mukhomor-motion', () => { if (paused()) draw(Math.max(time, 2400)); last = 0; schedule(); });
  reduced.addEventListener('change', () => { if (paused()) draw(Math.max(time, 2400)); last = 0; schedule(); });
  // Small integer shifts preserve the pixel grid while giving the scene depth.
  scene.addEventListener('pointermove', event => {
    if (paused() || event.pointerType !== 'mouse') return;
    scene.style.setProperty('--pointer-x', `${Math.round((event.clientX / innerWidth - .5) * 12)}px`);
    scene.style.setProperty('--pointer-y', `${Math.round((event.clientY / scene.clientHeight - .5) * 8)}px`);
  }, { passive: true });
  scene.addEventListener('pointerleave', () => { scene.style.setProperty('--pointer-x', '0px'); scene.style.setProperty('--pointer-y', '0px'); });

})();
