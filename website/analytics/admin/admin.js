(() => {
  'use strict';
  const $ = id => document.getElementById(id);
  const format = new Intl.NumberFormat('ru-RU');
  const dateFormat = new Intl.DateTimeFormat('ru-RU', { day: 'numeric', month: 'short', timeZone: 'UTC' });
  const regions = new Intl.DisplayNames(['ru'], { type: 'region' });
  let csrf = '', busy = false, sessionCheck = true, expiryTimer, expires = 0, challengeTimer;
  const metricKeys = ['uniqueVisitors', 'visits', 'pageviews', 'github', 'downloads'];
  const message = (id, text = '', error = false) => { $(id).textContent = text; $(id).classList.toggle('error', error); };
  function signedOut(text = '') {
    clearTimeout(expiryTimer); clearTimeout(challengeTimer); csrf = '';
    $('dashboard').hidden = true; $('login').hidden = false;
    $('login-form').hidden = false; $('code-step').hidden = true; $('code').value = '';
    $('login-title').textContent = 'Вход в панель'; $('login-subtitle').textContent = 'Сайт, релизы и статистика';
    metricKeys.forEach(key => { $('metric-' + key).textContent = '—'; });
    ['daily-rows', 'countries', 'chart'].forEach(id => $(id).replaceChildren());
    $('password').value = ''; message('login-status', text);
    document.dispatchEvent(new Event('mukhomor-signout'));
  }
  async function api(path, options = {}) {
    const response = await fetch('/admin/api/' + path, {
      ...options, credentials: 'same-origin', cache: 'no-store', redirect: 'error', signal: AbortSignal.timeout(10000),
      headers: { 'X-Admin-Request': '1', ...(options.body ? { 'Content-Type': 'application/json' } : {}), ...options.headers }
    });
    if (!response.ok) {
      const error = new Error('request_failed'); error.status = response.status;
      try { error.code=(await response.json()).error; } catch { /* Never display arbitrary server responses. */ }
      error.retry = Number(response.headers.get('Retry-After')) || 60; throw error;
    }
    return response.status === 204 ? null : response.json();
  }
  function signedIn(session) {
    clearTimeout(expiryTimer); clearTimeout(challengeTimer); csrf = session.csrf;
    $('code').value = '';
    $('account-name').textContent = session.username;
    $('login').hidden = true; $('dashboard').hidden = false;
    $('dashboard-title').focus({ preventScroll: true });
    expires = session.expires;
    expireAt(Math.min(expires, session.idle));
    document.dispatchEvent(new Event('mukhomor-signin'));
  }
  window.MukhomorAdmin=Object.freeze({request:async(path,options={})=>{
    if(!csrf){const error=new Error('signed_out');error.status=401;throw error;}
    try { const result=await api(path,{...options,headers:{...options.headers,'X-CSRF-Token':csrf}});expireAt(Math.min(expires,Date.now()+15*60000));return result; }
    catch(error){if(error.status===401)signedOut('Сессия истекла. Войдите снова.');throw error;}
  }});
  function needsCode(challenge) {
    signedOut(); $('login-form').hidden = true; $('code-step').hidden = false;
    $('login-title').textContent = 'Код из Telegram';
    $('login-subtitle').textContent = 'Шаг 2 из 2 · подтвердите вход';
    $('code-reference').textContent = 'Запрос ' + challenge.reference;
    $('code-expiry').textContent = 'Действует до ' + new Intl.DateTimeFormat('ru-RU', { hour: '2-digit', minute: '2-digit' }).format(new Date(challenge.expires));
    $('code').focus();
    challengeTimer = setTimeout(() => signedOut('Код истёк. Введите логин и пароль ещё раз.'), Math.max(0, challenge.expires - Date.now()));
  }
  function expireAt(deadline) { clearTimeout(expiryTimer); expiryTimer = setTimeout(() => signedOut('Сессия истекла. Войдите снова.'), Math.max(0, deadline - Date.now())); }
  function render(report) {
    metricKeys.forEach(key => { $('metric-' + key).textContent = format.format(report[key]); });
    const date = day => dateFormat.format(new Date(day + 'T00:00:00Z'));
    $('date-range').textContent = `${date(report.from)} — ${date(report.to)} · UTC`;
    $('chart-from').textContent = date(report.from); $('chart-to').textContent = date(report.to);
    const byDay = new Map(report.daily.map(item => [item.day, item]));
    const days = [];
    for (let stamp = Date.parse(report.from + 'T00:00:00Z'); stamp <= Date.parse(report.to + 'T00:00:00Z'); stamp += 86400000) {
      const day = new Date(stamp).toISOString().slice(0, 10);
      days.push(byDay.get(day) || { day, uniqueVisitors: 0, visits: 0, pageviews: 0, github: 0, downloads: 0 });
    }
    $('chart').replaceChildren(); $('daily-rows').replaceChildren();
    const maximum = Math.max(1, ...days.map(item => item.visits));
    for (const item of days) {
      const bar = document.createElement('div'), fill = document.createElement('i');
      bar.className = 'chart-bar'; bar.title = `${date(item.day)}: ${format.format(item.visits)}`;
      fill.style.setProperty('--bar-height', (item.visits / maximum * 100) + '%');
      bar.append(fill); $('chart').append(bar);
      const row = document.createElement('tr');
      [date(item.day), ...metricKeys.map(key => format.format(item[key]))].forEach(value => {
        const cell = document.createElement('td'); cell.textContent = value; row.append(cell);
      });
      $('daily-rows').append(row);
    }
    $('chart').setAttribute('aria-label', `Посещения по дням: ${format.format(report.visits)} за период. Таблица ниже.`);
    if (!report.visits) {
      const empty = document.createElement('span'); empty.className = 'chart-empty'; empty.textContent = 'За этот период посещений нет'; $('chart').append(empty);
    }
    $('countries').replaceChildren();
    for (const country of report.countries) {
      const row = document.createElement('div'), heading = document.createElement('div'), name = document.createElement('span');
      const value = document.createElement('span'), track = document.createElement('div'), fill = document.createElement('i');
      const percent = report.visits ? country.visits / report.visits * 100 : 0;
      row.className = 'country'; heading.className = 'country-heading'; name.className = 'country-name'; value.className = 'country-value'; track.className = 'country-track';
      name.textContent = country.country === 'ZZ' ? 'Не определено' : regions.of(country.country);
      value.textContent = `${format.format(country.visits)} · ${Math.round(percent)}%`;
      fill.style.setProperty('--bar-width', Math.min(100, percent) + '%');
      track.append(fill); heading.append(name, value); row.append(heading, track); $('countries').append(row);
    }
    if (!report.countries.length) { const empty = document.createElement('p'); empty.className = 'muted'; empty.textContent = 'Данные появятся после посещений'; $('countries').append(empty); }
    $('updated').textContent = 'Обновлено ' + new Intl.DateTimeFormat('ru-RU', { hour: '2-digit', minute: '2-digit' }).format(new Date());
  }
  async function refresh() {
    if (busy || !csrf) return;
    busy = true; $('refresh').disabled = true; $('period').disabled = true;
    message('dashboard-status', 'Загрузка…');
    try { const report = await api('report?days=' + $('period').value); if (csrf) { render(report); expireAt(Math.min(expires, Date.now() + 15 * 60000)); message('dashboard-status'); } }
    catch (error) {
      if (error.status === 401) signedOut('Сессия истекла. Войдите снова.');
      else message('dashboard-status', 'Не удалось обновить данные. Повторите позже.', true);
    } finally { busy = false; $('refresh').disabled = false; $('period').disabled = false; }
  }
  $('login-form').addEventListener('submit', async event => {
    event.preventDefault(); if (busy || sessionCheck) return;
    busy = true; $('submit').disabled = true; message('login-status', 'Проверка…');
    let body = JSON.stringify({ username: $('username').value, password: $('password').value });
    $('password').value = '';
    try { needsCode(await api('login', { method: 'POST', body })); }
    catch (error) {
      const text = error.status === 429 ? `Лимит попыток. Повторите через ${Math.ceil(error.retry / 60)} мин.` : error.status === 401 ? 'Неверный логин или пароль.' : 'Вход недоступен. Повторите позже.';
      message('login-status', text, true);
    } finally { body = ''; busy = false; $('submit').disabled = false; }
  });
  $('code-form').addEventListener('submit', async event => {
    event.preventDefault(); if (busy || sessionCheck) return;
    busy = true; $('verify').disabled = true; $('back').disabled = true; message('login-status', 'Проверка кода…');
    let body = JSON.stringify({ code: $('code').value }); $('code').value = '';
    try { signedIn(await api('verify', { method: 'POST', body })); }
    catch (error) {
      if (error.status === 401) signedOut('Код истёк или попытки закончились. Войдите заново.');
      else message('login-status', error.status === 400 ? 'Неверный код. Проверьте номер запроса в Telegram.' : error.status === 429 ? 'Слишком много запросов. Подождите минуту.' : 'Не удалось проверить код. Повторите позже.', true);
    } finally { body = ''; busy = false; $('verify').disabled = false; $('back').disabled = false; }
    if (csrf) await refresh();
  });
  $('back').addEventListener('click', async () => {
    if (busy) return;
    busy = true; $('back').disabled = true;
    try { await api('cancel', { method: 'POST', body: '{}' }); signedOut('Для нового кода войдите заново. Между отправками — минута.'); $('username').focus(); }
    catch { message('login-status', 'Не удалось отменить вход. Повторите позже.', true); }
    finally { busy = false; $('back').disabled = false; }
  });
  $('logout').addEventListener('click', async () => {
    $('logout').disabled = true;
    try { await api('logout', { method: 'POST', headers: { 'X-CSRF-Token': csrf }, body: '{}' }); signedOut('Вы вышли из панели.'); $('username').focus(); }
    catch (error) { if (error.status === 401) signedOut('Сессия завершена.'); else message('dashboard-status', 'Не удалось завершить сессию. Повторите выход.', true); }
    finally { $('logout').disabled = false; }
  });
  $('refresh').addEventListener('click', refresh); $('period').addEventListener('change', refresh);
  // No polling: an unattended tab must not keep the idle session alive.
  window.addEventListener('pageshow', event => { if (event.persisted) location.reload(); });
  (async () => {
    try { signedIn(await api('session')); await refresh(); }
    catch (error) {
      if (error.status === 401) {
        try { needsCode(await api('challenge')); }
        catch (challengeError) { signedOut(challengeError.status === 401 ? '' : 'Сервер недоступен. Повторите позже.'); }
      } else signedOut('Сервер недоступен. Повторите позже.');
    } finally { sessionCheck = false; }
  })();
  const sample = document.createElement('canvas'); sample.width = 144; sample.height = 96;
  const context = sample.getContext('2d');
  if (context) {
    const grain = context.createImageData(144, 96); let seed = 0x4d554b48;
    for (let i = 0; i < grain.data.length; i += 4) {
      seed ^= seed << 13; seed ^= seed >>> 17; seed ^= seed << 5;
      grain.data[i] = grain.data[i + 1] = grain.data[i + 2] = (seed >>> 30) * 85; grain.data[i + 3] = 255;
    }
    context.putImageData(grain, 0, 0);
    const texture = document.createElement('div'); texture.className = 'pixel-texture'; texture.setAttribute('aria-hidden', 'true');
    texture.style.backgroundImage = `url("${sample.toDataURL()}")`;
    const resize = () => { const scale = Math.max(document.documentElement.clientWidth / 144, window.innerHeight / 96); texture.style.backgroundSize = `${144 * scale}px ${96 * scale}px`; };
    resize(); window.addEventListener('resize', resize, { passive: true }); document.body.append(texture);
  }
})();
