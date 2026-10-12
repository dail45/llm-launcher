<script lang="ts">
  import { onMount } from 'svelte'
  import { api } from '../lib/api'
  import { app } from '../lib/state.svelte'
  import { t } from '../lib/i18n.svelte'
  import { bytes, date } from '../lib/fmt'
  import { confirmDlg } from '../lib/dialog.svelte'

  let runs = $state<any[]>([])
  let models = $state<any[]>([])
  let backends = $state<any[]>([])
  let sel = $state<string[]>([])
  let base = $state('')
  let backend = $state('')
  let dataset = $state('wikitext2')
  let customPath = $state('')
  let ctx = $state(512)
  let chunks = $state(40)
  let ngl = $state(99)
  let help = $state(localStorage.getItem('lll_ppl_help') !== '0')
  let filter = $state('')

  const jobs = $derived(Object.values(app.jobs).filter((j: any) => j.kind === 'perplexity').sort((a: any, b: any) => b.created_at - a.created_at).slice(0, 2) as any[])
  const running = $derived(jobs.some((j) => j.status === 'running'))
  // group by architecture+name so only quants of the same model are offered together
  const family = (m: any) => (m.file_name || '').replace(/[-_.](i?q\d.*|bf16|f16|f32|ud-.*)\.gguf$/i, '').replace(/-\d{5}-of-\d{5}$/i, '')
  const families = $derived([...new Set(models.map(family))].sort())
  const shownModels = $derived(models.filter((m) => !filter || family(m) === filter))
  const groups = $derived.by(() => {
    const g: Record<string, any[]> = {}
    for (const r of runs) (g[r.group] ||= []).push(r)
    return Object.entries(g)
      .map(([id, rs]) => ({ id, rs: rs.sort((a, b) => (b.is_base ? 1 : 0) - (a.is_base ? 1 : 0) || b.size - a.size), at: Math.max(...rs.map((r) => r.created_at)) }))
      .sort((a, b) => b.at - a.at)
  })
  const vocab = $derived(models.find((m) => m.id === base)?.info?.n_vocab || 150000)
  // llama-perplexity stores logits (uint16) for the second half of every chunk
  const kldBytes = $derived(base ? vocab * (ctx / 2) * (chunks || 580) * 2 : 0)

  async function load() {
    try { runs = await api.get('/api/perplexity') } catch (e) { app.err(e) }
  }
  onMount(async () => {
    try {
      ;[models, backends] = await Promise.all([api.get('/api/models'), api.get('/api/backends')])
      models = models.filter((m) => !m.info?.is_mmproj)
      backend = backends.find((b) => b.is_default)?.id || backends[0]?.id || ''
    } catch (e) { app.err(e) }
    load()
  })
  let seen = new Set<string>()
  $effect(() => {
    for (const j of jobs) if (j.status !== 'running' && !seen.has(j.id)) { seen.add(j.id); load() }
  })
  function toggle(id: string) {
    sel = sel.includes(id) ? sel.filter((x) => x !== id) : [...sel, id]
    if (base && !sel.includes(base)) base = ''
  }
  async function run() {
    const ids = [...new Set([...(base ? [base] : []), ...sel])]
    try {
      await api.post('/api/perplexity', { model_ids: ids, base_model_id: base || null, backend_id: backend, dataset: dataset === 'custom' ? customPath : 'wikitext2', ctx, chunks, ngl })
      app.toast('info', t('Сравнение запущено'), t('Модели прогоняются по очереди; лучше не запускать ничего на GPU параллельно'))
    } catch (e) { app.err(e) }
  }
  async function delGroup(id: string) {
    if (!(await confirmDlg(t('Удалить результаты сравнения?')))) return
    try { await api.del('/api/perplexity/' + id); load() } catch (e) { app.err(e) }
  }
  function verdict(kld: number | null) {
    if (kld == null) return ['', '']
    if (kld < 0.01) return ['ok', t('почти без потерь')]
    if (kld < 0.05) return ['ok', t('хорошо')]
    if (kld < 0.1) return ['warn', t('заметные потери')]
    if (kld < 0.3) return ['warn', t('сильные потери')]
    return ['bad', t('очень сильные потери')]
  }
  const f = (x: number | null | undefined, d = 3) => (x == null ? '—' : x.toFixed(d))
</script>

<div class="card col" style="margin-bottom:14px">
  <div class="row"><h3 style="margin:0">{t('Что это такое')}</h3><span class="sp"></span><button class="ghost sm" onclick={() => { help = !help; localStorage.setItem('lll_ppl_help', help ? '1' : '0') }}>{help ? t('скрыть') : t('показать')}</button></div>
  {#if help}
    <div class="small help">
      <p>{t('Квантование сжимает модель, но немного портит её ответы. Здесь можно измерить, насколько именно, — на вашем железе и вашей сборке llama.cpp (утилита llama-perplexity).')}</p>
      <ul>
        <li><b>Perplexity (PPL)</b> — {t('насколько модель «удивлена» реальным текстом: она предсказывает каждое следующее слово, а PPL показывает среднюю неуверенность. Меньше — лучше. Сравнивать можно только кванты одной и той же модели на одном тексте с одинаковым контекстом.')}</li>
        <li><b>KL-divergence (KLD)</b> — {t('насколько вероятности слов у кванта отличаются от эталона (BF16 или Q8_0 той же модели). Точнее PPL: квант может случайно получить такой же PPL, но «думать» иначе. 0 — идентично.')} <span class="dim">&lt;0.01 {t('почти без потерь')}, 0.01–0.05 {t('хорошо')}, 0.05–0.1 {t('заметно')}, &gt;0.1 {t('сильно')}</span></li>
        <li><b>Same top</b> — {t('в каком проценте случаев квант выбрал то же наиболее вероятное слово, что и эталон.')}</li>
      </ul>
      <p class="dim">{t('Для KLD сначала прогоняется эталон, его вероятности временно сохраняются на диск, затем с ними сравнивается каждый квант. Чем больше кусков текста — тем точнее и дольше.')}</p>
    </div>
  {/if}
</div>

<div class="card" style="margin-bottom:14px">
  <div class="grid g2">
    <div class="field">
      <span>{t('Кванты для сравнения')}</span>
      <select bind:value={filter} style="margin-bottom:6px"><option value="">{t('все модели')}</option>{#each families as fam}<option value={fam}>{fam}</option>{/each}</select>
      <div class="mlist">
        {#each shownModels as m (m.id)}
          <label class="chk small mrow"><input type="checkbox" checked={sel.includes(m.id) || base === m.id} disabled={base === m.id} onchange={() => toggle(m.id)} />
            <span class="ell" style="flex:1" title={m.file_name}>{m.file_name}</span><span class="dim">{bytes(m.size)}</span>
            <button class="ghost sm" class:on={base === m.id} title={t('Сделать эталоном для KLD')} onclick={(e) => { e.preventDefault(); base = base === m.id ? '' : m.id; if (base && !sel.includes(base)) sel = [...sel, base] }}>{base === m.id ? '★ ' + t('эталон') : '☆'}</button></label>
        {/each}
        {#if !models.length}<div class="dim small">{t('Нет моделей')}</div>{/if}
      </div>
      <span class="hint">{t('☆ — эталон для KLD (обычно самый большой квант: BF16 / Q8_0). Без эталона считается только PPL.')}</span>
    </div>
    <div class="col">
      <label class="field"><span>{t('Сборка')}</span><select bind:value={backend}>{#each backends as b}<option value={b.id}>{b.name}</option>{/each}</select></label>
      <label class="field"><span>{t('Текст')}</span>
        <select bind:value={dataset}><option value="wikitext2">wikitext-2 ({t('стандарт, скачается ~4 МБ')})</option><option value="custom">{t('свой файл .txt')}</option></select></label>
      {#if dataset === 'custom'}<label class="field"><span>{t('Путь к файлу')}</span><input bind:value={customPath} placeholder="C:\texts\sample.txt" /><span class="hint">{t('Например, ваш код или документы — так видно потери именно на ваших задачах')}</span></label>{/if}
      <div class="row">
        <label class="field" style="flex:1"><span>{t('Контекст')}</span><input type="number" bind:value={ctx} /></label>
        <label class="field" style="flex:1"><span>{t('Кусков текста')}</span><input type="number" bind:value={chunks} /><span class="hint">0 — {t('весь текст')}</span></label>
        <label class="field" style="flex:1"><span>-ngl</span><input type="number" bind:value={ngl} /></label>
      </div>
      {#if base}<div class="small {kldBytes > 20e9 ? 'bad' : 'dim'}">{t('Временный файл вероятностей эталона')}: ≈ {bytes(kldBytes)}</div>{/if}
      <div class="row"><span class="sp"></span><button class="primary" disabled={running || !backend || sel.length + (base && !sel.includes(base) ? 1 : 0) < 1 || (dataset === 'custom' && !customPath)} onclick={run}>▶ {t('Сравнить')}</button></div>
    </div>
  </div>
  {#each jobs as j (j.id)}
    <div class="row small" style="margin-top:8px">
      {#if j.status === 'running'}<span class="spin"></span>{/if}<b>{j.title}</b><span class="muted">{j.message}</span>
      <span class="sp"></span><span class="badge {j.status === 'failed' ? 'bad' : j.status === 'done' ? 'ok' : 'info'}">{j.status}</span>
    </div>
    {#if j.progress != null && j.status === 'running'}<div class="bar"><div style="width:{j.progress * 100}%;background:var(--acc)"></div></div>{/if}
  {/each}
</div>

{#each groups as g (g.id)}
  {@const hasKld = g.rs.some((r) => r.kld != null)}
  {@const basePpl = g.rs.find((r) => r.is_base)?.ppl ?? Math.min(...g.rs.filter((r) => r.ppl).map((r) => r.ppl))}
  {@const maxKld = Math.max(0.001, ...g.rs.map((r) => r.kld || 0))}
  <div class="card" style="margin-bottom:12px;padding:0">
    <div class="row" style="padding:10px 14px">
      <b>{date(g.at)}</b><span class="dim small">{g.rs[0].dataset} · ctx {g.rs[0].ctx} · {g.rs[0].chunks || t('весь текст')} {t('кусков')}</span>
      <span class="sp"></span><button class="ghost sm" onclick={() => delGroup(g.id)}>✕</button>
    </div>
    <table>
      <thead><tr><th>{t('Модель')}</th><th>{t('Размер')}</th><th>PPL</th><th>ΔPPL</th>{#if hasKld}<th style="width:22%">KLD</th><th>KLD 99%</th><th>Same top</th>{/if}<th>{t('Время')}</th></tr></thead>
      <tbody>
        {#each g.rs as r (r.id)}
          {@const v = verdict(r.kld)}
          <tr>
            <td class="small"><div class="ell" style="max-width:320px" title={r.model}>{r.model}</div>
              <div class="row tiny">{#if r.quant}<span class="badge acc">{r.quant}</span>{/if}{#if r.is_base}<span class="badge">★ {t('эталон')}</span>{/if}</div></td>
            <td class="small">{bytes(r.size)}</td>
            {#if r.error}
              <td colspan={hasKld ? 5 : 2} class="small" style="color:var(--bad)">{r.error}</td>
            {:else}
              <td class="small"><b>{f(r.ppl, 4)}</b> <span class="dim">± {f(r.ppl_err, 3)}</span></td>
              <td class="small">{r.ppl && basePpl && !r.is_base ? '+' + (((r.ppl - basePpl) / basePpl) * 100).toFixed(2) + '%' : '—'}</td>
              {#if hasKld}
                <td>{#if r.kld != null}<div class="row small nw"><b>{f(r.kld, 4)}</b><span class="badge {v[0]}">{v[1]}</span></div><div class="bar thin"><div style="width:{(r.kld / maxKld) * 100}%;background:var(--{v[0] || 'info'})"></div></div>{:else}<span class="dim small">{r.is_base ? '0' : '—'}</span>{/if}</td>
                <td class="small">{f(r.kld_p99, 3)}</td>
                <td class="small">{r.same_top != null ? r.same_top.toFixed(1) + '%' : r.is_base ? '100%' : '—'}</td>
              {/if}
            {/if}
            <td class="small dim">{Math.round(r.secs)} {t('с')}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/each}
{#if !groups.length}<div class="empty">{t('Сравнений пока нет')}</div>{/if}

<style>
  .help p { margin: 6px 0; }
  .help ul { margin: 6px 0; padding-left: 18px; display: flex; flex-direction: column; gap: 6px; }
  .mlist { max-height: 260px; overflow: auto; border: 1px solid var(--line); border-radius: var(--r); padding: 4px 8px; }
  .mrow { display: flex; width: 100%; padding: 3px 0; }
  button.on { color: var(--warn); }
</style>
