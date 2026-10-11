<script lang="ts">
  import { onMount } from 'svelte'
  import { app } from './lib/state.svelte'
  import { t, lang, setLang } from './lib/i18n.svelte'
  import { bytes } from './lib/fmt'
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
  import Chat from './pages/Chat.svelte'
  import Settings from './pages/Settings.svelte'

  const nav = [
    ['/', '◉', 'Дашборд'],
    ['/presets', '▶', 'Пресеты'],
    ['/models', '◫', 'Модели'],
    ['/hf', '⇩', 'Hugging Face'],
    ['/backends', '⚙', 'Сборки llama.cpp'],
    ['/aliases', '⇄', 'Алиасы и роутер'],
    ['/instances', '☰', 'Инстансы и логи'],
    ['/bench', '⏱', 'Бенчмарки'],
    ['/chat', '✎', 'Чат'],
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

  function isOn(p: string) {
    return p === '/' ? path === '/' : path.startsWith(p)
  }
</script>

{#if !app.ready}
  <div class="empty"><span class="spin"></span></div>
{:else if app.needLogin}
  <Login />
{:else}
  <div class="shell">
    <aside>
      <div class="logo">🦙 <b>Llama Launcher</b></div>
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
        <div class="row" style="margin-top:8px">
          <span class="dot" class:ready={app.sseOk} title={app.sseOk ? t('Подключено') : t('Нет связи')}></span>
          <span class="sp"></span>
          <button class="ghost sm" onclick={() => setLang(lang.v === 'ru' ? 'en' : 'ru')}>{lang.v === 'ru' ? 'EN' : 'RU'}</button>
          <button class="ghost sm" onclick={() => (theme = theme === 'dark' ? 'light' : 'dark')}>{theme === 'dark' ? '☀' : '☾'}</button>
          {#if !app.auth.local}<button class="ghost sm" title={t('Выйти')} onclick={() => app.logout()}>⏻</button>{/if}
        </div>
      </div>
    </aside>
    <main>
      {#key path}
        {#if path === '/'}<Dashboard />
        {:else if seg[0] === 'presets' && seg[1]}<PresetEditor id={seg[1]} />
        {:else if seg[0] === 'presets'}<Presets />
        {:else if seg[0] === 'models'}<Models />
        {:else if seg[0] === 'hf'}<Hf />
        {:else if seg[0] === 'backends'}<Backends />
        {:else if seg[0] === 'aliases'}<Aliases />
        {:else if seg[0] === 'instances'}<Instances />
        {:else if seg[0] === 'bench'}<Bench />
        {:else if seg[0] === 'chat'}<Chat />
        {:else if seg[0] === 'settings'}<Settings />
        {:else}<div class="empty">404</div>
        {/if}
      {/key}
    </main>
  </div>
{/if}
<Toasts />
<Dialogs />

<style>
  .shell { display: flex; height: 100%; }
  aside { width: 228px; flex: none; background: var(--bg2); border-right: 1px solid var(--line); display: flex; flex-direction: column; }
  .logo { padding: 16px 16px 12px; font-size: 15px; }
  nav { display: flex; flex-direction: column; padding: 4px 8px; gap: 1px; flex: 1; overflow: auto; }
  nav a { color: var(--fg2); padding: 7px 10px; border-radius: 7px; display: flex; align-items: center; gap: 10px; }
  nav a:hover { background: var(--bg3); text-decoration: none; color: var(--fg); }
  nav a.on { background: var(--bg4); color: var(--fg); }
  .ic { width: 16px; text-align: center; opacity: 0.8; }
  .cnt { margin-left: auto; }
  .foot { padding: 12px 14px; border-top: 1px solid var(--line); }
  .sys { display: flex; flex-direction: column; gap: 3px; }
  .bar.thin { height: 4px; margin-bottom: 4px; }
  main { flex: 1; overflow: auto; padding: 22px 26px 40px; min-width: 0; }
  @media (max-width: 760px) {
    .shell { flex-direction: column; }
    aside { width: 100%; height: auto; }
    nav { flex-direction: row; flex-wrap: wrap; }
    .foot { display: none; }
    main { padding: 14px; }
  }
</style>
