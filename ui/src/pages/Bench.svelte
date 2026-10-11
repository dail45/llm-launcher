<script lang="ts">
  import { onMount } from 'svelte'
  import { api, q } from '../lib/api'
  import { app } from '../lib/state.svelte'
  import { t } from '../lib/i18n.svelte'
  import { date } from '../lib/fmt'

  let runs = $state<any[]>([])
  let models = $state<any[]>([])
  let backends = $state<any[]>([])
  let modelId = $state('')
  let bsel = $state<string[]>([])
  let sets = $state('-ngl 99 -fa 1\n-ngl 99 -fa 1 -ctk q8_0 -ctv q8_0')
  let pp = $state(512)
  let tg = $state(128)
  let reps = $state(3)
  let fModel = $state('')

  const jobs = $derived(Object.values(app.jobs).filter((j: any) => j.kind === 'bench').sort((a: any, b: any) => b.created_at - a.created_at).slice(0, 3) as any[])
  const mname = (id: string) => models.find((m) => m.id === id)?.file_name || id
  const bname = (id: string) => backends.find((b) => b.id === id)?.name || id
  const shown = $derived(runs.filter((r) => !fModel || r.model_id === fModel).sort((a, b) => b.created_at - a.created_at))
  const maxTg = $derived(Math.max(1, ...shown.map((r) => r.tg_ts || 0)))
  const maxPp = $derived(Math.max(1, ...shown.map((r) => r.pp_ts || 0)))

  async function load() {
    try { runs = await api.get('/api/bench') } catch (e) { app.err(e) }
  }
  onMount(async () => {
    try {
      ;[models, backends] = await Promise.all([api.get('/api/models'), api.get('/api/backends')])
      models = models.filter((m) => !m.info.is_mmproj)
      bsel = backends.filter((b) => b.is_default && b.bench_bin).map((b) => b.id)
    } catch (e) { app.err(e) }
    load()
  })
  let seen = new Set<string>()
  $effect(() => {
    for (const j of jobs) if (j.status !== 'running' && !seen.has(j.id)) { seen.add(j.id); load() }
  })

  function parseSets() {
    return sets.split('\n').map((line) => {
      const m: Record<string, any> = {}
      const toks = line.trim().split(/\s+/).filter(Boolean)
      for (let i = 0; i < toks.length; i++) {
        if (!toks[i].startsWith('-')) continue
        const nxt = toks[i + 1]
        if (nxt !== undefined && !nxt.startsWith('-')) { m[toks[i]] = isNaN(+nxt) ? nxt : +nxt; i++ }
        else m[toks[i]] = 1
      }
      return m
    }).filter((m) => Object.keys(m).length)
  }
  async function run() {
    try {
      const param_sets = parseSets()
      await api.post('/api/bench', { model_id: modelId, backend_ids: bsel, param_sets: param_sets.length ? param_sets : [{}], pp, tg, reps })
      app.toast('info', t('Бенчмарк запущен'), t('Модель будет загружаться для каждой комбинации'))
    } catch (e) { app.err(e) }
  }
  async function del(r: any) {
    try { await api.del('/api/bench/' + r.id); runs = runs.filter((x) => x.id !== r.id) } catch (e) { app.err(e) }
  }
  const fmtP = (p: any) => Object.entries(p || {}).map(([k, v]) => `${k} ${v}`).join(' ')
</script>

<h1>{t('Бенчмарки')}</h1>

<div class="card" style="margin-bottom:14px">
  <div class="grid g2">
    <label class="field"><span>{t('Модель')}</span>
      <select bind:value={modelId}><option value="">—</option>{#each models as m}<option value={m.id}>{m.file_name}</option>{/each}</select></label>
    <div class="field"><span>{t('Сборки (нужен llama-bench)')}</span>
      <div class="row">{#each backends as b}<label class="chk small" title={b.bench_bin ? '' : t('llama-bench не найден')}><input type="checkbox" disabled={!b.bench_bin} checked={bsel.includes(b.id)} onchange={() => (bsel = bsel.includes(b.id) ? bsel.filter((x) => x !== b.id) : [...bsel, b.id])} /> {b.name}</label>{/each}</div></div>
  </div>
  <label class="field" style="margin-top:10px"><span>{t('Наборы параметров llama-bench — по одному в строке')}</span>
    <textarea rows="3" bind:value={sets}></textarea>
    <span class="hint">{t('Каждая строка × каждая сборка = отдельный прогон. Например: -ngl 99 -fa 1 -ctk q8_0 -ctv q8_0 -t 8')}</span></label>
  <div class="row" style="margin-top:10px">
    <label class="row small">pp <input type="number" bind:value={pp} style="width:80px" /></label>
    <label class="row small">tg <input type="number" bind:value={tg} style="width:80px" /></label>
    <label class="row small">{t('повторов')} <input type="number" bind:value={reps} style="width:60px" /></label>
    <span class="sp"></span>
    <button class="primary" disabled={!modelId || !bsel.length || jobs.some((j) => j.status === 'running')} onclick={run}>▶ {t('Запустить')}</button>
  </div>
  {#each jobs as j (j.id)}
    <div class="row small" style="margin-top:8px">
      {#if j.status === 'running'}<span class="spin"></span>{/if}<b>{j.title}</b><span class="muted">{j.message}</span>
      <span class="sp"></span><span class="badge {j.status === 'failed' ? 'bad' : j.status === 'done' ? 'ok' : 'info'}">{j.status}</span>
    </div>
    {#if j.progress != null && j.status === 'running'}<div class="bar"><div style="width:{j.progress * 100}%;background:var(--acc)"></div></div>{/if}
  {/each}
  <div class="tiny dim" style="margin-top:6px">{t('Во время бенчмарка не запускайте другие модели — результаты будут занижены.')}</div>
</div>

<div class="row" style="margin-bottom:8px">
  <h2 style="margin:0">{t('Результаты')}</h2><span class="sp"></span>
  <select bind:value={fModel}><option value="">{t('все модели')}</option>{#each [...new Set(runs.map((r) => r.model_id))] as id}<option value={id}>{mname(id)}</option>{/each}</select>
</div>
<div class="card scroll" style="padding:0">
  <table>
    <thead><tr><th>{t('Дата')}</th><th>{t('Модель')}</th><th>{t('Сборка')}</th><th>{t('Параметры')}</th><th>pp t/s</th><th>tg t/s</th><th></th></tr></thead>
    <tbody>
      {#each shown as r (r.id)}
        <tr>
          <td class="small">{date(r.created_at)}</td>
          <td class="small ell" style="max-width:240px">{mname(r.model_id)}</td>
          <td class="small">{bname(r.backend_id)}</td>
          <td class="small mono">{fmtP(r.params)}</td>
          {#if r.error}
            <td colspan="2" class="small bad">{r.error}</td>
          {:else}
            <td style="width:16%"><div class="small">{r.pp_ts?.toFixed(1) ?? '—'}</div><div class="bar"><div style="width:{((r.pp_ts || 0) / maxPp) * 100}%;background:var(--info)"></div></div></td>
            <td style="width:16%"><div class="small"><b>{r.tg_ts?.toFixed(1) ?? '—'}</b></div><div class="bar"><div style="width:{((r.tg_ts || 0) / maxTg) * 100}%;background:var(--ok)"></div></div></td>
          {/if}
          <td><button class="sm ghost" onclick={() => del(r)}>✕</button></td>
        </tr>
      {/each}
    </tbody>
  </table>
  {#if !shown.length}<div class="empty">{t('Результатов нет')}</div>{/if}
</div>
