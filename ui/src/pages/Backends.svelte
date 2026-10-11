<script lang="ts">
  import { onMount } from 'svelte'
  import { api, q } from '../lib/api'
  import { app } from '../lib/state.svelte'
  import { t } from '../lib/i18n.svelte'
  import { bytes, date } from '../lib/fmt'
  import { confirmDlg, promptDlg } from '../lib/dialog.svelte'
  import Modal from '../components/Modal.svelte'

  let tab = $state('local')
  let backends = $state<any[]>([])
  let sources = $state<any[]>([])
  let updates = $state<any[]>([])
  let busy = $state<Record<string, boolean>>({})
  let form = $state<any>(null)
  let src = $state<any>(null)
  let releases = $state<any[]>([])
  let relLoading = $state(false)
  let onlyWin = $state(true)
  let showAll = $state(false)

  const jobs = $derived(Object.values(app.jobs).filter((j: any) => j.kind === 'backend_install').sort((a: any, b: any) => b.created_at - a.created_at).slice(0, 5) as any[])

  async function load() {
    try {
      ;[backends, sources] = await Promise.all([api.get('/api/backends'), api.get('/api/github/sources')])
    } catch (e) { app.err(e) }
  }
  onMount(() => {
    load()
    api.get('/api/github/updates').then((u) => (updates = u)).catch(() => {})
  })
  // reload backends when an install job finishes
  let doneJobs = new Set<string>()
  $effect(() => {
    for (const j of Object.values(app.jobs) as any[]) if (j.status === 'done' && !doneJobs.has(j.id)) { doneJobs.add(j.id); load() }
  })

  async function save() {
    busy.form = true
    try {
      const body = { name: form.name, dir: form.dir, kind: form.kind || null, color: form.color || null, notes: form.notes || '' }
      if (form.id) await api.put('/api/backends/' + form.id, body)
      else await api.post('/api/backends', body)
      form = null
      load()
    } catch (e) { app.err(e) } finally { busy.form = false }
  }
  async function rescan(b: any) {
    busy[b.id] = true
    try { await api.post(`/api/backends/${b.id}/rescan`); app.toast('success', t('Сборка просканирована'), b.name); load() } catch (e) { app.err(e) } finally { busy[b.id] = false }
  }
  async function setDefault(b: any) {
    try { await api.post(`/api/backends/${b.id}/default`); load() } catch (e) { app.err(e) }
  }
  async function del(b: any) {
    if (!(await confirmDlg(t('Удалить сборку?'), b.name + (b.managed ? '\n' + t('Файлы, установленные лаунчером, тоже будут удалены.') : ''), { danger: true, ok: t('Удалить') }))) return
    try { await api.del('/api/backends/' + b.id + q({ delete_files: b.managed })); load() } catch (e) { app.err(e) }
  }
  async function openSrc(s: any) {
    src = s
    releases = []
    relLoading = true
    try { releases = await api.get('/api/github/releases' + q({ repo: s.repo })) } catch (e) { app.err(e) } finally { relLoading = false }
  }
  async function addSrc() {
    const repo = await promptDlg(t('Репозиторий GitHub'), '', t('Формат owner/repo, например ggml-org/llama.cpp'))
    if (!repo) return
    try { await api.post('/api/github/sources', { repo, name: '', asset_pattern: '' }); load() } catch (e) { app.err(e) }
  }
  async function delSrc(s: any) {
    if (!(await confirmDlg(t('Убрать источник?'), s.repo))) return
    try { await api.del('/api/github/sources/' + s.id); load() } catch (e) { app.err(e) }
  }
  function cudaVer(name: string) {
    return name.match(/cu(?:da)?[-_]?(\d+\.\d+)/i)?.[1]
  }
  async function install(rel: any, a: any) {
    const extra: string[] = []
    if (a.kind === 'cuda' && !a.is_cudart) {
      const v = cudaVer(a.name)
      const rt = rel.assets.find((x: any) => x.is_cudart && (!v || cudaVer(x.name) === v))
      if (rt && (await confirmDlg(t('Добавить CUDA runtime?'), t('Для CUDA-сборки нужны DLL рантайма: {f}. Скачать вместе (рекомендуется, если CUDA Toolkit не установлен)?', { f: rt.name }), { ok: t('Да') }))) extra.push(rt.url)
    }
    const name = await promptDlg(t('Название сборки'), `${src.name || src.repo} ${rel.tag} ${a.kind}`)
    if (name === null) return
    try {
      await api.post('/api/github/install', { repo: src.repo, tag: rel.tag, asset_name: a.name, asset_url: a.url, extra_urls: extra, name })
      app.toast('info', t('Установка запущена'), a.name)
      src = null
      tab = 'local'
    } catch (e) { app.err(e) }
  }
  const kinds = ['cuda', 'rocm', 'vulkan', 'cpu', 'sycl', 'other']
</script>

<div class="row" style="margin-bottom:12px">
  <h1 style="margin:0">{t('Сборки llama.cpp')}</h1>
  <span class="sp"></span>
  <button class="primary" onclick={() => (form = { name: '', dir: '', kind: '', color: '', notes: '' })}>+ {t('Добавить папку')}</button>
</div>

{#if updates.length}
  <div class="alert info" style="margin-bottom:12px">
    {#each updates as u}<div>{u.backend_name}: <b>{u.current}</b> → <b>{u.latest}</b> ({u.repo}) <button class="sm" onclick={() => { tab = 'github'; const s = sources.find((x) => x.repo === u.repo); if (s) openSrc(s) }}>{t('Обновить')}</button></div>{/each}
  </div>
{/if}

<div class="tabs">
  <button class:on={tab === 'local'} onclick={() => (tab = 'local')}>{t('Установленные')} ({backends.length})</button>
  <button class:on={tab === 'github'} onclick={() => (tab = 'github')}>{t('Скачать с GitHub')}</button>
</div>

{#each jobs.filter((j) => j.status === 'running' || j.status === 'failed') as j (j.id)}
  <div class="card" style="margin-bottom:10px">
    <div class="row small"><b>{j.title}</b><span class="sp"></span><span class="badge {j.status === 'failed' ? 'bad' : 'info'}">{j.status}</span></div>
    {#if j.progress != null}<div class="bar" style="margin-top:6px"><div style="width:{j.progress * 100}%;background:var(--acc)"></div></div>{/if}
    <div class="tiny muted">{j.message}</div>
  </div>
{/each}

{#if tab === 'local'}
  {#if backends.length === 0}
    <div class="card empty">{t('Укажите папку с llama-server.exe (например, распакованный релиз) или скачайте сборку с GitHub. Можно добавить несколько: CUDA, Vulkan, ROCm, форки.')}</div>
  {/if}
  <div class="cards">
    {#each backends as b (b.id)}
      <div class="card" style={b.color ? `border-top:3px solid ${b.color}` : ''}>
        <div class="row nw">
          <b class="ell">{b.name}</b>
          {#if b.is_default}<span class="badge acc">{t('по умолчанию')}</span>{/if}
          <span class="sp"></span>
          <span class="badge">{b.kind}</span>
        </div>
        <div class="kv small" style="margin-top:8px">
          <span>{t('Версия')}</span><span>{b.version || '—'} {b.commit ? `(${b.commit})` : ''}</span>
          {#if b.source_repo}<span>GitHub</span><span>{b.source_repo} {b.release_tag || ''}</span>{/if}
          <span>{t('Папка')}</span><code class="ell" title={b.dir}>{b.dir}</code>
          <span>{t('Устройства')}</span><span>{b.devices.length ? b.devices.map((d: any) => `${d.id}: ${d.name}`).join('; ') : '—'}</span>
          {#if b.features.some((f: string) => !f.startsWith('--'))}<span>{t('Фичи')}</span><span class="row" style="gap:4px">{#each b.features.filter((f: string) => !f.startsWith('--')) as f}<span class="badge info">{f}</span>{/each}</span>{/if}
          <span>llama-bench</span><span>{b.bench_bin ? '✓' : '—'}</span>
        </div>
        {#if b.scan_error}<div class="alert bad small" style="margin-top:8px">{b.scan_error}</div>{/if}
        {#if b.notes}<div class="small muted" style="margin-top:6px">{b.notes}</div>{/if}
        <div class="row" style="margin-top:10px">
          <button class="sm" disabled={busy[b.id]} onclick={() => rescan(b)}>{#if busy[b.id]}<span class="spin"></span>{/if}{t('Пересканировать')}</button>
          {#if !b.is_default}<button class="sm" onclick={() => setDefault(b)}>{t('По умолчанию')}</button>{/if}
          <button class="sm" onclick={() => (form = { ...b, kind: b.kind })}>{t('Изменить')}</button>
          <span class="sp"></span>
          <button class="sm ghost danger" onclick={() => del(b)}>✕</button>
        </div>
      </div>
    {/each}
  </div>
{:else}
  <div class="grid" style="grid-template-columns:260px minmax(0,1fr);align-items:start">
    <div class="card col" style="gap:4px">
      {#each sources as s}
        <div class="row nw src" class:on={src?.id === s.id}>
          <button class="ghost" style="flex:1;justify-content:flex-start;overflow:hidden" onclick={() => openSrc(s)}><span class="ell">{s.name || s.repo}</span></button>
          <button class="ghost sm" onclick={() => delSrc(s)}>✕</button>
        </div>
      {/each}
      <button class="sm" style="margin-top:6px" onclick={addSrc}>+ {t('Репозиторий')}</button>
    </div>
    <div>
      {#if !src}
        <div class="card empty">{t('Выберите источник слева. Официальный llama.cpp и форки (например, с turbo3 или bonsai) публикуют готовые Windows-сборки в релизах.')}</div>
      {:else}
        <div class="row" style="margin-bottom:10px">
          <b>{src.repo}</b>
          <a class="small" href="https://github.com/{src.repo}/releases" target="_blank">releases ↗</a>
          <span class="sp"></span>
          <label class="chk small"><input type="checkbox" bind:checked={onlyWin} /> {t('только Windows')}</label>
        </div>
        {#if relLoading}<div class="empty"><span class="spin"></span></div>{/if}
        {#if !relLoading && !releases.length}<div class="card empty">{t('Релизов с файлами не найдено. Сборку форка можно собрать самостоятельно и добавить папкой.')}</div>{/if}
        {#each releases.slice(0, showAll ? 30 : 5) as rel, i (rel.tag)}
          <details class="card" style="margin-bottom:8px" open={i === 0}>
            <summary class="row"><b>{rel.tag}</b> {#if rel.prerelease}<span class="badge warn">pre</span>{/if}<span class="dim small">{rel.name !== rel.tag ? rel.name : ''} · {rel.published_at?.slice(0, 10)}</span></summary>
            <table style="margin-top:6px">
              <tbody>
                {#each rel.assets.filter((a: any) => !onlyWin || a.windows) as a}
                  <tr>
                    <td class="small ell" style="max-width:420px">{a.name}</td>
                    <td><span class="badge">{a.kind}</span>{#if a.is_cudart}<span class="badge info">runtime</span>{/if}</td>
                    <td class="small">{bytes(a.size)}</td>
                    <td style="text-align:right">{#if !a.is_cudart}<button class="sm primary" onclick={() => install(rel, a)}>{t('Установить')}</button>{/if}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </details>
        {/each}
        {#if releases.length > 5 && !showAll}<button onclick={() => (showAll = true)}>{t('Показать больше')}</button>{/if}
      {/if}
    </div>
  </div>
{/if}

{#if form}
  <Modal title={form.id ? t('Изменить сборку') : t('Добавить сборку')} width="560px" onclose={() => (form = null)}>
    <div class="col">
      <label class="field"><span>{t('Название')}</span><input bind:value={form.name} placeholder="llama.cpp CUDA b6500" /></label>
      <label class="field"><span>{t('Папка с llama-server.exe')}</span><input bind:value={form.dir} placeholder="C:\llama.cpp\build\bin\Release" /></label>
      <div class="grid g2">
        <label class="field"><span>{t('Тип')}</span>
          <select bind:value={form.kind}><option value="">{t('определить автоматически')}</option>{#each kinds as k}<option value={k}>{k}</option>{/each}</select></label>
        <label class="field"><span>{t('Цвет')}</span><input type="color" bind:value={form.color} style="height:34px;padding:2px" /></label>
      </div>
      <label class="field"><span>{t('Заметки')}</span><textarea rows="2" bind:value={form.notes} style="font-family:inherit"></textarea></label>
    </div>
    {#snippet footer()}
      <button onclick={() => (form = null)}>{t('Отмена')}</button>
      <button class="primary" disabled={!form.name || !form.dir || busy.form} onclick={save}>{busy.form ? '…' : t('Сохранить')}</button>
    {/snippet}
  </Modal>
{/if}

<style>
  .src.on { background: var(--bg4); border-radius: 6px; }
  summary { cursor: pointer; list-style: none; }
</style>
