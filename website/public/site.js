(() => {
  'use strict';
  // The tile never moves or scales. Hide only a mushroom cut by the bottom edge.
  const gutters = [...document.querySelectorAll('.gutter-art')];
  const trimGutter = element => {
    const style = getComputedStyle(element);
    const period = Number(style.getPropertyValue('--gutter-tile-height'));
    if (!period) return;
    const height = element.getBoundingClientRect().height;
    const tail = height % period;
    const bands = style.getPropertyValue('--gutter-bands').trim().split(',')
      .map(band => band.trim().split(/\s+/).map(Number));
    const partial = bands.find(([start, end]) => tail > start && tail < end);
    element.style.clipPath = partial ? `inset(0 0 ${tail - partial[0]}px)` : 'none';
  };
  gutters.forEach(trimGutter);
  if ('ResizeObserver' in window) {
    const observer = new ResizeObserver(entries => entries.forEach(({ target }) => trimGutter(target)));
    gutters.forEach(element => observer.observe(element));
  } else if (gutters.length) {
    window.addEventListener('resize', () => gutters.forEach(trimGutter), { passive: true });
    new MutationObserver(() => gutters.forEach(trimGutter)).observe(document.querySelector('main'), {
      subtree: true, attributes: true, attributeFilter: ['hidden', 'open'], childList: true, characterData: true
    });
  }
  // Repeat one static noise tile across the document, on the field's pixel grid.
  const field = document.querySelector('#living-field');
  const texture = document.createElement('div');
  texture.className = 'pixel-texture';
  texture.setAttribute('aria-hidden', 'true');
  const sample = document.createElement('canvas');
  sample.width = field?.width || 144;
  sample.height = field?.height || 96;
  const textureContext = sample.getContext('2d');
  if (textureContext) {
    const grain = textureContext.createImageData(sample.width, sample.height);
    let seed = 0x4d554b48;
    for (let i = 0; i < grain.data.length; i += 4) {
      seed ^= seed << 13; seed ^= seed >>> 17; seed ^= seed << 5;
      const shade = (seed >>> 30) * 85;
      grain.data[i] = grain.data[i + 1] = grain.data[i + 2] = shade;
      grain.data[i + 3] = 255;
    }
    textureContext.putImageData(grain, 0, 0);
    texture.style.backgroundImage = `url("${sample.toDataURL()}")`;
    const sizeTexture = () => {
      const width = field ? field.clientWidth : document.documentElement.clientWidth;
      const height = field ? field.clientHeight : window.innerHeight;
      const scale = Math.max(width / sample.width, height / sample.height);
      const tileWidth = sample.width * scale, tileHeight = sample.height * scale;
      texture.style.backgroundSize = `${tileWidth}px ${tileHeight}px`;
      texture.style.backgroundPosition = `${(width - tileWidth) / 2}px ${(height - tileHeight) / 2}px`;
    };
    sizeTexture();
    document.body.append(texture);
    if ('ResizeObserver' in window) new ResizeObserver(sizeTexture).observe(field || document.documentElement);
    window.addEventListener('resize', sizeTexture, { passive: true });
  }
  const scenarios = {
    play: { title: 'Игры и голосовой чат', description: 'Готовые исключения оставляют игры на прямом соединении. Браузер и чат можно направить через VPN.', detail: '', direct: 'Игра', vpn: 'Браузер и чат' },
    create: { title: 'Код, видео и веб-сервисы', description: 'Настройте отдельные маршруты для редактора, IDE и рабочих сервисов. Сохраните правила в свой набор.', detail: '', direct: 'Редактор и IDE', vpn: 'Веб-сервисы' },
    daily: { title: 'Сайты и приложения', description: 'Выберите прямые исключения. Остальной трафик пойдёт через ваш VPN. Клиент может работать в трее.', detail: '', direct: 'Ваши исключения', vpn: 'Другие приложения' }
  };
  const tabs = [...document.querySelectorAll('[data-scenario]')];
  document.addEventListener('mukhomor-content',event=>{
    const texts=event.detail.texts;
    for(const [name,values] of Object.entries(scenarios))for(const key of Object.keys(values))if(typeof texts[`scenario.${name}.${key}`]==='string')values[key]=texts[`scenario.${name}.${key}`];
    const selected=tabs.find(tab=>tab.getAttribute('aria-selected')==='true');if(selected)selectScenario(selected);
  });
  const selectScenario = (tab, focus = false) => {
    const content = scenarios[tab.dataset.scenario];
    for (const item of tabs) { item.setAttribute('aria-selected', String(item === tab)); item.tabIndex = item === tab ? 0 : -1; }
    document.querySelector('#scenario-panel').setAttribute('aria-labelledby', tab.id);
    for (const [id, value] of Object.entries({ 'scenario-title': content.title, 'scenario-description': content.description, 'scenario-detail': content.detail, 'route-direct': content.direct, 'route-vpn': content.vpn })) document.getElementById(id).textContent = value;
    if (focus) tab.focus();
  };
  if (tabs.length) {
    document.querySelector('.scenario-tabs').hidden = false;
    tabs.forEach((tab, index) => {
      tab.addEventListener('click', () => selectScenario(tab));
      tab.addEventListener('keydown', event => {
        let next;
        if (event.key === 'ArrowRight') next = (index + 1) % tabs.length;
        if (event.key === 'ArrowLeft') next = (index - 1 + tabs.length) % tabs.length;
        if (event.key === 'Home') next = 0;
        if (event.key === 'End') next = tabs.length - 1;
        if (next !== undefined) { event.preventDefault(); selectScenario(tabs[next], true); }
      });
    });
  }
  const motionButtons = [...document.querySelectorAll('.motion-toggle')];
  if (motionButtons.length) {
    const preference = matchMedia('(prefers-reduced-motion: reduce)');
    let saved = null;
    try { saved = localStorage.getItem('mukhomor-site-motion'); } catch { /* Storage is optional. */ }
    let paused = preference.matches || saved === 'paused';
    const renderMotion = () => {
      document.body.classList.toggle('motion-paused', paused);
      for (const button of motionButtons) {
        button.setAttribute('aria-pressed', String(paused));
        button.textContent = paused ? 'Анимация: выкл.' : 'Анимация: вкл.';
        button.hidden = preference.matches;
      }
      document.dispatchEvent(new CustomEvent('mukhomor-motion', { detail: { paused } }));
    };
    renderMotion();
    motionButtons.forEach(button => button.addEventListener('click', () => {
      paused = !paused;
      saved = paused ? 'paused' : 'running';
      renderMotion();
      try { localStorage.setItem('mukhomor-site-motion', saved); } catch { /* Storage is optional. */ }
    }));
    preference.addEventListener('change', event => { paused = event.matches || saved === 'paused'; renderMotion(); });
  }
  const dialog = document.querySelector('.preview-dialog');
  const previewTrigger = document.querySelector('[data-open-preview]');
  if (dialog && previewTrigger && typeof dialog.showModal === 'function') {
    previewTrigger.hidden = false;
    previewTrigger.addEventListener('click', () => dialog.showModal());
    dialog.querySelector('[data-close-preview]').addEventListener('click', () => dialog.close());
    dialog.addEventListener('click', event => {
      const bounds = dialog.getBoundingClientRect();
      if (event.target === dialog && (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom)) dialog.close();
    });
  }
  const sections = [...document.querySelectorAll('.doc-section')];
  if (sections.length) {
    document.body.classList.add('docs-enhanced');
    const docLinks = [...document.querySelectorAll('[data-doc-link]')];
    const showSection = (navigate = false) => {
      const requested = location.hash.slice(1);
      // Keep the skip link functional; unknown fragments fall back to the start.
      if (requested === 'main') return;
      const selected = sections.find(section => section.id === requested) || sections[0];
      for (const section of sections) section.hidden = section !== selected;
      for (const link of docLinks) {
        if (link.hash === '#' + selected.id) link.setAttribute('aria-current', 'location');
        else link.removeAttribute('aria-current');
      }
      const heading = selected.querySelector('h2');
      document.title = heading.textContent + ' — Мухомор';
      if (navigate) {
        heading.focus({ preventScroll: true });
        if (matchMedia('(max-width: 600px)').matches || window.scrollY > selected.offsetTop) selected.scrollIntoView({ block: 'start', behavior: 'instant' });
        else window.scrollTo({ top: 0, behavior: 'instant' });
      }
    };
    showSection();
    window.addEventListener('hashchange', () => showSection(true));
    for (const button of document.querySelectorAll('[data-copy]')) {
      if (!navigator.clipboard || !window.isSecureContext) continue;
      button.hidden = false;
      button.addEventListener('click', async () => {
        const feedback = button.closest('.code-block').nextElementSibling;
        try {
          await navigator.clipboard.writeText(button.parentElement.querySelector('code').textContent);
          feedback.textContent = 'Команда скопирована. Замените имя архива на своё.';
        } catch { feedback.textContent = 'Не удалось скопировать. Выделите команду и скопируйте вручную.'; }
      });
    }
  }
})();
