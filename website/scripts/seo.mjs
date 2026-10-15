import assert from 'node:assert/strict';
import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';
import { compareVersions } from '../analytics/content.mjs';

const root = new URL('../public/', import.meta.url);
const escape = value => value.replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
const json = value => JSON.stringify(value).replace(/</g, '\\u003c');
// Available before any external CSS: no white canvas or uncovered incoming body.
const navigationHead = `  <style id="navigation-critical">
    html { background: #120c18; color-scheme: dark; }
    .navigation-covered::after { content: ''; position: fixed; inset: 0; z-index: 10000; background: #120c18; pointer-events: all; }
    .pixel-transition { position: fixed; inset: 0; z-index: 10001; width: 100%; height: 100%; pointer-events: all; image-rendering: pixelated; }
  </style>
  <script src="navigation.js"></script>`;
// One shared footer keeps navigation consistent as the site gains pages.
function renderFooter(file, note, repo) {
  const link = (href, label) => `<a href="${href}"${file === href ? ' aria-current="page"' : ''}>${label}</a>`;
  return `<footer class="site-footer wrap" aria-label="Подвал сайта">
    <div class="footer-main">
      <div class="footer-identity">
        <a class="footer-brand" href="./" aria-label="Мухомор — на главную"${file === 'index.html' ? ' aria-current="page"' : ''}><img src="assets/mushroom-mini.svg" width="60" height="48" alt="" loading="lazy"><span>Мухомор</span></a>
        <p class="footer-fine">${note}</p>
      </div>
      <nav class="footer-nav" aria-label="Разделы сайта">
        <div class="footer-group">
          <p class="footer-heading">Приложение</p>
          <ul><li>${link('docs.html', 'Документация')}</li><li>${link('releases.html', 'Релизы')}</li><li>${link('changelog.html', 'Обновления')}</li></ul>
        </div>
        <div class="footer-group">
          <p class="footer-heading">Проект</p>
          <ul><li>${link('faq.html', 'Вопросы и ответы')}</li><li><a href="${repo}" target="_blank" rel="noopener noreferrer">GitHub <span class="footer-external" aria-hidden="true">↗</span></a></li></ul>
        </div>
      </nav>
    </div>
    <div class="footer-bottom">
      <nav class="footer-legal" aria-label="Правила и данные">${link('policy.html', 'Правила использования')}${link('privacy.html', 'Cookies и данные')}</nav>
      <div class="footer-settings"><button type="button" class="footer-control" data-analytics-settings hidden>Настройки cookies</button><button type="button" class="footer-control" data-analytics-revoke hidden>Отключить статистику</button><button class="motion-toggle" type="button" aria-pressed="false" hidden>Анимация: вкл.</button></div>
    </div>
  </footer>`;
}
export async function syncSeo(checkOnly = false) {
  const config = JSON.parse(await readFile(new URL('../site.json', import.meta.url), 'utf8'));
  const releases = JSON.parse(await readFile(new URL('../changelog.json', import.meta.url), 'utf8'));
  const content = JSON.parse(await readFile(new URL('../content.json', import.meta.url), 'utf8'));
  const faq = JSON.parse(await readFile(new URL('../faq.json', import.meta.url), 'utf8')).map(item=>({...item,question:content.texts[`faq.${item.id}.question`] || item.question,answer:content.texts[`faq.${item.id}.answer`] || item.answer}));
  const base = new URL(config.url);
  assert(base.protocol === 'https:' && base.pathname.endsWith('/') && !base.search && !base.hash, 'Use a canonical HTTPS site directory');
  const urls = ['index.html', 'docs.html', 'changelog.html', 'faq.html', 'privacy.html', 'policy.html', 'releases.html'];
  const url = file => new URL(file === 'index.html' ? './' : file, base).href;
  const repo = config.repository;
  assert(repo === 'https://github.com/0nionow1/Mukhomor', 'Unexpected project repository');
  const analytics = config.analytics;
  assert(typeof analytics.enabled === 'boolean' && Object.keys(analytics).every(key => ['enabled', 'endpoint', 'turnstileSiteKey'].includes(key)), 'Only public analytics settings belong in site.json');
  if (analytics.enabled) {
    const endpoint = new URL(analytics.endpoint);
    assert(endpoint.protocol === 'https:' && endpoint.pathname === '/' && !endpoint.search && !endpoint.hash && !endpoint.username && !endpoint.password, 'Analytics requires an HTTPS origin');
    assert(/^[A-Za-z0-9_-]{10,100}$/.test(analytics.turnstileSiteKey), 'Missing public Turnstile site key');
  }
  const challengeOrigin = analytics.enabled ? ' https://challenges.cloudflare.com' : '';
  assert(typeof config.content?.enabled==='boolean' && Object.keys(config.content).every(key=>['enabled','endpoint'].includes(key)), 'Use only public content configuration');
  const contentEndpoint=new URL(config.content.endpoint);
  assert(contentEndpoint.protocol==='https:' && contentEndpoint.href===contentEndpoint.origin+'/' && !contentEndpoint.username && !contentEndpoint.password,'Content requires an HTTPS origin');
  const connections=[...new Set([...(analytics.enabled?[new URL(analytics.endpoint).origin,'https://challenges.cloudflare.com']:[]),...(config.content.enabled?[contentEndpoint.origin]:[])])];
  const policy = `default-src 'self'; base-uri 'self'; object-src 'none'; form-action 'none'; script-src 'self'${challengeOrigin}; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'${connections.length?' '+connections.join(' '):''}; frame-src ${analytics.enabled ? 'https://challenges.cloudflare.com' : "'none'"}; upgrade-insecure-requests`;
  const write = async (path, content) => {
    if (checkOnly) assert.equal(await readFile(path, 'utf8'), content, `SEO output is stale: ${path.pathname}; run node website/scripts/seo.mjs`);
    else await writeFile(path, content);
  };
  const app = {
    '@type': 'SoftwareApplication', '@id': `${base}#application`, name: 'Мухомор', alternateName: 'Mukhomor',
    url: base.href, description: faq[0].answer, applicationCategory: 'SecurityApplication',
    operatingSystem: ['Windows 10/11 (x64)', 'Linux (x64)', 'Android 8.0+ (ARM64/x86_64)'], ...(content.releases.some(item=>item.visible)?{softwareVersion:content.releases.filter(item=>item.visible).sort((a,b)=>compareVersions(b.version,a.version))[0].version}:{}),
    downloadUrl: url('releases.html'), releaseNotes: url('changelog.html'),
    license: `${repo}/blob/main/LICENSE`, isAccessibleForFree: true, inLanguage: ['ru', 'en'],
    screenshot: url('assets/app-preview.png'), sameAs: [repo],
    softwareRequirements: 'Собственный VPS с VPN и клиентский профиль. Для Mycelia нужна совместимая сборка.'
  };
  for (const file of urls) {
    const path = new URL(file, root);
    const original = await readFile(path, 'utf8');
    let html = original;
    if (file === 'faq.html') {
      const ids = new Set();
      const entries = faq.map(item => {
        assert(/^[a-z-]+$/.test(item.id) && !ids.has(item.id) && item.question && item.answer, 'Invalid FAQ');
        ids.add(item.id);
        return `<section class="faq-answer" id="${item.id}" aria-labelledby="${item.id}-title"><h2 id="${item.id}-title" data-cms="faq.${item.id}.question">${escape(item.question)}</h2><p data-cms="faq.${item.id}.answer">${escape(item.answer)}</p></section>`;
      }).join('\n');
      html = html.replace(/<!-- faq:start -->[\s\S]*?<!-- faq:end -->/, `<!-- faq:start -->\n${entries}\n<!-- faq:end -->`);
    }
    const title = html.match(/<title>([^<]+)<\/title>/)[1];
    const description = html.match(/<meta name="description" content="([^"]+)"/)[1];
    const page = { '@type': file === 'docs.html' ? 'TechArticle' : 'WebPage', '@id': `${url(file)}#page`, url: url(file), name: title, description, inLanguage: 'ru', isPartOf: { '@id': `${base}#website` }, about: { '@id': app['@id'] } };
    const graph = [{ '@type': 'WebSite', '@id': `${base}#website`, url: base.href, name: 'Мухомор', alternateName: 'Mukhomor', inLanguage: 'ru' }, app, page];
    if (file === 'faq.html') graph.push({ '@type': 'FAQPage', '@id': `${url(file)}#faq`, mainEntity: faq.map(item => ({ '@type': 'Question', name: item.question, acceptedAnswer: { '@type': 'Answer', text: item.answer } })) });
    if (file === 'index.html') graph.push({ '@type': 'SoftwareSourceCode', '@id': `${base}#source`, name: 'Mukhomor', codeRepository: repo, programmingLanguage: ['Rust', 'Slint', 'Go', 'PowerShell', 'Java'], license: app.license, targetProduct: { '@id': app['@id'] } });
    const block = `<!-- seo:start -->\n  <link rel="canonical" href="${url(file)}">\n  <link rel="sitemap" type="application/xml" href="sitemap.xml">\n  <meta name="robots" content="index,follow,max-image-preview:large,max-snippet:-1,max-video-preview:-1">\n  <meta name="referrer" content="strict-origin-when-cross-origin">\n  <meta property="og:type" content="website">\n  <meta property="og:site_name" content="Мухомор">\n  <meta property="og:locale" content="ru_RU">\n  <meta property="og:title" content="${title}">\n  <meta property="og:description" content="${description}">\n  <meta property="og:url" content="${url(file)}">\n  <meta property="og:image" content="${url('assets/social-preview.png')}">\n  <meta property="og:image:width" content="1280">\n  <meta property="og:image:height" content="640">\n  <meta property="og:image:alt" content="Мухомор — VPN-клиент для Windows, Linux и Android; игры и свои правила маршрутизации">\n  <meta name="twitter:card" content="summary_large_image">\n  <meta name="twitter:title" content="${title}">\n  <meta name="twitter:description" content="${description}">\n  <meta name="twitter:image" content="${url('assets/social-preview.png')}">\n  <script type="application/ld+json">${json({ '@context': 'https://schema.org', '@graph': graph })}</script>\n  <script src="analytics-config.js" defer></script>\n  <script src="analytics.js" defer></script>\n  <!-- seo:end -->`;
    if (html.includes('<!-- seo:start -->')) html = html.replace(/<!-- seo:start -->[\s\S]*?<!-- seo:end -->/, block);
    else {
      html = html.replace(/\s*<meta property="og:[^>]+>/g, '');
      html = html.replace('</head>', `  ${block}\n</head>`);
    }
    const footer = html.match(/<footer\b[^>]*class="site-footer wrap"[^>]*>[\s\S]*?<\/footer>/);
    assert(footer, `Missing site footer: ${file}`);
    const note = footer[0].match(/<p class="footer-fine">([\s\S]*?)<\/p>/)?.[1];
    assert(note, `Missing footer note: ${file}`);
    html = html.replace(footer[0], renderFooter(file, note, repo));
    html = html.replace(/\n  <meta http-equiv="Content-Security-Policy"[^>]+>/g, '');
    if(!html.includes('src="content.js"')) html=html.replace('</head>','  <link rel="stylesheet" href="content.css">\n  <script src="content-config.js" defer></script>\n  <script src="content-snapshot.js" defer></script>\n  <script src="content.js" defer></script>\n</head>');
    html = html.replace('<meta charset="utf-8">', `<meta charset="utf-8">\n  <meta http-equiv="Content-Security-Policy" content="${escape(policy)}">`);
    html = html.replace(/\n  <style id="navigation-critical">[\s\S]*?<\/style>/g, '')
      .replace(/\n  <script src="navigation\.js"><\/script>/g, '');
    html = html.replace(/(<meta name="viewport"[^>]+>)/, `$1\n${navigationHead}`);
    await write(path, html);
  }
  await write(new URL('sitemap.xml', root), `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${urls.map(file => `  <url><loc>${escape(url(file))}</loc></url>`).join('\n')}\n</urlset>\n`);
  const robots = `# Must be served at the host root: ${base.origin}/robots.txt\nUser-agent: *\nAllow: /\n\nSitemap: ${url('sitemap.xml')}\n`;
  await write(new URL('robots.txt', root), robots);
  if (!checkOnly) await mkdir(new URL('../domain-root/', import.meta.url), { recursive: true });
  await write(new URL('../domain-root/robots.txt', import.meta.url), robots);
  const facts = `# Мухомор (Mukhomor)\n\n> VPN-клиент с открытым исходным кодом для Windows 10/11 x64, Linux x64 и Android 8.0+. Подключается к собственному VPS с VPN.\n\n${faq.map(item => `## ${item.question}\n\n${item.answer}\n`).join('\n')}\n## Ссылки\n\n- [Главная](${base})\n- [Документация](${url('docs.html')})\n- [Вопросы и ответы](${url('faq.html')})\n- [История обновлений](${url('changelog.html')})\n- [Исходный код](${repo})\n- [Опубликованные выпуски](${repo}/releases)\n\nВерсия опубликованного клиента: ${releases.releases[0].version}. Готовящиеся изменения отмечены отдельно в истории обновлений.\n`;
  await write(new URL('llms.txt', root), facts.replace(/Версия опубликованного клиента:[^\n]+/,`Актуальные версии, статусы и рекомендуемый выпуск: ${url('releases.html')}.\nПубличный JSON опубликованного содержимого: ${contentEndpoint.origin}/v1/site\nПравила использования: ${url('policy.html')}. Статические тексты отражают снимок на момент сборки; актуальный каталог берётся с сервера проекта.`));
  await write(new URL('analytics-config.js', root), `window.MukhomorAnalytics = Object.freeze(${json({ ...analytics, siteUrl: base.href })});\n`);
  await write(new URL('content-config.js', root), `window.MukhomorSiteContent = Object.freeze(${json(config.content)});\n`);
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await syncSeo(process.argv.includes('--check'));
  console.log(process.argv.includes('--check') ? 'PASS: SEO outputs are current.' : 'Updated metadata, FAQ, sitemap, robots, llms and analytics configuration.');
}
