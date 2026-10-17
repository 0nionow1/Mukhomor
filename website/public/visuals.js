(() => {
  'use strict';
  const gallery = document.querySelector('[data-app-gallery]');
  if (gallery) {
    const tabs = [...gallery.querySelectorAll('[data-screen-tab]')];
    const panels = [...gallery.querySelectorAll('[data-screen-panel]')];
    const link = gallery.querySelector('[data-gallery-image]');
    const image = link.querySelector('img');
    const caption = gallery.querySelector('[data-gallery-caption]');
    const picker = gallery.querySelector('.theme-picker');
    const radios = [...gallery.querySelectorAll('[name="gallery-theme"]')];
    const themeCaption = gallery.querySelector('[data-gallery-theme]');
    const status = gallery.querySelector('[data-gallery-status]');
    const figure = gallery.querySelector('[data-gallery-figure]');
    const themeNames = { neon: 'Неон', paper: 'Поп-арт', glacier: 'Киберпанк' };
    let selected = tabs[0], selectedTheme = 'neon';
    let wanted = selected, wantedTheme = selectedTheme, requestId = 0;
    const decoded = new Map();
    const load = src => {
      if (!decoded.has(src)) {
        const next = new Image();
        const loaded = new Promise((resolve, reject) => {
          next.onload = () => resolve(next);
          next.onerror = () => reject(new Error('Screenshot unavailable'));
        }).then(async next => { if (next.decode) await next.decode(); return next; });
        let timeout;
        const ready = Promise.race([loaded, new Promise((_, reject) => {
          timeout = setTimeout(() => reject(new Error('Screenshot timeout')), 10000);
        })]).finally(() => clearTimeout(timeout));
        next.src = src;
        decoded.set(src, ready);
        ready.catch(() => decoded.delete(src));
      }
      return decoded.get(src);
    };
    const commit = (tab, theme, src, next) => {
      for (const item of tabs) { item.setAttribute('aria-selected', String(item === tab)); item.tabIndex = item === tab ? 0 : -1; }
      for (const panel of panels) panel.hidden = panel.id !== tab.getAttribute('aria-controls');
      for (const radio of radios) radio.checked = radio.value === theme;
      image.src = src;
      image.width = next.naturalWidth;
      image.height = next.naturalHeight;
      image.alt = tab.dataset.alt.replace('тема Неон', `тема ${themeNames[theme]}`);
      link.href = src;
      link.dataset.title = `${tab.dataset.title} · ${themeNames[theme]}`;
      link.setAttribute('aria-label', `Увеличить: ${link.dataset.title}`);
      caption.textContent = tab.dataset.title;
      if (themeCaption) themeCaption.textContent = `тема «${themeNames[theme]}»`;
      gallery.dataset.theme = theme;
      selected = tab; selectedTheme = theme;
    };
    const select = async (tab, theme = wantedTheme, focus = false) => {
      wanted = tab; wantedTheme = theme;
      const id = ++requestId;
      if (focus) tab.focus();
      const src = tab.dataset.src.replace('.png', `${theme === 'neon' ? '' : '-' + theme}.png`);
      figure?.setAttribute('aria-busy', 'true');
      if (status) status.textContent = '';
      const loading = setTimeout(() => { if (id === requestId && status) status.textContent = 'Загружаем снимок…'; }, 400);
      try {
        const next = await load(src);
        if (id !== requestId) return;
        commit(tab, theme, src, next);
        if (status) status.textContent = `${tab.dataset.title}. Тема «${themeNames[theme]}».`;
        gallery.classList.remove('gallery-error');
        const stage = gallery.querySelector('.screen-stage');
        if (typeof stage.animate === 'function' && !matchMedia('(prefers-reduced-motion: reduce)').matches && !document.body.classList.contains('motion-paused')) {
          stage.getAnimations().forEach(animation => animation.cancel());
          stage.animate([{ opacity: .8 }, { opacity: 1 }], { duration: 180, easing: 'ease-out' });
        }
      } catch {
        if (id !== requestId) return;
        wanted = selected; wantedTheme = selectedTheme;
        for (const radio of radios) radio.checked = radio.value === selectedTheme;
        gallery.classList.add('gallery-error');
        if (status) status.textContent = 'Снимок не загрузился. Показан предыдущий экран. Повторите выбор экрана или выберите другую тему.';
      } finally {
        clearTimeout(loading);
        if (id === requestId) figure?.removeAttribute('aria-busy');
      }
    };
    gallery.querySelector('[role="tablist"]').hidden = false;
    if (picker) picker.hidden = false;
    for (const radio of radios) radio.addEventListener('change', () => { if (radio.checked) select(wanted, radio.value); });
    tabs.forEach((tab, index) => {
      tab.addEventListener('click', () => select(tab));
      tab.addEventListener('keydown', event => {
        const keys = { ArrowRight: (index + 1) % tabs.length, ArrowLeft: (index - 1 + tabs.length) % tabs.length, Home: 0, End: tabs.length - 1 };
        if (event.key in keys) { event.preventDefault(); select(tabs[keys[event.key]], wantedTheme, true); }
      });
    });
  }
  // Every thumbnail remains a normal image link without JavaScript.
  const links = [...document.querySelectorAll('[data-screenshot]')];
  if (!links.length || typeof HTMLDialogElement === 'undefined' || !HTMLDialogElement.prototype.showModal) return;
  const dialog = document.createElement('dialog');
  dialog.className = 'preview-dialog media-dialog';
  dialog.setAttribute('aria-labelledby', 'media-title');
  dialog.setAttribute('aria-describedby', 'media-note');
  dialog.innerHTML = '<div class="preview-dialog-heading"><h2 id="media-title"></h2><button class="dialog-close" type="button" aria-label="Закрыть просмотр" autofocus>×</button></div><p id="media-note">Снимок интерфейса на демонстрационных данных. Esc — закрыть.</p><img width="560" height="740" alt="">';
  document.body.append(dialog);
  let opener;
  for (const link of links) link.addEventListener('click', event => {
    if (event.button !== 0 || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
    event.preventDefault();
    opener = link;
    dialog.querySelector('h2').textContent = link.dataset.title;
    const image = dialog.querySelector('img');
    image.src = link.href;
    image.alt = link.querySelector('img').alt;
    image.width = link.querySelector('img').naturalWidth || Number(link.querySelector('img').getAttribute('width'));
    image.height = link.querySelector('img').naturalHeight || Number(link.querySelector('img').getAttribute('height'));
    dialog.showModal();
    dialog.scrollTop = 0;
  });
  dialog.querySelector('button').addEventListener('click', () => dialog.close());
  dialog.addEventListener('click', event => {
    const rect = dialog.getBoundingClientRect();
    if (event.target === dialog && (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom)) dialog.close();
  });
  dialog.addEventListener('close', () => opener?.focus({ preventScroll: true }));
})();
