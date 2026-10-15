(() => {
  'use strict';
  const $=id=>document.getElementById(id),create=(tag,text,className)=>{const node=document.createElement(tag);if(text!==undefined)node.textContent=text;if(className)node.className=className;return node;};
  const pageNames={home:'Главная',docs:'Документация',faq:'Вопросы и ответы',policy:'Правила использования',privacy:'Данные сайта'};
  let state=null,dirty=false,pending=false,view='releases',generation=0;
  const message=(text,error=false)=>{$('cms-status').textContent=text;$('cms-status').classList.toggle('error',error);};
  const describe=error=>({content_conflict:'Черновик изменён в другой вкладке. Скопируйте нужные правки и загрузите свежую версию.',github_release_not_found:'Такой опубликованный релиз не найден на GitHub.',github_unavailable:'GitHub временно недоступен. Попробуйте позже.',github_prerelease_cannot_be_stable:'Сначала снимите отметку prerelease у выпуска на GitHub.',prerelease_cannot_be_stable:'Версия с суффиксом предварительного выпуска не может быть Stable.',invalid_recommended_release:'Рекомендуемая версия должна быть видимой и стабильной.',import_release_from_github:'Сведения о файлах изменились. Импортируйте версию с GitHub заново.',content_rate_limited:'Слишком много изменений. Подождите минуту.'}[error.code] || (error.status===401?'Сессия истекла. Войдите снова.':'Не удалось выполнить действие. Проверьте поля и повторите.'));
  const date=time=>new Intl.DateTimeFormat('ru-RU',{dateStyle:'short',timeStyle:'short',timeZone:'Europe/Moscow'}).format(new Date(time));
  function controls() {
    $('cms-save').disabled=pending || !state || !dirty;
    $('cms-publish').disabled=pending || !state || dirty || state.revision===state.publishedRevision;
    $('cms-export').disabled=pending || !state?.publishedRevision;
    $('cms-refresh').disabled=pending;
    $('cms-preview').disabled=pending || !state;
    $('cms-restore').disabled=pending || !state?.history.length;
    for(const input of document.querySelectorAll('#release-editor input, #release-editor select, #release-editor textarea, #release-editor button, #text-editor input, #text-editor select, #text-editor textarea'))input.disabled=pending || !state;
    $('cms-revision').textContent=state?`Черновик ${state.revision}${dirty?' · есть несохранённые изменения':''}`:'';
    $('cms-publication').textContent=state?.publishedAt?`Опубликовано: ${date(state.publishedAt)} МСК · версия ${state.publishedRevision}`:'Данные сайта ещё не опубликованы';
  }
  function changed(){dirty=true;controls();message('Изменения пока только в этой вкладке. Сохраните черновик.');}
  function selectView(next) {
    view=next;
    for(const button of document.querySelectorAll('[data-admin-view]'))button.setAttribute('aria-pressed',String(button.dataset.adminView===view));
    $('analytics-panel').hidden=view!=='analytics';$('manage-panel').hidden=view==='analytics';
    $('release-editor').hidden=view!=='releases';$('text-editor').hidden=view!=='texts';
    $('date-range').hidden=view!=='analytics';
  }
  function field(label,value,change,multiline=false) {
    const wrapper=create('label',undefined,'cms-field');wrapper.append(create('span',label));
    const input=create(multiline?'textarea':'input');input.value=value;input.maxLength=multiline?32000:160;
    if(multiline)input.rows=Math.min(10,Math.max(3,value.split('\n').length));
    input.addEventListener('input',()=>{change(input.value);changed();});wrapper.append(input);return {wrapper,input};
  }
  function recommendations() {
    const select=$('recommended-release');select.replaceChildren();
    const auto=create('option','Автоматически: последняя Stable');auto.value='';select.append(auto);
    for(const release of state.document.releases.filter(item=>item.visible && item.status==='stable')){const option=create('option',release.version);option.value=release.version;select.append(option);}
    select.value=state.document.recommended || '';
    if(select.selectedIndex===-1){state.document.recommended=null;select.value='';changed();}
  }
  function releaseEditors() {
    $('release-edit-list').replaceChildren();
    for(const release of state.document.releases) {
      const panel=create('details',undefined,'cms-release');panel.open=state.document.releases.length<4;
      const summary=create('summary',`${release.version} · ${release.status==='stable'?'Stable':'В проверке'}${release.visible?'':' · скрыт'}`);panel.append(summary);
      const body=create('div',undefined,'cms-release-body');
      body.append(field('Название',release.title,value=>release.title=value).wrapper);
      const status=create('label',undefined,'cms-field');status.append(create('span','Статус'));const select=create('select');
      for(const [value,label] of [['testing','В проверке — доступна для скачивания'],['stable','Stable — стабильная']]){const option=create('option',label);option.value=value;select.append(option);}select.value=release.status;
      select.addEventListener('change',()=>{release.status=select.value;summary.textContent=`${release.version} · ${release.status==='stable'?'Stable':'В проверке'}${release.visible?'':' · скрыт'}`;changed();recommendations();});status.append(select);body.append(status);
      body.append(field('Что изменилось — один пункт на строку',release.changes.join('\n'),value=>release.changes=value.split('\n').map(line=>line.trim()).filter(Boolean),true).wrapper);
      const visible=create('label',undefined,'cms-checkbox'),checkbox=create('input');checkbox.type='checkbox';checkbox.checked=release.visible;
      checkbox.addEventListener('change',()=>{release.visible=checkbox.checked;changed();recommendations();summary.textContent=`${release.version} · ${release.status==='stable'?'Stable':'В проверке'}${release.visible?'':' · скрыт'}`;});visible.append(checkbox,create('span','Показывать на сайте'));body.append(visible);
      body.append(create('p',`Опубликовано на GitHub: ${date(release.publishedAt)} МСК`,'muted'));
      const files=create('ul',undefined,'cms-file-list');for(const file of release.files)files.append(create('li',`${file.name} · ${(file.size/1048576).toFixed(1)} МБ`));body.append(files);
      const source=create('a','Открыть выпуск на GitHub ↗','cms-link');source.href='https://github.com/0nionow1/Mukhomor/releases/tag/v'+encodeURIComponent(release.version);source.target='_blank';source.rel='noopener noreferrer';body.append(source);panel.append(body);$('release-edit-list').append(panel);
    }
    recommendations();
  }
  function textEditors() {
    if(!state)return;
    const page=$('content-page').value,query=$('content-search').value.toLowerCase().trim();$('content-fields').replaceChildren();
    for(const definition of state.fields.filter(item=>item.page===page && (!query || (item.label+' '+state.document.texts[item.key]).toLowerCase().includes(query)))) {
      const editor=field(definition.label,state.document.texts[definition.key],value=>state.document.texts[definition.key]=value,definition.maxLength>400);
      editor.input.maxLength=definition.maxLength;editor.input.required=!definition.allowEmpty;$('content-fields').append(editor.wrapper);
    }
  }
  function render() {
    releaseEditors();textEditors();$('cms-history').replaceChildren();
    for(const item of state.history){const option=create('option',`Версия ${item.revision} · ${date(item.created)} МСК`);option.value=item.revision;$('cms-history').append(option);}
    $('cms-restore').disabled=!state.history.length;
    const names={save:'Сохранён черновик',publish:'Опубликованы данные',restore:'Восстановлена версия в черновик'};
    $('cms-audit').replaceChildren();for(const entry of state.audit)$('cms-audit').append(create('li',`${date(entry.created)} МСК — ${names[entry.action] || entry.action}, версия ${entry.revision}`));controls();
  }
  async function request(path,body) {return window.MukhomorAdmin.request('site'+path,body===undefined?{}:{method:'POST',body:JSON.stringify(body)});}
  async function action(task,success) {
    if(pending)return;const current=generation;pending=true;controls();message('Выполняется…');
    try{const result=await task(()=>current===generation);if(current!==generation)return;if(result?.document){state=result;dirty=false;render();}if(success)message(success);return result;}
    catch(error){if(current===generation)message(describe(error),true);}
    finally{if(current===generation){pending=false;controls();}}
  }
  function preview(publishing=false) {
    if(!state)return;
    $('cms-preview-title').textContent=publishing?'Публикация данных сайта':'Предпросмотр содержимого';
    const preview=$('cms-preview-body');preview.replaceChildren();
    for(const release of state.document.releases.filter(item=>item.visible)) {
      const card=create('section',undefined,'cms-preview-release');card.append(create('h3',release.version+' · '+(release.status==='stable'?'Stable':'В проверке')),create('p',release.title));
      const list=create('ul');release.changes.forEach(change=>list.append(create('li',change)));card.append(list);preview.append(card);
    }
    if(view==='texts')for(const definition of state.fields.filter(item=>item.page===$('content-page').value))preview.append(create('p',state.document.texts[definition.key]));
    $('cms-confirm-publish').hidden=!publishing;
    $('cms-preview-note').textContent=publishing?'Опубликованные данные станут доступны посетителям сайта. Сохранённые изменения текстов и релизов будут применены вместе.':'Предпросмотр текущего черновика. Данные посетителей и файлы приложения не меняются.';
    $('cms-preview-dialog').showModal();
  }
  for(const [page,name] of Object.entries(pageNames)){const option=create('option',name);option.value=page;$('content-page').append(option);}
  for(const button of document.querySelectorAll('[data-admin-view]'))button.addEventListener('click',()=>selectView(button.dataset.adminView));
  $('content-page').addEventListener('change',textEditors);$('content-search').addEventListener('input',textEditors);
  $('recommended-release').addEventListener('change',()=>{state.document.recommended=$('recommended-release').value || null;changed();});
  $('cms-save').addEventListener('click',()=>action(()=>request('/save',{revision:state.revision,document:state.document}),'Черновик сохранён. Для применения нажмите «Опубликовать».'));
  $('cms-refresh').addEventListener('click',()=>{if(dirty && !confirm('Загрузить сохранённый черновик? Несохранённые правки в этой вкладке будут потеряны.'))return;action(()=>request(''),'Черновик загружен.');});
  $('cms-preview').addEventListener('click',()=>preview(false));$('cms-publish').addEventListener('click',()=>preview(true));
  $('cms-close-preview').addEventListener('click',()=>$('cms-preview-dialog').close());
  $('cms-confirm-publish').addEventListener('click',()=>{$('cms-preview-dialog').close();action(()=>request('/publish',{revision:state.revision}),'Опубликовано. Сайт получит изменения в течение 30 секунд после открытия или обновления страницы.');});
  $('cms-import-form').addEventListener('submit',event=>{
    event.preventDefault();if(!state || pending)return;
    action(async isCurrent=>{
      const imported=await request('/import',{version:$('cms-version').value.trim()});
      if(!isCurrent())return;
      const existing=state.document.releases.find(item=>item.version===imported.version);
      if(existing){existing.files=imported.files;existing.publishedAt=imported.publishedAt;if(imported.prerelease)existing.status='testing';}
      else state.document.releases.unshift({version:imported.version,title:imported.title,changes:imported.changes,status:'testing',visible:true,publishedAt:imported.publishedAt,files:imported.files});
      dirty=true;$('cms-version').value='';releaseEditors();controls();
    },'Релиз добавлен в форму со статусом «В проверке». Заполните описание и сохраните черновик.');
  });
  $('cms-restore').addEventListener('click',()=>{if(dirty && !confirm('Заменить несохранённые правки выбранной версией?'))return;action(()=>request('/restore',{revision:state.revision,fromRevision:Number($('cms-history').value)}),'Версия восстановлена в черновик. Проверьте её перед публикацией.');});
  $('cms-export').addEventListener('click',()=>action(async isCurrent=>{
    const document=await request('/export');if(!isCurrent())return;
    const url=URL.createObjectURL(new Blob([JSON.stringify(document,null,2)+'\n'],{type:'application/json'}));
    const link=create('a');link.href=url;link.download='mukhomor-content.json';link.click();setTimeout(()=>URL.revokeObjectURL(url),1000);
  },'Опубликованный снимок сохранён в файл.'));
  document.addEventListener('mukhomor-signin',()=>{selectView('releases');action(()=>request(''),'Черновик загружен.');});
  document.addEventListener('mukhomor-signout',()=>{generation++;state=null;dirty=false;pending=false;for(const id of ['content-fields','release-edit-list','cms-preview-body','cms-audit','cms-history'])$(id).replaceChildren();$('cms-preview-dialog').close();controls();});
  window.addEventListener('beforeunload',event=>{if(dirty){event.preventDefault();event.returnValue='';}});
})();
