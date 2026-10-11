<script lang="ts">
  import { onMount } from 'svelte'
  import { api, download, raw } from '../lib/api'
  import { app } from '../lib/state.svelte'
  import { t, lang, setLang } from '../lib/i18n.svelte'
  import { date, copy } from '../lib/fmt'
  import { confirmDlg } from '../lib/dialog.svelte'
  import Modal from '../components/Modal.svelte'

  let tab = $state('general')
  let s = $state<any>(null)
  let orig = ''
  let pw1 = $state('')
  let pw2 = $state('')
  let keys = $state<any[]>([])
  let sessions = $state<any[]>([])
  let seclog = $state<any[]>([])
  let aliases = $state<any[]>([])
  let keyForm = $state<any>(null)
  let newSecret = $state('')
  let autostart = $state<boolean | null>(null)
  let restoreEl = $state<HTMLInputElement>()
  let cacheText = $state('')

  async function load() {
    try {
      s = await api.get('/api/settings')
      orig = JSON.stringify(s)
      cacheText = Object.entries(s.custom_cache_bits || {}).map(([k, v]) => `${k}=${v}`).join('\n')
      ;[keys, sessions, aliases] = await Promise.all([api.get('/api/keys'), api.get('/api/auth/sessions'), api.get('/api/aliases')])
      api.get('/api/system/autostart').then((r) => (autostart = r.enabled)).catch(() => (autostart = null))
    } catch (e) { app.err(e) }
  }
  onMount(load)
  const dirty = $derived(s && JSON.stringify(s) !== orig)

  async function save() {
    const bits: Record<string, number> = {}
    for (const l of cacheText.split('\n')) { const [k, v] = l.split('='); if (k?.trim() && v && !isNaN(+v)) bits[k.trim()] = +v }
    s.custom_cache_bits = bits
    const lines = (x: any) => (Array.isArray(x) ? x : String(x || '').split(/[\n,]/)).map((v: string) => v.trim()).filter(Boolean)
    s.allowed_subnets = lines(s.allowed_subnets)
    s.trusted_proxies = lines(s.trusted_proxies)
    s.cors_origins = lines(s.cors_origins)
    try {
      const r = await api.put('/api/settings', s)
      if (s.language !== lang.v) setLang(s.language)
      app.toast('success', t('Настройки сохранены'), r.restarting ? t('Сетевые параметры изменены — сервер перезапускается. Если поменяли порт UI — откройте новый адрес.') : '')
      await load()
      app.status = await api.get('/api/status')
    } catch (e) { app.err(e) }
  }
  async function setPw() {
    if (pw1.length < 8) return app.toast('error', t('Минимум 8 символов'))
    if (pw1 !== pw2) return app.toast('error', t('Пароли не совпадают'))
    try { await api.post('/api/auth/password', { password: pw1 }); pw1 = pw2 = ''; app.toast('success', t('Пароль установлен')); app.auth = await api.get('/api/auth/status') } catch (e) { app.err(e) }
  }
  async function saveKey() {
    const body = { name: keyForm.name, scopes: keyForm.scopes.split(',').map((x: string) => x.trim()).filter(Boolean), can_load: keyForm.can_load, can_manage: keyForm.can_manage, expires_at: keyForm.expires ? Math.floor(new Date(keyForm.expires).getTime() / 1000) : null }
    try {
      if (keyForm.id) await api.put('/api/keys/' + keyForm.id, body)
      else newSecret = (await api.post('/api/keys', body)).secret
      keyForm = null
      keys = await api.get('/api/keys')
    } catch (e) { app.err(e) }
  }
  async function delKey(k: any) {
    if (!(await confirmDlg(t('Отозвать ключ?'), k.name, { danger: true }))) return
    try { await api.del('/api/keys/' + k.id); keys = await api.get('/api/keys') } catch (e) { app.err(e) }
  }
  async function delSession(x: any) {
    try { await api.del('/api/auth/sessions/' + x.id); sessions = await api.get('/api/auth/sessions') } catch (e) { app.err(e) }
  }
  async function loadSec() {
    try { seclog = await api.get('/api/security-log') } catch (e) { app.err(e) }
  }
  async function setAutostart(v: boolean) {
    try { await api.put('/api/system/autostart', { enabled: v }); autostart = v } catch (e) { app.err(e) }
  }
  async function doRestore(e: Event) {
    const f = (e.target as HTMLInputElement).files?.[0]
    if (!f) return
    if (!(await confirmDlg(t('Восстановить из бэкапа?'), t('Текущие настройки, пресеты и ключи будут заменены при следующем запуске лаунчера.'), { danger: true }))) return
    try {
      const r = await raw('/api/restore', { method: 'POST', body: f })
      const j = await r.json()
      if (!r.ok) throw new Error(j.error?.message || 'error')
      app.toast('success', t('Бэкап загружен'), j.message)
    } catch (er) { app.err(er) }
    ;(e.target as HTMLInputElement).value = ''
  }
  const asText = (a: any) => (Array.isArray(a) ? a.join('\n') : a)
</script>

<div class="row" style="margin-bottom:12px">
  <h1 style="margin:0">{t('Настройки')}</h1>
  <span class="sp"></span>
  {#if dirty}<span class="small warn">{t('Есть несохранённые изменения')}</span>{/if}
  <button class="primary" disabled={!dirty} onclick={save}>{t('Сохранить')}</button>
</div>

{#if s}
  <div class="tabs">
    <button class:on={tab === 'general'} onclick={() => (tab = 'general')}>{t('Общие')}</button>
    <button class:on={tab === 'network'} onclick={() => (tab = 'network')}>{t('Сеть и доступ')}</button>
    <button class:on={tab === 'security'} onclick={() => { tab = 'security'; loadSec() }}>{t('Безопасность')}</button>
    <button class:on={tab === 'models'} onclick={() => (tab = 'models')}>{t('Память и запуск')}</button>
    <button class:on={tab === 'downloads'} onclick={() => (tab = 'downloads')}>{t('Загрузки')}</button>
    <button class:on={tab === 'backup'} onclick={() => (tab = 'backup')}>{t('Бэкап и о программе')}</button>
  </div>

  {#if tab === 'general'}
    <div class="card grid g2">
      <label class="field"><span>{t('Язык')}</span><select bind:value={s.language}><option value="ru">Русский</option><option value="en">English</option></select></label>
      <label class="field"><span>{t('Тема')}</span><select bind:value={s.theme} onchange={() => { document.documentElement.dataset.theme = s.theme; localStorage.setItem('lll_theme', s.theme) }}><option value="dark">{t('Тёмная')}</option><option value="light">{t('Светлая')}</option></select></label>
      <label class="chk"><input type="checkbox" bind:checked={s.start_minimized} /> {t('Запускать свёрнутым в трей')}</label>
      <label class="chk"><input type="checkbox" bind:checked={s.check_updates} /> {t('Проверять обновления сборок llama.cpp')}</label>
      {#if autostart !== null}
        <label class="chk"><input type="checkbox" checked={autostart} onchange={(e) => setAutostart(e.currentTarget.checked)} /> {t('Запускать вместе с Windows')}</label>
      {/if}
    </div>
  {:else if tab === 'network'}
    <div class="card col">
      <div class="alert">{t('По умолчанию всё слушает только 127.0.0.1. Чтобы открыть доступ из локальной сети, укажите 0.0.0.0 (или IP адаптера) и задайте пароль во вкладке «Безопасность». HTTPS не встроен — при необходимости поставьте перед лаунчером Caddy или nginx и добавьте его адрес в доверенные прокси.')}</div>
      <div class="grid g2">
        <label class="field"><span>{t('Интерфейс: адрес')}</span><input bind:value={s.ui_bind} /></label>
        <label class="field"><span>{t('Интерфейс: порт')}</span><input type="number" bind:value={s.ui_port} /></label>
        <label class="field"><span>{t('Роутер OpenAI: адрес')}</span><input bind:value={s.router_bind} /></label>
        <label class="field"><span>{t('Роутер OpenAI: порт')}</span><input type="number" bind:value={s.router_port} /></label>
        <label class="field"><span>{t('Внутренние порты llama-server: с')}</span><input type="number" bind:value={s.internal_port_start} /></label>
        <label class="field"><span>{t('по')}</span><input type="number" bind:value={s.internal_port_end} /></label>
      </div>
      {#if app.status.lan_ip}<div class="small muted">{t('IP этого компьютера в сети')}: <code>{app.status.lan_ip}</code></div>{/if}
      <label class="chk"><input type="checkbox" bind:checked={s.local_no_auth} /> {t('Не спрашивать пароль на этом компьютере (127.0.0.1)')}</label>
      <div class="small muted">{keys.length ? t('Роутер требует API-ключ (есть ключи во вкладке «Безопасность»).') : t('API-ключей нет — роутер OpenAI доступен без токена. Создайте ключ во вкладке «Безопасность», и роутер начнёт его требовать.')}</div>
      <label class="chk"><input type="checkbox" bind:checked={s.log_request_bodies} /> {t('Логировать тела запросов (для отладки)')}</label>
      <div class="grid g3">
        <label class="field"><span>{t('Разрешённые подсети (CIDR)')}</span><textarea rows="3" value={asText(s.allowed_subnets)} oninput={(e) => (s.allowed_subnets = e.currentTarget.value)} placeholder="192.168.1.0/24"></textarea><span class="hint">{t('Пусто — любые')}</span></label>
        <label class="field"><span>{t('Доверенные прокси')}</span><textarea rows="3" value={asText(s.trusted_proxies)} oninput={(e) => (s.trusted_proxies = e.currentTarget.value)} placeholder="127.0.0.1"></textarea><span class="hint">{t('Их X-Forwarded-For считается адресом клиента')}</span></label>
        <label class="field"><span>{t('Доп. CORS origins')}</span><textarea rows="3" value={asText(s.cors_origins)} oninput={(e) => (s.cors_origins = e.currentTarget.value)} placeholder="http://192.168.1.10:3000"></textarea></label>
      </div>
    </div>
  {:else if tab === 'security'}
    <div class="grid g2">
      <div class="card col">
        <h3 style="margin:0">{t('Пароль администратора')}</h3>
        <div class="small muted">{app.auth.has_password ? t('Пароль задан. Можно сменить.') : t('Пароль не задан — удалённый вход невозможен.')}</div>
        <input type="password" autocomplete="new-password" placeholder={t('Новый пароль (мин. 8 символов)')} bind:value={pw1} />
        <input type="password" autocomplete="new-password" placeholder={t('Повторите')} bind:value={pw2} />
        <div class="row">
          <label class="field" style="flex:1"><span>{t('Время жизни access-токена, мин')}</span><input type="number" value={s.access_ttl_secs / 60} onchange={(e) => (s.access_ttl_secs = +e.currentTarget.value * 60)} /></label>
          <label class="field" style="flex:1"><span>{t('Сессия, дней')}</span><input type="number" bind:value={s.refresh_ttl_days} /></label>
        </div>
        <button class="primary" disabled={!pw1} onclick={setPw}>{t('Установить пароль')}</button>
      </div>
      <div class="card col">
        <h3 style="margin:0">{t('Активные сессии')}</h3>
        {#each sessions as x}
          <div class="row small"><span class="ell" style="flex:1" title={x.device}>{x.device || '—'}</span><span class="dim">{x.ip}</span><span class="dim">{date(x.last_seen)}</span><button class="ghost sm danger" onclick={() => delSession(x)}>✕</button></div>
        {/each}
        {#if !sessions.length}<div class="dim small">{t('Нет')}</div>{/if}
      </div>
    </div>
    <div class="card" style="margin-top:12px">
      <div class="row"><h3 style="margin:0">{t('API-ключи')}</h3><span class="sp"></span>
        <button class="primary sm" onclick={() => (keyForm = { name: '', scopes: '*', can_load: true, can_manage: false, expires: '' })}>+ {t('Ключ')}</button></div>
      <div class="small muted" style="margin:6px 0">{t('Для клиентов роутера (Open WebUI, IDE, скрипты): Authorization: Bearer lll_… Можно ограничить алиасы (через запятую, * — все), запретить загрузку моделей.')}</div>
      <table>
        <thead><tr><th>{t('Имя')}</th><th>{t('Префикс')}</th><th>{t('Алиасы')}</th><th>{t('Права')}</th><th>{t('Использован')}</th><th>{t('Истекает')}</th><th></th></tr></thead>
        <tbody>
          {#each keys as k}
            <tr>
              <td>{k.name}</td><td><code>{k.prefix}…</code></td><td class="small">{k.scopes.join(', ')}</td>
              <td class="small">{k.can_load ? t('загрузка') : ''} {k.can_manage ? t('управление') : ''}</td>
              <td class="small">{date(k.last_used)}</td><td class="small">{k.expires_at ? date(k.expires_at) : '∞'}</td>
              <td class="row nw"><button class="sm ghost" onclick={() => (keyForm = { ...k, scopes: k.scopes.join(', '), expires: k.expires_at ? new Date(k.expires_at * 1000).toISOString().slice(0, 10) : '' })}>✎</button><button class="sm ghost danger" onclick={() => delKey(k)}>✕</button></td>
            </tr>
          {/each}
        </tbody>
      </table>
      {#if !keys.length}<div class="empty">{t('Ключей нет')}</div>{/if}
    </div>
    <div class="card" style="margin-top:12px">
      <h3 style="margin-top:0">{t('Журнал безопасности')}</h3>
      <div class="scroll" style="max-height:300px">
        <table><tbody>
          {#each seclog as r}<tr><td class="small">{date(r.ts)}</td><td class="small"><span class="badge {r.event.includes('fail') || r.event.includes('denied') ? 'bad' : ''}">{r.event}</span></td><td class="small">{r.ip}</td><td class="small muted">{r.detail}</td></tr>{/each}
        </tbody></table>
        {#if !seclog.length}<div class="dim small">{t('Пусто')}</div>{/if}
      </div>
    </div>
  {:else if tab === 'models'}
    <div class="card grid g2">
      <label class="field"><span>{t('Выгружать при простое (TTL), сек')}</span><input type="number" bind:value={s.default_ttl} /><span class="hint">0 — {t('никогда; у пресета можно переопределить')}</span></label>
      <label class="field"><span>{t('Максимум одновременных моделей')}</span><input type="number" min="1" bind:value={s.max_instances} /></label>
      <label class="field"><span>{t('Таймаут загрузки модели, сек')}</span><input type="number" bind:value={s.load_timeout_secs} /></label>
      <label class="field"><span>{t('Запас VRAM, МиБ')}</span><input type="number" bind:value={s.vram_margin_mib} /></label>
      <label class="field"><span>{t('Накладные CUDA/драйвера на GPU, МиБ')}</span><input type="number" bind:value={s.gpu_overhead_mib} /></label>
      <label class="field"><span>{t('Интервал мониторинга, мс')}</span><input type="number" bind:value={s.poll_interval_ms} /></label>
      <label class="field"><span>{t('Хранить статистику токенов, дней')}</span><input type="number" min="0" bind:value={s.usage_retention_days} /><span class="hint">0 — {t('бессрочно')}</span></label>
      <label class="chk"><input type="checkbox" bind:checked={s.swap_mode} /> {t('Режим «одна модель»: при запуске выгружать остальные')}</label>
      <label class="chk"><input type="checkbox" bind:checked={s.auto_restart} /> {t('Перезапускать упавшие инстансы')}</label>
      <label class="field"><span>{t('Попыток перезапуска')}</span><input type="number" bind:value={s.max_restart_attempts} /></label>
      <label class="field"><span>{t('Биты на элемент для нестандартных типов KV-кэша')}</span><textarea rows="3" bind:value={cacheText} placeholder="turbo3=3.5"></textarea><span class="hint">{t('Для форков с собственными типами (-ctk/-ctv), чтобы калькулятор памяти считал верно')}</span></label>
    </div>
  {:else if tab === 'downloads'}
    <div class="card grid g2">
      <label class="field"><span>{t('Папка для моделей')}</span><input bind:value={s.download_dir} /></label>
      <label class="field"><span>{t('Папка для сборок llama.cpp')}</span><input bind:value={s.backends_dir} /></label>
      <label class="field"><span>{t('HF-токен (для gated-моделей)')}</span><input type="password" bind:value={s.hf_token} placeholder="hf_…" /></label>
      <label class="field"><span>{t('HF endpoint (зеркало)')}</span><input bind:value={s.hf_endpoint} /></label>
      <label class="field"><span>{t('GitHub-токен (лимиты API)')}</span><input type="password" bind:value={s.github_token} /></label>
      <label class="field"><span>{t('Параллельных загрузок')}</span><input type="number" min="1" bind:value={s.max_parallel_downloads} /></label>
      <label class="field"><span>{t('Ограничение скорости, КБ/с')}</span><input type="number" bind:value={s.download_speed_limit_kbps} /><span class="hint">0 — {t('без ограничений')}</span></label>
    </div>
  {:else}
    <div class="grid g2">
      <div class="card col">
        <h3 style="margin:0">{t('Резервная копия')}</h3>
        <div class="small muted">{t('Сборки, модели (метаданные), пресеты, алиасы, ключи, настройки, история бенчмарков. Файлы моделей не включаются.')}</div>
        <div class="row">
          <button class="primary" onclick={() => download('/api/backup', `llama-launcher-backup-${new Date().toISOString().slice(0, 10)}.zip`).catch((e) => app.err(e))}>⇩ {t('Скачать бэкап')}</button>
          <button onclick={() => restoreEl?.click()}>{t('Восстановить…')}</button>
          <input type="file" accept=".zip" hidden bind:this={restoreEl} onchange={doRestore} />
        </div>
      </div>
      <div class="card">
        <h3 style="margin-top:0">{t('О программе')}</h3>
        <div class="kv small">
          <span>{t('Версия')}</span><span>{app.status.version}</span>
          <span>{t('Данные')}</span><code style="word-break:break-all">{app.status.data_dir}</code>
          <span>UI</span><span>{app.status.ui_bind}:{app.status.ui_port}</span>
          <span>{t('Роутер')}</span><span>{app.status.router_bind}:{app.status.router_port}</span>
        </div>
      </div>
    </div>
  {/if}
{/if}

{#if keyForm}
  <Modal title={keyForm.id ? t('Ключ') : t('Новый API-ключ')} width="480px" onclose={() => (keyForm = null)}>
    <div class="col">
      <label class="field"><span>{t('Имя')}</span><input bind:value={keyForm.name} placeholder="Open WebUI" /></label>
      <label class="field"><span>{t('Разрешённые алиасы')}</span><input bind:value={keyForm.scopes} placeholder="*" />
        <span class="hint">{t('Доступные')}: {aliases.map((a) => a.name).join(', ') || '—'}</span></label>
      <label class="chk"><input type="checkbox" bind:checked={keyForm.can_load} /> {t('Может загружать модели (иначе только уже запущенные)')}</label>
      <label class="chk"><input type="checkbox" bind:checked={keyForm.can_manage} /> {t('Может управлять лаунчером через API')}</label>
      <label class="field"><span>{t('Истекает')}</span><input type="date" bind:value={keyForm.expires} /></label>
    </div>
    {#snippet footer()}
      <button onclick={() => (keyForm = null)}>{t('Отмена')}</button>
      <button class="primary" disabled={!keyForm.name} onclick={saveKey}>{t('Сохранить')}</button>
    {/snippet}
  </Modal>
{/if}

{#if newSecret}
  <Modal title={t('Ключ создан')} width="560px" onclose={() => (newSecret = '')}>
    <div class="alert warn">{t('Скопируйте ключ сейчас — повторно он показан не будет.')}</div>
    <div class="row" style="margin-top:10px"><pre style="flex:1">{newSecret}</pre><button onclick={() => { copy(newSecret); app.toast('success', t('Скопировано')) }}>📋</button></div>
  </Modal>
{/if}
