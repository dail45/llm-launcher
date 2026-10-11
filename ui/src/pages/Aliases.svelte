<script lang="ts">
  import { onMount } from 'svelte'
  import { api } from '../lib/api'
  import { app } from '../lib/state.svelte'
  import { t } from '../lib/i18n.svelte'
  import { copy, date } from '../lib/fmt'
  import { confirmDlg } from '../lib/dialog.svelte'
  import Modal from '../components/Modal.svelte'

  let tab = $state('aliases')
  let aliases = $state<any[]>([])
  let groups = $state<any[]>([])
  let presets = $state<any[]>([])
  let reqs = $state<any[]>([])
  let newA = $state<any>(null)
  let grp = $state<any>(null)
  let rfilter = $state('')
  let addPid = $state('')

  async function load() {
    try {
      ;[aliases, groups, presets] = await Promise.all([api.get('/api/aliases'), api.get('/api/alias-groups'), api.get('/api/presets')])
    } catch (e) { app.err(e) }
  }
  async function loadReqs() {
    try { reqs = await api.get('/api/router/requests?limit=500') } catch (e) { app.err(e) }
  }
  onMount(() => { load(); loadReqs() })

  const pname = (id: string) => presets.find((p) => p.id === id)?.name || '?'
  const gname = (id: string) => groups.find((g) => g.id === id)?.name || '?'
  const host = $derived(app.status.router_bind === '127.0.0.1' ? '127.0.0.1' : app.status.lan_ip || location.hostname)
  const base = $derived(`http://${host}:${app.status.router_port}`)
  const shownReqs = $derived(reqs.filter((r) => !rfilter || JSON.stringify(r).toLowerCase().includes(rfilter.toLowerCase())))
  const example = $derived(`curl ${base}/v1/chat/completions -H "Authorization: Bearer lll_..." -H "Content-Type: application/json" -d "{\\"model\\":\\"${aliases[0]?.name || 'my-model'}\\",\\"stream\\":true,\\"messages\\":[{\\"role\\":\\"user\\",\\"content\\":\\"Привет\\"}]}"`)

  async function saveAlias() {
    try {
      await api.post('/api/aliases', { name: newA.name.trim(), target_type: newA.target_type, target_id: newA.target_id, is_primary: newA.is_primary })
      newA = null
      load()
    } catch (e) { app.err(e) }
  }
  async function delAlias(a: any) {
    if (!(await confirmDlg(t('Удалить алиас?'), a.name, { danger: true }))) return
    try { await api.del('/api/aliases?name=' + encodeURIComponent(a.name)); load() } catch (e) { app.err(e) }
  }
  async function saveGroup() {
    try {
      const body = { id: grp.id || '', name: grp.name, preset_ids: grp.preset_ids }
      const g = grp.id ? await api.put('/api/alias-groups/' + grp.id, body) : await api.post('/api/alias-groups', body)
      if (!grp.id && grp.alias) await api.post('/api/aliases', { name: grp.alias, target_type: 'group', target_id: g.id, is_primary: true })
      grp = null
      load()
    } catch (e) { app.err(e) }
  }
  async function delGroup(g: any) {
    if (!(await confirmDlg(t('Удалить группу?'), g.name + '\n' + t('Алиасы группы тоже будут удалены.'), { danger: true }))) return
    try { await api.del('/api/alias-groups/' + g.id); load() } catch (e) { app.err(e) }
  }
  function move(i: number, d: number) {
    const a = grp.preset_ids
    const j = i + d
    if (j < 0 || j >= a.length) return
    ;[a[i], a[j]] = [a[j], a[i]]
  }
</script>

<h1>{t('Алиасы и роутер')}</h1>

<div class="card" style="margin-bottom:14px">
  <div class="row"><b>{t('Единая точка входа OpenAI API')}</b><span class="sp"></span>
    <code>{base}/v1</code><button class="sm" onclick={() => copy(base + '/v1')}>📋</button></div>
  <div class="small muted" style="margin-top:6px">
    {t('Клиент указывает алиас в поле model. Если пресет не запущен — лаунчер запустит его (и при нехватке памяти выгрузит давно неиспользуемые). Ответ передаётся потоком, включая reasoning_content — размышления видны в реальном времени.')}
    {t('Нативный API конкретного инстанса')}: <code>{base}/instance/&lt;{t('алиас')}&gt;/…</code>
  </div>
  <details style="margin-top:8px"><summary class="small">{t('Пример запроса')}</summary><pre style="margin-top:6px">{example}</pre></details>
</div>

<div class="tabs">
  <button class:on={tab === 'aliases'} onclick={() => (tab = 'aliases')}>{t('Алиасы')} ({aliases.length})</button>
  <button class:on={tab === 'groups'} onclick={() => (tab = 'groups')}>{t('Группы')} ({groups.length})</button>
  <button class:on={tab === 'log'} onclick={() => { tab = 'log'; loadReqs() }}>{t('Журнал запросов')}</button>
</div>

{#if tab === 'aliases'}
  <div class="row" style="margin-bottom:10px">
    <span class="small muted">{t('У пресета может быть несколько алиасов; ★ — основной, передаётся в --alias.')}</span>
    <span class="sp"></span>
    <button class="primary" onclick={() => (newA = { name: '', target_type: 'preset', target_id: presets[0]?.id || '', is_primary: false })}>+ {t('Алиас')}</button>
  </div>
  <div class="card" style="padding:0">
    <table>
      <thead><tr><th>{t('Алиас')}</th><th>{t('Цель')}</th><th></th><th></th></tr></thead>
      <tbody>
        {#each aliases.sort((a, b) => a.name.localeCompare(b.name)) as a (a.name)}
          {@const inst = a.target_type === 'preset' ? app.instances[a.target_id] : null}
          <tr>
            <td><code>{a.name}</code> {#if a.is_primary}<span title={t('основной')}>★</span>{/if}</td>
            <td>
              {#if a.target_type === 'preset'}<a href="#/presets/{a.target_id}">{pname(a.target_id)}</a>{:else}<span class="badge info">{t('группа')}</span> {gname(a.target_id)}{/if}
              {#if inst}<span class="dot {inst.state}" style="margin-left:6px"></span>{/if}
            </td>
            <td><button class="sm ghost" onclick={() => copy(a.name)}>📋</button></td>
            <td style="text-align:right"><button class="sm ghost danger" onclick={() => delAlias(a)}>✕</button></td>
          </tr>
        {/each}
      </tbody>
    </table>
    {#if !aliases.length}<div class="empty">{t('Алиасов нет — пресеты доступны по названию.')}</div>{/if}
  </div>
{:else if tab === 'groups'}
  <div class="row" style="margin-bottom:10px">
    <span class="small muted">{t('Группа — один алиас на несколько пресетов: используется уже запущенный, иначе первый, который влезает в память (например, 2×GPU → 1×GPU → CPU).')}</span>
    <span class="sp"></span>
    <button class="primary" onclick={() => (grp = { name: '', alias: '', preset_ids: [] })}>+ {t('Группа')}</button>
  </div>
  <div class="cards">
    {#each groups as g (g.id)}
      <div class="card">
        <div class="row"><b>{g.name}</b><span class="sp"></span>
          {#each aliases.filter((a) => a.target_type === 'group' && a.target_id === g.id) as a}<code class="badge acc">{a.name}</code>{/each}</div>
        <ol class="small" style="margin:8px 0;padding-left:20px">{#each g.preset_ids as pid}<li>{pname(pid)}</li>{/each}</ol>
        <div class="row"><button class="sm" onclick={() => (grp = { ...g, preset_ids: [...g.preset_ids] })}>{t('Изменить')}</button><span class="sp"></span><button class="sm ghost danger" onclick={() => delGroup(g)}>✕</button></div>
      </div>
    {/each}
  </div>
{:else}
  <div class="row" style="margin-bottom:10px">
    <input placeholder={t('Фильтр…')} bind:value={rfilter} style="width:260px" />
    <span class="dim small">{shownReqs.length}</span>
    <span class="sp"></span>
    <button onclick={loadReqs}>{t('Обновить')}</button>
  </div>
  <div class="card scroll" style="padding:0">
    <table>
      <thead><tr><th>{t('Время')}</th><th>{t('Алиас')}</th><th>{t('Путь')}</th><th>{t('Клиент')}</th><th>{t('Статус')}</th><th>{t('Вход/выход')}</th><th>pp t/s</th><th>tg t/s</th><th>{t('мс')}</th></tr></thead>
      <tbody>
        {#each shownReqs as r}
          <tr>
            <td class="small">{date(r.ts)}</td>
            <td><code class="small">{r.alias}</code></td>
            <td class="small muted">{r.path}{r.stream ? ' ⇶' : ''}</td>
            <td class="small">{r.principal} <span class="dim">{r.ip}</span></td>
            <td><span class="badge {r.status < 400 ? 'ok' : 'bad'}">{r.status}</span></td>
            <td class="small">{r.prompt_tokens ?? '—'} / {r.completion_tokens ?? '—'}</td>
            <td class="small">{r.pp_tps ? r.pp_tps.toFixed(0) : '—'}</td>
            <td class="small">{r.tps ? r.tps.toFixed(1) : '—'}</td>
            <td class="small">{r.ms}</td>
          </tr>
        {/each}
      </tbody>
    </table>
    {#if !shownReqs.length}<div class="empty">{t('Запросов пока не было')}</div>{/if}
  </div>
{/if}

{#if newA}
  <Modal title={t('Новый алиас')} width="480px" onclose={() => (newA = null)}>
    <div class="col">
      <label class="field"><span>{t('Имя (латиница, цифры, - _ . : /)')}</span><input bind:value={newA.name} placeholder="gpt-4o" /></label>
      <label class="field"><span>{t('Тип')}</span><select bind:value={newA.target_type} onchange={() => (newA.target_id = (newA.target_type === 'preset' ? presets[0]?.id : groups[0]?.id) || '')}><option value="preset">{t('пресет')}</option><option value="group">{t('группа')}</option></select></label>
      <label class="field"><span>{t('Цель')}</span>
        <select bind:value={newA.target_id}>
          {#each newA.target_type === 'preset' ? presets : groups as x}<option value={x.id}>{x.name}</option>{/each}
        </select></label>
      {#if newA.target_type === 'preset'}<label class="chk"><input type="checkbox" bind:checked={newA.is_primary} /> {t('Основной (передавать в --alias)')}</label>{/if}
      <div class="small muted">{t('Если алиас уже существует — он будет перенаправлен на новую цель. Так можно незаметно для клиентов менять модель.')}</div>
    </div>
    {#snippet footer()}
      <button onclick={() => (newA = null)}>{t('Отмена')}</button>
      <button class="primary" disabled={!newA.name.trim() || !newA.target_id} onclick={saveAlias}>{t('Сохранить')}</button>
    {/snippet}
  </Modal>
{/if}

{#if grp}
  <Modal title={grp.id ? t('Группа') : t('Новая группа')} width="520px" onclose={() => (grp = null)}>
    <div class="col">
      <label class="field"><span>{t('Название')}</span><input bind:value={grp.name} /></label>
      {#if !grp.id}<label class="field"><span>{t('Алиас группы')}</span><input bind:value={grp.alias} placeholder="chat" /></label>{/if}
      <div class="field"><span>{t('Пресеты по приоритету')}</span>
        {#each grp.preset_ids as pid, i}
          <div class="row nw small"><span class="dim">{i + 1}.</span><span class="sp ell">{pname(pid)}</span>
            <button class="ghost sm" onclick={() => move(i, -1)}>↑</button><button class="ghost sm" onclick={() => move(i, 1)}>↓</button>
            <button class="ghost sm" onclick={() => (grp.preset_ids = grp.preset_ids.filter((x: string) => x !== pid))}>✕</button></div>
        {/each}
        <div class="row nw">
          <select bind:value={addPid} style="flex:1"><option value="">—</option>{#each presets.filter((p) => !grp.preset_ids.includes(p.id)) as p}<option value={p.id}>{p.name}</option>{/each}</select>
          <button class="sm" disabled={!addPid} onclick={() => { grp.preset_ids = [...grp.preset_ids, addPid]; addPid = '' }}>+</button>
        </div>
      </div>
    </div>
    {#snippet footer()}
      <button onclick={() => (grp = null)}>{t('Отмена')}</button>
      <button class="primary" disabled={!grp.name || !grp.preset_ids.length} onclick={saveGroup}>{t('Сохранить')}</button>
    {/snippet}
  </Modal>
{/if}
