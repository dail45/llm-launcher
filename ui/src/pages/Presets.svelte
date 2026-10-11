<script lang="ts">
  import { onMount } from 'svelte'
  import { api } from '../lib/api'
  import { app } from '../lib/state.svelte'
  import { t } from '../lib/i18n.svelte'
  import { copy, bytes } from '../lib/fmt'
  import { confirmDlg } from '../lib/dialog.svelte'
  import { startPreset, stopPreset, stateCls, stateTxt, isAlive } from '../lib/actions'
  import Modal from '../components/Modal.svelte'

  let presets = $state<any[]>([])
  let models = $state<Record<string, any>>({})
  let backends = $state<any[]>([])
  let aliases = $state<any[]>([])
  let filter = $state('')
  let busy = $state<Record<string, boolean>>({})
  let showImport = $state(false)
  let impCmd = $state('')
  let impBackend = $state('')
  let impBusy = $state(false)
  let fileInput = $state<HTMLInputElement>()

  async function load() {
    try {
      const [p, m, b, a] = await Promise.all([api.get('/api/presets'), api.get('/api/models'), api.get('/api/backends'), api.get('/api/aliases')])
      presets = p.sort((x: any, y: any) => x.order - y.order || x.name.localeCompare(y.name))
      models = Object.fromEntries(m.map((x: any) => [x.id, x]))
      backends = b
      aliases = a
      impBackend = impBackend || b.find((x: any) => x.is_default)?.id || b[0]?.id || ''
    } catch (e) { app.err(e) }
  }
  onMount(load)

  const bname = (id: string) => backends.find((b) => b.id === id)?.name || '?'
  const aliasOf = (id: string) => aliases.filter((a) => a.target_type === 'preset' && a.target_id === id)
  const shown = $derived(presets.filter((p) => !filter || (p.name + ' ' + (models[p.model_id]?.file_name || '') + ' ' + p.group).toLowerCase().includes(filter.toLowerCase())))
  const groups = $derived([...new Set(shown.map((p) => p.group || ''))].sort())

  async function toggle(p: any) {
    busy[p.id] = true
    const i = app.instances[p.id]
    if (i && isAlive(i.state)) await stopPreset(p.id)
    else await startPreset(p.id, p.name)
    busy[p.id] = false
  }
  async function clone(p: any) {
    try { const c = await api.post(`/api/presets/${p.id}/clone`); app.go('/presets/' + c.id) } catch (e) { app.err(e) }
  }
  async function del(p: any) {
    if (!(await confirmDlg(t('Удалить пресет?'), p.name, { danger: true, ok: t('Удалить') }))) return
    try { await api.del('/api/presets/' + p.id); load() } catch (e) { app.err(e) }
  }
  async function copyCmd(p: any) {
    try { const c = await api.get(`/api/presets/${p.id}/command`); await copy(c.cmd); app.toast('success', t('Команда скопирована')) } catch (e) { app.err(e) }
  }
  async function doImport() {
    impBusy = true
    try {
      const r = await api.post('/api/presets/import-command', { backend_id: impBackend, command: impCmd })
      sessionStorage.setItem('lll_draft', JSON.stringify({ backend_id: impBackend, ...r }))
      showImport = false
      app.go('/presets/new')
    } catch (e) { app.err(e) } finally { impBusy = false }
  }
  async function exportAll() {
    try {
      const d = await api.get('/api/presets/export')
      const a = document.createElement('a')
      a.href = URL.createObjectURL(new Blob([JSON.stringify(d, null, 2)], { type: 'application/json' }))
      a.download = 'llama-launcher-presets.json'
      a.click()
    } catch (e) { app.err(e) }
  }
  async function importFile(e: Event) {
    const f = (e.target as HTMLInputElement).files?.[0]
    if (!f) return
    try {
      const r = await api.post('/api/presets/import', JSON.parse(await f.text()))
      app.toast('success', t('Импортировано пресетов: {n}', { n: r.imported ?? r.count ?? '' }), (r.warnings || []).join('\n'))
      load()
    } catch (e) { app.err(e) }
    ;(e.target as HTMLInputElement).value = ''
  }
</script>

<div class="row" style="margin-bottom:16px">
  <h1 style="margin:0">{t('Пресеты')}</h1>
  <span class="sp"></span>
  <input placeholder={t('Поиск…')} bind:value={filter} style="width:200px" />
  <button onclick={() => (showImport = true)}>{t('Импорт команды')}</button>
  <button onclick={exportAll}>{t('Экспорт')}</button>
  <button onclick={() => fileInput?.click()}>{t('Импорт файла')}</button>
  <input type="file" accept=".json" bind:this={fileInput} onchange={importFile} hidden />
  <button class="primary" onclick={() => app.go('/presets/new')}>+ {t('Новый пресет')}</button>
</div>

{#if presets.length === 0}
  <div class="card empty">{t('Пресет — это модель + сборка llama.cpp + параметры запуска. Создайте первый или импортируйте готовую команду llama-server.')}</div>
{/if}

{#each groups as g}
  {#if g}<h2>{g}</h2>{/if}
  <div class="cards">
    {#each shown.filter((p) => (p.group || '') === g) as p (p.id)}
      {@const i = app.instances[p.id]}
      {@const m = models[p.model_id]}
      <div class="card pc" style={p.color ? `border-top:3px solid ${p.color}` : ''}>
        <div class="row nw">
          <span class="dot {i?.state || ''}"></span>
          <a href="#/presets/{p.id}" class="name ell">{p.icon || ''} {p.name}</a>
          <span class="sp"></span>
          {#if i}<span class="badge {stateCls(i.state)}">{stateTxt(i.state)}</span>{/if}
        </div>
        <div class="small muted ell" title={m?.paths?.[0]}>{m ? m.file_name : t('модель не найдена')} {m ? '· ' + bytes(m.size) : ''}</div>
        <div class="row small" style="margin-top:6px">
          <span class="badge">{bname(p.backend_id)}</span>
          {#each aliasOf(p.id) as a}<code class="badge acc" title={a.is_primary ? t('основной алиас') : ''}>{a.name}{a.is_primary ? ' ★' : ''}</code>{/each}
          {#if p.pinned}<span class="badge">📌</span>{/if}
          {#if p.autostart}<span class="badge info">{t('автозапуск')}</span>{/if}
          {#if p.args['--ctx-size']}<span class="badge">ctx {p.args['--ctx-size']}</span>{/if}
        </div>
        <div class="row" style="margin-top:10px">
          <button class="sm" class:primary={!i || !isAlive(i.state)} disabled={busy[p.id]} onclick={() => toggle(p)}>
            {#if busy[p.id]}<span class="spin"></span>{:else if i && isAlive(i.state)}■ {t('Стоп')}{:else}▶ {t('Запустить')}{/if}
          </button>
          <button class="sm" onclick={() => app.go('/presets/' + p.id)}>{t('Изменить')}</button>
          <span class="sp"></span>
          <button class="sm ghost" title={t('Копировать команду')} onclick={() => copyCmd(p)}>📋</button>
          <button class="sm ghost" title={t('Клонировать')} onclick={() => clone(p)}>📑</button>
          <button class="sm ghost danger" title={t('Удалить')} onclick={() => del(p)}>✕</button>
        </div>
      </div>
    {/each}
  </div>
{/each}

{#if showImport}
  <Modal title={t('Импорт команды llama-server')} onclose={() => (showImport = false)} width="720px">
    <div class="col">
      <div class="small muted">{t('Вставьте строку запуска (bat/ps1/sh). Параметры распознаются по --help выбранной сборки, модель ищется среди известных файлов.')}</div>
      <label class="field"><span>{t('Сборка')}</span>
        <select bind:value={impBackend}>{#each backends as b}<option value={b.id}>{b.name}</option>{/each}</select>
      </label>
      <textarea rows="8" bind:value={impCmd} placeholder="llama-server.exe -m D:\models\qwen.gguf -c 32768 -ngl 99 -fa on --jinja"></textarea>
    </div>
    {#snippet footer()}
      <button onclick={() => (showImport = false)}>{t('Отмена')}</button>
      <button class="primary" disabled={!impCmd.trim() || !impBackend || impBusy} onclick={doImport}>{t('Распознать')}</button>
    {/snippet}
  </Modal>
{/if}

<style>
  .pc { display: flex; flex-direction: column; gap: 4px; }
  .name { color: var(--fg); font-weight: 600; font-size: 15px; }
</style>
