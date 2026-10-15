(() => {
  'use strict';
  const config = window.MukhomorAnalytics;
  // Renew the choice when the purpose, providers or notice materially change.
  const NOTICE = '2026-10-15.1', CHOICE_MS = 180 * 86400000;
  const keys = { consent: 'mukhomor.analytics.consent', visitor: 'mukhomor.analytics.visitor', session: 'mukhomor.analytics.session' };
  const read = key => { try { return JSON.parse(localStorage.getItem(key)); } catch { return null; } };
  const write = (key, value) => { try { localStorage.setItem(key, JSON.stringify(value)); } catch { /* Storage is optional. */ } };
  const remove = key => { try { localStorage.removeItem(key); } catch { /* Storage may be unavailable. */ } };
  const optedOut = () => navigator.globalPrivacyControl === true || navigator.doNotTrack === '1' || window.doNotTrack === '1';
  const validChoice = value => value && value.version === NOTICE && typeof value.accepted === 'boolean' && Number.isSafeInteger(value.at) && Number.isSafeInteger(value.expires) && value.at <= Date.now() && value.expires === value.at + CHOICE_MS && value.expires > Date.now();
  let choice = read(keys.consent);
  if (!validChoice(choice)) { choice = null; remove(keys.consent); remove(keys.visitor); remove(keys.session); }
  let consent = choice?.accepted === true && !optedOut(), session, pendingSession, widget, panel, stopped = false, generation = 0, cancelChallenge, trackingReady = false;
  const requests = new Set();
  const preference = document.querySelector('#analytics-preference-status');
  const status = text => { if (preference) preference.textContent = text; };
  const controls = () => {
    for (const button of document.querySelectorAll('[data-analytics-revoke]')) button.hidden = !trackingReady || !consent;
    for (const button of document.querySelectorAll('[data-analytics-settings]')) button.hidden = false;
  };
  const remember = accepted => { const at = Date.now(); choice = { version: NOTICE, accepted, at, expires: at + CHOICE_MS }; write(keys.consent, choice); };
  const stop = () => {
    generation++;
    consent = false; stopped = true; session = null;
    pendingSession = null;
    remove(keys.visitor); remove(keys.session);
    for (const controller of requests) controller.abort(); requests.clear();
    cancelChallenge?.();
    if (widget !== undefined && window.turnstile) window.turnstile.remove(widget);
    widget = undefined;
    document.querySelector('.analytics-challenge')?.remove();
    controls();
  };
  const revoke = () => {
    stop(); remember(false); panel?.remove();
    status('Статистика отключена в этом браузере.');
  };
  function showPanel() {
    panel?.remove(); panel = document.createElement('aside'); panel.className = 'analytics-consent';
    panel.setAttribute('aria-label', 'Настройки cookies и статистики');
    const available = trackingReady && !optedOut();
    panel.innerHTML = available
      ? '<p>Разрешить статистику?</p><p class="consent-detail">Проект «Мухомор» использует localStorage для подсчёта посещений, стран и переходов. Проверка Cloudflare Turnstile загрузится после согласия. Выбор сохраняется на 180 дней; отказ не мешает работе сайта.</p><a class="text-link" href="privacy.html">Cookies, данные и сроки хранения</a><div class="analytics-actions"><button type="button" class="button button-secondary" data-choice="no">Отказаться</button><button type="button" class="button button-secondary" data-choice="yes">Разрешить</button></div>'
      : '<p>' + (optedOut() ? 'Статистика отключена сигналом DNT/GPC в браузере.' : 'Сбор статистики пока не включён.') + '</p><a class="text-link" href="privacy.html">Cookies и данные</a>';
    const close = document.createElement('button'); close.type = 'button'; close.className = 'text-link'; close.textContent = 'Закрыть';
    close.addEventListener('click', () => panel.remove()); panel.append(close);
    panel.addEventListener('keydown', event => { if (event.key === 'Escape') panel.remove(); });
    panel.addEventListener('click', event => {
      const button = event.target.closest('[data-choice]'); if (!button || !available || optedOut()) return;
      if (button.dataset.choice === 'no') return revoke();
      remember(true); consent = true; stopped = false; panel.remove(); controls();
      status('Статистика разрешена в этом браузере.'); emit('page_view');
    });
    document.body.append(panel);
  }
  for (const button of document.querySelectorAll('[data-analytics-revoke]')) button.addEventListener('click', revoke);
  for (const button of document.querySelectorAll('[data-analytics-settings]')) button.addEventListener('click', showPanel);
  controls();
  if (!config?.enabled) { stop(); status('Сбор статистики пока не включён.'); return; }
  if (optedOut()) { stop(); status('Статистика отключена сигналом DNT/GPC в браузере.'); return; }
  let base, endpoint;
  try {
    base = new URL(config.siteUrl); endpoint = new URL(config.endpoint);
    if (location.origin !== base.origin || !location.pathname.startsWith(base.pathname) || endpoint.protocol !== 'https:') return;
  } catch { return; }
  const pages = { '': 'home', 'index.html': 'home', 'docs.html': 'docs', 'changelog.html': 'changelog', 'faq.html': 'faq', 'privacy.html': 'privacy', 'policy.html':'policy', 'releases.html':'releases' };
  const page = pages[location.pathname.slice(base.pathname.length)];
  if (!page) return;
  trackingReady = true; controls();
  const allowed = () => consent && !stopped && !optedOut() && validChoice(choice);
  window.addEventListener('storage', event => {
    if (event.key !== keys.consent && event.key !== null) return;
    const updated = read(keys.consent); stop();
    choice = validChoice(updated) ? updated : null;
    if (choice?.accepted && !optedOut()) { consent = true; stopped = false; controls(); panel?.remove(); emit('page_view'); }
    else { status('Статистика отключена в этом браузере.'); panel?.remove(); }
  });
  document.addEventListener('visibilitychange', () => {
    if (!document.hidden && consent && !allowed()) { stop(); choice = null; remove(keys.consent); status('Для статистики нужно новое согласие.'); if (!optedOut()) showPanel(); }
  });
  const active = candidate => candidate && typeof candidate.token === 'string' && candidate.expires > Date.now() + 10000 && candidate.endpoint === endpoint.origin;
  async function post(path, body, token, keepalive = false) {
    if (!allowed()) throw new Error('Disabled');
    const controller = new AbortController(), current = generation; requests.add(controller);
    try { const response = await fetch(new URL(path, endpoint), {
      method: 'POST', mode: 'cors', credentials: 'omit', referrerPolicy: 'no-referrer', cache: 'no-store',
      headers: { 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) },
      body: JSON.stringify(body), keepalive, signal: AbortSignal.any([controller.signal, AbortSignal.timeout(8000)])
    });
      if (!allowed() || generation !== current) throw new Error('Disabled');
      return response;
    } finally { requests.delete(controller); }
  }
  let scriptReady;
  function challenge() {
    if (!scriptReady) scriptReady = new Promise((resolve, reject) => {
      if (window.turnstile) return resolve();
      const script = document.createElement('script');
      script.src = 'https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit';
      script.async = true; script.onload = resolve; script.onerror = reject;
      document.head.append(script);
    });
    const current = generation;
    return scriptReady.then(() => new Promise((resolve, reject) => {
      if (!allowed() || current !== generation) return reject(new Error('Disabled'));
      const box = document.createElement('aside');
      box.className = 'analytics-challenge'; box.setAttribute('aria-label', 'Проверка статистики');
      const cancel = document.createElement('button'); cancel.type = 'button'; cancel.textContent = 'Отключить статистику';
      cancel.className = 'text-link'; cancel.addEventListener('click', revoke);
      const host = document.createElement('div'); box.append(host, cancel); document.body.append(box);
      let settled = false;
      const finish = (error, token) => {
        if (settled) return; settled = true;
        cancelChallenge = undefined;
        clearTimeout(timer); box.remove();
        if (widget !== undefined) { window.turnstile.remove(widget); widget = undefined; }
        if (error) reject(error); else resolve(token);
      };
      const timer = setTimeout(() => finish(new Error('Challenge expired')), 120000);
      cancelChallenge = () => finish(new Error('Disabled'));
      widget = window.turnstile.render(host, {
        sitekey: config.turnstileSiteKey, action: 'mukhomor-analytics', theme: 'dark', size: 'flexible',
        appearance: 'interaction-only', retry: 'never', 'refresh-expired': 'never',
        callback: token => finish(null, token), 'error-callback': () => finish(new Error('Challenge failed')),
        'expired-callback': () => finish(new Error('Challenge expired'))
      });
      if (settled && widget !== undefined) { window.turnstile.remove(widget); widget = undefined; }
    }));
  }
  async function getSession() {
    if (!allowed()) throw new Error('Disabled');
    if (active(session)) return session;
    const stored = read(keys.session);
    if (active(stored)) return (session = stored);
    if (stored) remove(keys.session);
    if (pendingSession) return pendingSession;
    const current = generation;
    const task = (async () => {
      const visitor = read(keys.visitor);
      if (visitor && !(visitor.expires > Date.now())) remove(keys.visitor);
      const response = await post('/v1/session', { challenge: await challenge(), ...(visitor?.expires > Date.now() ? { visitorToken: visitor.token } : {}) });
      if (!response.ok) throw new Error('Session unavailable');
      const result = await response.json();
      if (!allowed() || current !== generation) throw new Error('Disabled');
      session = { token: result.token, expires: result.expires, endpoint: endpoint.origin };
      write(keys.session, session); write(keys.visitor, { token: result.visitorToken, expires: result.visitorExpires });
      return session;
    })().finally(() => { if (pendingSession === task) pendingSession = null; });
    pendingSession = task;
    return pendingSession;
  }
  let viewedSession;
  // Serialise the first view before clicks. Never delay navigation for analytics.
  let queue = Promise.resolve();
  function emit(kind) {
    if (!allowed()) return;
    queue = queue.then(async () => {
      for (let attempt = 0; attempt < 2 && allowed(); attempt++) {
        const current = await getSession();
        if (viewedSession !== current.token) {
          const response = await post('/v1/event', { id: crypto.randomUUID(), kind: 'page_view', page }, current.token, true);
          if (response.status === 401) { session = null; remove(keys.session); continue; }
          if (!response.ok) throw new Error('View rejected');
          viewedSession = current.token;
        }
        if (kind !== 'page_view') {
          const response = await post('/v1/event', { id: crypto.randomUUID(), kind, page }, current.token, true);
          if (response.status === 401) { session = null; remove(keys.session); continue; }
        }
        break;
      }
    }).catch(() => { /* Statistics must never interrupt reading or downloading. */ });
  }
  const onClick = event => {
    if (!event.isTrusted || (event.type === 'auxclick' && event.button !== 1)) return;
    const anchor = event.target.closest('a[href]'); if (!anchor) return;
    const url = new URL(anchor.href);
    if (url.origin !== 'https://github.com' || !/^\/0nionow1\/Mukhomor(?:\/|$)/.test(url.pathname)) return;
    emit(anchor.dataset.analytics === 'download' ? 'download' : 'github');
  };
  document.addEventListener('click', onClick); document.addEventListener('auxclick', onClick);
  if (consent) { status('Статистика разрешена в этом браузере.'); emit('page_view'); }
  else if (!choice) showPanel();
  else { stop(); status('Статистика отключена в этом браузере.'); }
})();
