<script lang="ts">
  import { onMount } from 'svelte'
  import { app } from './lib/state.svelte'
  import { t, lang, setLang } from './lib/i18n.svelte'
  import { bytes } from './lib/fmt'
  import { api } from './lib/api'
  import { pwa } from './lib/pwa.svelte'
  import Toasts from './components/Toasts.svelte'
  import Dialogs from './components/Dialogs.svelte'
  import Login from './pages/Login.svelte'
  import Dashboard from './pages/Dashboard.svelte'
  import Presets from './pages/Presets.svelte'
  import PresetEditor from './pages/PresetEditor.svelte'
  import Models from './pages/Models.svelte'
  import Hf from './pages/Hf.svelte'
  import Backends from './pages/Backends.svelte'
  import Aliases from './pages/Aliases.svelte'
  import Instances from './pages/Instances.svelte'
  import Bench from './pages/Bench.svelte'
  import Analytics from './pages/Analytics.svelte'
  import Chat from './pages/Chat.svelte'
  import Settings from './pages/Settings.svelte'

  const nav = [
    ['/', '◉', 'Дашборд'],
    ['/presets', '▶', 'Пресеты'],
    ['/models', '◆', 'Модели'],
    ['/hf', '⇩', 'Hugging Face'],
    ['/backends', '⚙', 'Сборки llama.cpp'],
    ['/aliases', '⇄', 'Алиасы и роутер'],
    ['/instances', '≡', 'Инстансы и логи'],
    ['/analytics', '∑', 'Аналитика'],
    ['/bench', '⏱', 'Бенчмарки'],
    ['/chat', '✉', 'Чат'],
    ['/settings', '⚒', 'Настройки'],
  ]

  let theme = $state(localStorage.getItem('lll_theme') || 'dark')
  $effect(() => {
    document.documentElement.dataset.theme = theme
    localStorage.setItem('lll_theme', theme)
  })

  const path = $derived(app.route.split('?')[0])
  const seg = $derived(path.split('/').filter(Boolean))
  const alive = $derived(Object.values(app.instances).filter((i: any) => ['ready', 'loading', 'starting'].includes(i.state)))
  const activeJobs = $derived(Object.values(app.jobs).filter((j: any) => j.status === 'running').length + Object.values(app.downloads).filter((d: any) => d.status === 'running').length)

  onMount(() => app.boot())

  // phone layout: bottom tab bar with the most used pages, the rest in the «More» sheet
  const tabs = ['/', '/chat', '/presets', '/instances']
  const short: Record<string, string> = { '/instances': 'Инстансы' }
  let more = $state(false)
  const title = $derived(nav.find(([p]) => p !== '/' && path.startsWith(p))?.[2] ?? (path === '/' ? 'Дашборд' : ''))
  $effect(() => {
    path
    more = false
  })
  async function install() {
    if (await pwa.install()) app.toast('success', t('Приложение установлено'))
  }
  async function retry() {
    app.ready = false
    await app.boot()
  }

  function isOn(p: string) {
    return p === '/' ? path === '/' : path.startsWith(p)
  }
</script>

{#if !app.ready}
  <div class="empty"><span class="spin"></span></div>
{:else if app.offline}
  <div class="offline">
    <div style="font-size:48px">🦙</div>
    <h2>{t('Нет связи с лаунчером')}</h2>
    <div class="muted small">{t('Компьютер выключен, спит или телефон не в той же сети. Проверьте, что лаунчер запущен и доступен по сети.')}</div>
    <button class="primary" onclick={retry}>↻ {t('Повторить')}</button>
  </div>
{:else if app.needLogin}
  <Login />
{:else}
  <div class="shell">
    <aside>
      <div class="logo">🦙 <b>Llama Launcher</b>{#if app.auth.version}<span class="ver" title={t('Версия')}>v{app.auth.version}</span>{/if}</div>
      {#if app.update?.available}<a class="upd" href="#/settings?tab=update">⬆ {t('Доступна версия')} v{app.update.latest}</a>{/if}
      <nav>
        {#each nav as [p, ic, label]}
          <a href={'#' + p} class:on={isOn(p)}>
            <span class="ic">{ic}</span><span>{t(label)}</span>
            {#if p === '/instances' && alive.length}<span class="badge ok cnt">{alive.length}</span>{/if}
            {#if p === '/hf' && activeJobs}<span class="badge info cnt">{activeJobs}</span>{/if}
          </a>
        {/each}
      </nav>
      <div class="foot">
        <div class="sys small">
          {#each app.system.gpus || [] as g}
            <div class="ell" title={g.name}>
              <span class="dim">GPU{g.index}</span>
              {bytes(g.used_bytes, 1)} / {bytes(g.total_bytes, 0)}
              {#if g.util != null}<span class="dim">· {Math.round(g.util)}%</span>{/if}
            </div>
            <div class="bar thin"><div style="width:{(g.used_bytes / Math.max(g.total_bytes, 1)) * 100}%;background:var(--acc)"></div></div>
          {/each}
          {#if app.system.ram_total}
            <div><span class="dim">RAM</span> {bytes(app.system.ram_used)} / {bytes(app.system.ram_total, 0)}</div>
            <div class="bar thin"><div style="width:{(app.system.ram_used / app.system.ram_total) * 100}%;background:var(--ok)"></div></div>
          {/if}
        </div>
        {#if pwa.prompt && !pwa.standalone}<button class="sm" style="margin-top:8px;width:100%;justify-content:center" onclick={install}>📲 {t('Установить приложение')}</button>{/if}
        <div class="row" style="margin-top:8px">
          <span class="dot" class:ready={app.sseOk} title={app.sseOk ? t('Подключено') : t('Нет связи')}></span>
          <span class="sp"></span>
          <button class="ghost sm" onclick={() => setLang(lang.v === 'ru' ? 'en' : 'ru')}>{lang.v === 'ru' ? 'EN' : 'RU'}</button>
          <button class="ghost sm" onclick={() => (theme = theme === 'dark' ? 'light' : 'dark')}>{theme === 'dark' ? '☀' : '☾'}</button>
          {#if !app.auth.local}<button class="ghost sm" title={t('Выйти')} onclick={() => app.logout()}>⏻</button>{/if}
        </div>
      </div>
    </aside>
    <header class="mtop">
      <span class="mlogo">🦙</span>
      <b class="ell">{t(title)}</b>
      <span class="sp"></span>
      {#if app.game.active}<span title={t('Игровой режим')}>🎮</span>{/if}
      {#if alive.length}<span class="badge ok">{alive.length} ●</span>{/if}
      <span class="dot" class:ready={app.sseOk} title={app.sseOk ? t('Подключено') : t('Нет связи')}></span>
    </header>
    <main>
      {#if app.game.active}
        <div class="gamebar small">
          <span>🎮 <b>{t('Игровой режим')}</b>: {app.game.reason === 'manual' ? t('включён вручную') : app.game.reason} — {t('модели выгружены, автозагрузка приостановлена')}</span>
          <span class="sp"></span>
          <button class="sm" onclick={async () => { try { app.game = await api.post('/api/gamemode/deactivate') } catch (e) { app.err(e) } }}>{t('Выключить')}</button>
        </div>
      {/if}
      {#key path}
        {#if path === '/'}<Dashboard />
        {:else if seg[0] === 'presets' && seg[1]}<PresetEditor id={seg[1]} />
        {:else if seg[0] === 'presets'}<Presets />
        {:else if seg[0] === 'models'}<Models />
        {:else if seg[0] === 'hf'}<Hf />
        {:else if seg[0] === 'backends'}<Backends />
        {:else if seg[0] === 'aliases'}<Aliases />
        {:else if seg[0] === 'instances'}<Instances />
        {:else if seg[0] === 'analytics'}<Analytics />
        {:else if seg[0] === 'bench'}<Bench />
        {:else if seg[0] === 'chat'}<Chat />
        {:else if seg[0] === 'settings'}<Settings />
        {:else}<div class="empty">404</div>
        {/if}
      {/key}
    </main>
    <nav class="mtab">
      {#each nav.filter(([p]) => tabs.includes(p)) as [p, ic, label]}
        <a href={'#' + p} class:on={isOn(p) && !more}>
          <span class="tic">{ic}{#if p === '/instances' && alive.length}<i class="pip"></i>{/if}</span><span class="tl">{t(short[p] ?? label)}</span>
        </a>
      {/each}
      <button class:on={more || !tabs.some((p) => isOn(p))} onclick={() => (more = !more)}>
        <span class="tic">⋯{#if activeJobs}<i class="pip info"></i>{/if}</span><span class="tl">{t('Ещё')}</span>
      </button>
    </nav>
    {#if more}
      <button class="mscrim" aria-label="close" onclick={() => (more = false)}></button>
      <div class="sheet">
        <div class="grab"></div>
        <div class="mgrid">
          {#each nav.filter(([p]) => !tabs.includes(p)) as [p, ic, label]}
            <a href={'#' + p} class:on={isOn(p)}>
              <span class="tic">{ic}</span><span class="small">{t(label)}</span>
              {#if p === '/hf' && activeJobs}<span class="badge info cnt">{activeJobs}</span>{/if}
            </a>
          {/each}
        </div>
        <div class="sys small">
          {#each app.system.gpus || [] as g}
            <div class="row nw"><span class="dim">GPU{g.index}</span><span class="ell">{bytes(g.used_bytes, 1)} / {bytes(g.total_bytes, 0)}{#if g.util != null} · {Math.round(g.util)}%{/if}</span></div>
            <div class="bar thin"><div style="width:{(g.used_bytes / Math.max(g.total_bytes, 1)) * 100}%;background:var(--acc)"></div></div>
          {/each}
          {#if app.system.ram_total}
            <div class="row nw"><span class="dim">RAM</span><span>{bytes(app.system.ram_used)} / {bytes(app.system.ram_total, 0)}</span></div>
            <div class="bar thin"><div style="width:{(app.system.ram_used / app.system.ram_total) * 100}%;background:var(--ok)"></div></div>
          {/if}
        </div>
        {#if !pwa.standalone && !pwa.tauri}
          {#if pwa.prompt}
            <button class="primary" style="justify-content:center" onclick={install}>📲 {t('Установить приложение')}</button>
          {:else if pwa.ios}
            <div class="alert small">📲 {t('Чтобы установить приложение: «Поделиться» → «На экран „Домой“».')}</div>
          {:else if !pwa.secure}
            <div class="alert small">📲 {t('Полноценная установка приложения требует HTTPS (Настройки → Сеть и доступ). Ярлык можно добавить через меню браузера → «Добавить на главный экран».')}</div>
          {:else}
            <div class="alert small">📲 {t('Установить приложение: меню браузера → «Установить приложение» / «Добавить на главный экран».')}</div>
          {/if}
        {/if}
        <div class="row">
          <button onclick={() => setLang(lang.v === 'ru' ? 'en' : 'ru')}>🌐 {lang.v === 'ru' ? 'English' : 'Русский'}</button>
          <button onclick={() => (theme = theme === 'dark' ? 'light' : 'dark')}>{theme === 'dark' ? '☀ ' + t('Светлая') : '☾ ' + t('Тёмная')}</button>
          <span class="sp"></span>
          {#if !app.auth.local}<button class="danger" onclick={() => app.logout()}>⏻ {t('Выйти')}</button>{/if}
        </div>
        {#if app.update?.available}<a class="btn" style="justify-content:center" href="#/settings?tab=update">⬆ {t('Доступна версия')} v{app.update.latest}</a>{/if}
        {#if app.auth.version}<div class="tiny dim" style="text-align:center">Llama Launcher v{app.auth.version}</div>{/if}
      </div>
    {/if}
  </div>
{/if}
<Toasts />
<Dialogs />

<style>
  :global(:root) { --avail-h: calc(100dvh - 62px); }
  .shell { display: flex; height: 100%; }
  .mtop, .mtab { display: none; }
  .offline { height: 100%; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 10px; padding: 24px; text-align: center; }
  .offline h2 { margin: 6px 0 0; }
  aside { width: 228px; flex: none; background: var(--bg2); border-right: 1px solid var(--line); display: flex; flex-direction: column; }
  .logo { padding: 16px 16px 12px; font-size: 15px; }
  .upd { margin: -6px 16px 8px; font-size: 12px; padding: 3px 10px; border-radius: 99px; text-align: center; background: color-mix(in srgb, var(--ok) 20%, transparent); color: var(--ok); }
  .logo .ver { margin-left: 6px; font-size: 11px; color: var(--fg3); font-weight: 400; }
  aside nav { display: flex; flex-direction: column; padding: 4px 8px; gap: 1px; flex: 1; overflow: auto; }
  aside nav a { color: var(--fg2); padding: 7px 10px; border-radius: 7px; display: flex; align-items: center; gap: 10px; }
  aside nav a:hover { background: var(--bg3); text-decoration: none; color: var(--fg); }
  aside nav a.on { background: var(--bg4); color: var(--fg); }
  .ic { width: 16px; text-align: center; opacity: 0.8; }
  .cnt { margin-left: auto; }
  .foot { padding: 12px 14px; border-top: 1px solid var(--line); }
  .sys { display: flex; flex-direction: column; gap: 3px; }
  .bar.thin { height: 4px; margin-bottom: 4px; }
  main { flex: 1; overflow: auto; padding: 22px 26px 40px; min-width: 0; }
  @media (max-width: 760px) {
    :global(:root) { --tab-h: 58px; --top-h: 46px; --avail-h: calc(100dvh - var(--top-h) - var(--tab-h) - env(safe-area-inset-top) - env(safe-area-inset-bottom) - 20px); }
    .shell { flex-direction: column; height: 100dvh; }
    aside { display: none; }
    .mtop { display: flex; align-items: center; gap: 8px; flex: none; height: calc(var(--top-h) + env(safe-area-inset-top)); padding: env(safe-area-inset-top) 14px 0; background: var(--bg2); border-bottom: 1px solid var(--line); font-size: 15px; }
    .mlogo { font-size: 18px; }
    main { padding: 10px 12px calc(var(--tab-h) + env(safe-area-inset-bottom) + 10px); }
    .mtab { display: flex; position: fixed; z-index: 45; left: 0; right: 0; bottom: 0; height: calc(var(--tab-h) + env(safe-area-inset-bottom)); padding: 0 4px env(safe-area-inset-bottom); background: var(--bg2); border-top: 1px solid var(--line); }
    .mtab a, .mtab button { flex: 1; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 2px; color: var(--fg3); background: none; border: 0; border-radius: 0; padding: 4px 0; font-size: 11px; text-decoration: none; min-width: 0; }
    .mtab .on { color: var(--acc); }
    .tic { font-size: 19px; line-height: 1.1; position: relative; }
    .tl { max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .pip { position: absolute; top: 0; right: -7px; width: 7px; height: 7px; border-radius: 50%; background: var(--ok); }
    .pip.info { background: var(--info); }
    .mscrim { position: fixed; inset: 0; z-index: 43; background: rgba(0, 0, 0, .45); border: 0; border-radius: 0; padding: 0; }
    .sheet { position: fixed; z-index: 44; left: 0; right: 0; bottom: calc(var(--tab-h) + env(safe-area-inset-bottom)); max-height: 75dvh; overflow: auto; background: var(--bg2); border-top: 1px solid var(--line); border-radius: 16px 16px 0 0; padding: 8px 14px 14px; display: flex; flex-direction: column; gap: 12px; animation: up .18s ease-out; }
    @keyframes up { from { transform: translateY(30px); opacity: 0; } }
    .grab { width: 38px; height: 4px; border-radius: 4px; background: var(--bg4); margin: 0 auto; }
    .mgrid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 8px; }
    .mgrid a { display: flex; flex-direction: column; align-items: center; gap: 4px; padding: 10px 4px; border-radius: 10px; background: var(--bg3); color: var(--fg2); text-align: center; position: relative; text-decoration: none; }
    .mgrid a.on { background: var(--bg4); color: var(--fg); }
    .mgrid .cnt { position: absolute; top: 4px; right: 4px; }
    .sys .row { justify-content: space-between; }
  }
</style>
