<script lang="ts">
  import { api, q, download } from '../lib/api'
  import { app } from '../lib/state.svelte'
  import { t } from '../lib/i18n.svelte'
  import { num } from '../lib/fmt'
  import { confirmDlg } from '../lib/dialog.svelte'

  const periods: [string, string, number, number][] = [
    ['24h', '24 ч', 86400, 3600],
    ['7d', '7 дней', 7 * 86400, 86400],
    ['30d', '30 дней', 30 * 86400, 86400],
    ['90d', '90 дней', 90 * 86400, 86400],
    ['365d', 'Год', 365 * 86400, 7 * 86400],
  ]
  const groupBy: [string, string][] = [
    ['model', 'Модель'],
    ['preset', 'Пресет'],
    ['alias', 'Алиас'],
    ['user', 'Пользователь (API-ключ)'],
  ]
  const palette = ['#7c8cff', '#3ecf8e', '#f2b84b', '#4fb3ff', '#ef5a5a', '#c78bff', '#5fd4c8', '#ff9d6b', '#a4abb8']

  let period = $state(localStorage.getItem('lll_an_period') || '7d')
  let group = $state(localStorage.getItem('lll_an_group') || 'model')
  let mode = $state<'tokens' | 'groups'>('tokens')
  let filter = $state<{ key: string; label: string } | null>(null)
  let data = $state<any>(null)
  let loading = $state(false)

  const range = $derived.by(() => {
    const p = periods.find((x) => x[0] === period) || periods[1]
    const now = Math.floor(Date.now() / 1000)
    const tz = -new Date().getTimezoneOffset() * 60
    // align the start to a bucket boundary in local time
    const from = Math.floor((now - p[2] + tz) / p[3]) * p[3] - tz + p[3]
    return { from, to: now + 1, bucket: p[3], tz }
  })

  async function load() {
    loading = true
    try {
      data = await api.get('/api/analytics' + q({ ...range, group, filter: filter?.key }))
    } catch (e) { app.err(e) }
    loading = false
  }
  $effect(() => {
    localStorage.setItem('lll_an_period', period)
    localStorage.setItem('lll_an_group', group)
    void range; void filter
    load()
  })
  function setGroup(g: string) { group = g; filter = null }

  const label = (g: any) => (group === 'user' && !g.key ? t('без ключа') : g.label || g.key || '—')
  const tot = $derived(data?.totals || { requests: 0, prompt: 0, cached: 0, completion: 0, errors: 0 })
  const cacheRate = (x: any) => (x.prompt ? Math.round((x.cached / x.prompt) * 100) : 0)
  const groups = $derived((data?.groups || []) as any[])
  const maxGroupTok = $derived(Math.max(1, ...groups.map((g) => g.prompt + g.completion)))
  // top groups get their own colour, the rest is "other"
  const top = $derived(groups.slice(0, 8).map((g) => g.key))
  const color = (key: string) => { const i = top.indexOf(key); return i < 0 ? palette[8] : palette[i] }

  // buckets for the chart
  const buckets = $derived.by(() => {
    if (!data) return []
    const out: any[] = []
    for (let b = range.from; b < range.to; b += range.bucket) out.push({ t: b, input: 0, cached: 0, output: 0, requests: 0, by: {} as Record<string, number> })
    const idx = new Map(out.map((x, i) => [x.t, i]))
    for (const s of data.series) {
      const i = idx.get(s.t)
      if (i === undefined) continue
      const o = out[i]
      o.input += s.prompt - s.cached
      o.cached += s.cached
      o.output += s.completion
      o.requests += s.requests
      const k = top.includes(s.key) ? s.key : '__other'
      o.by[k] = (o.by[k] || 0) + s.prompt + s.completion
    }
    return out
  })
  const maxB = $derived(Math.max(1, ...buckets.map((b) => b.input + b.cached + b.output)))
  const W = 1000, H = 220
  const bw = $derived(buckets.length ? W / buckets.length : W)
  let hover = $state<any>(null)
  const fmtT = (ts: number) => {
    const d = new Date(ts * 1000)
    return range.bucket < 86400 ? d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) : d.toLocaleDateString([], { day: '2-digit', month: '2-digit' })
  }
  function segs(b: any) {
    const sc = (v: number) => (v / maxB) * H
    if (mode === 'tokens') return [
      { v: b.cached, c: 'var(--info)' }, { v: b.input, c: 'var(--acc)' }, { v: b.output, c: 'var(--ok)' },
    ].map((x) => ({ ...x, h: sc(x.v) }))
    return [...top, '__other'].filter((k) => b.by[k]).map((k) => ({ v: b.by[k], c: k === '__other' ? palette[8] : color(k), h: sc(b.by[k]) }))
  }

  async function clearAll() {
    if (!(await confirmDlg(t('Удалить всю статистику токенов?'), t('Действие необратимо.'), { danger: true }))) return
    try { await api.del('/api/analytics'); load() } catch (e) { app.err(e) }
  }
  const exportCsv = () => download('/api/analytics/export' + q({ from: range.from, to: range.to }), `usage-${period}.csv`).catch((e) => app.err(e))
</script>

<div class="row" style="margin-bottom:12px">
  <h1 style="margin:0">{t('Аналитика')}</h1>
  <span class="sp"></span>
  <div class="seg">{#each periods as p}<button class:on={period === p[0]} onclick={() => (period = p[0])}>{t(p[1])}</button>{/each}</div>
  <button onclick={exportCsv}>⇩ CSV</button>
  <button class="danger" title={t('Удалить всю статистику токенов?')} onclick={clearAll}>{t('Очистить')}</button>
</div>

<div class="cards">
  <div class="card stat"><span>{t('Запросов')}</span><b>{num(tot.requests)}</b>{#if tot.errors}<i class="bad">{tot.errors} {t('с ошибкой')}</i>{/if}</div>
  <div class="card stat"><span>{t('Входящие токены')}</span><b>{num(tot.prompt)}</b><i>{t('новых')}: {num(tot.prompt - tot.cached)}</i></div>
  <div class="card stat"><span>{t('Из кэша')}</span><b style="color:var(--info)">{num(tot.cached)}</b><i>{cacheRate(tot)}% {t('входящих')}</i></div>
  <div class="card stat"><span>{t('Исходящие токены')}</span><b style="color:var(--ok)">{num(tot.completion)}</b>{#if tot.avg_tps}<i>{t('в среднем')} {tot.avg_tps.toFixed(1)} tok/s</i>{/if}</div>
</div>

<div class="row" style="margin:14px 0 8px">
  <div class="seg">
    {#each groupBy as g}<button class:on={group === g[0]} onclick={() => setGroup(g[0])}>{t(g[1])}</button>{/each}
  </div>
  {#if filter}<span class="badge info">{filter.label} <button class="ghost sm" onclick={() => (filter = null)}>✕</button></span>{/if}
  <span class="sp"></span>
  <div class="seg small">
    <button class:on={mode === 'tokens'} onclick={() => (mode = 'tokens')}>{t('По типу токенов')}</button>
    <button class:on={mode === 'groups'} onclick={() => (mode = 'groups')}>{t('По группам')}</button>
  </div>
</div>

<div class="card chart">
  {#if loading && !data}<div class="empty"><span class="spin"></span></div>{/if}
  <svg viewBox="0 0 {W} {H + 22}" preserveAspectRatio="none" role="img" onmouseleave={() => (hover = null)}>
    {#each [0.25, 0.5, 0.75, 1] as f}<line x1="0" x2={W} y1={H - H * f} y2={H - H * f} class="grid" />{/each}
    {#each buckets as b, i}
      {@const ss = segs(b)}
      <g role="presentation" onmouseenter={() => (hover = { ...b, i })}>
        <rect x={i * bw} y="0" width={bw} height={H} fill="transparent" />
        {#each ss as s, k}
          {@const y = H - ss.slice(0, k + 1).reduce((a, x) => a + x.h, 0)}
          <rect x={i * bw + bw * 0.15} y={y} width={Math.max(1, bw * 0.7)} height={s.h} fill={s.c} rx="1.5" />
        {/each}
        {#if i % Math.max(1, Math.ceil(buckets.length / 12)) === 0}
          <text x={Math.max(i * bw + bw / 2, 22)} y={H + 15} class="ax">{fmtT(b.t)}</text>
        {/if}
      </g>
    {/each}
  </svg>
  <div class="row tiny legend">
    {#if mode === 'tokens'}
      <span><i style="background:var(--acc)"></i>{t('входящие (новые)')}</span>
      <span><i style="background:var(--info)"></i>{t('входящие из кэша')}</span>
      <span><i style="background:var(--ok)"></i>{t('исходящие')}</span>
    {:else}
      {#each groups.slice(0, 8) as g}<span><i style="background:{color(g.key)}"></i>{label(g)}</span>{/each}
      {#if groups.length > 8}<span><i style="background:{palette[8]}"></i>{t('прочие')}</span>{/if}
    {/if}
    <span class="sp"></span>
    <span class="dim">{t('макс.')} {num(maxB)} tok / {range.bucket >= 86400 ? (range.bucket > 86400 ? t('неделя') : t('день')) : t('час')}</span>
  </div>
  {#if hover}
    <div class="tip small">
      <b>{new Date(hover.t * 1000).toLocaleString([], range.bucket < 86400 ? { dateStyle: 'short', timeStyle: 'short' } : { dateStyle: 'medium' })}</b>
      <div>{t('Запросов')}: {hover.requests}</div>
      <div>{t('Входящие')}: {num(hover.input + hover.cached)} <span class="dim">({t('из кэша')} {num(hover.cached)})</span></div>
      <div>{t('Исходящие')}: {num(hover.output)}</div>
    </div>
  {/if}
</div>

<div class="card scroll" style="padding:0;margin-top:14px">
  <table>
    <thead><tr>
      <th>{t(groupBy.find((g) => g[0] === group)?.[1] || '')}</th><th>{t('Запросов')}</th><th>{t('Входящие')}</th><th>{t('Из кэша')}</th>
      <th>{t('Исходящие')}</th><th>{t('Всего')}</th><th>tok/s</th><th>{t('Ошибки')}</th>
    </tr></thead>
    <tbody>
      {#each groups as g (g.key)}
        <tr class="clk" class:sel={filter?.key === g.key} onclick={() => (filter = filter?.key === g.key ? null : { key: g.key, label: label(g) })}>
          <td><span class="dot" style="background:{color(g.key)}"></span> {label(g)}</td>
          <td>{num(g.requests)}</td>
          <td>{num(g.prompt)}</td>
          <td>{num(g.cached)} <span class="dim tiny">{cacheRate(g)}%</span></td>
          <td>{num(g.completion)}</td>
          <td style="width:22%"><div class="small">{num(g.prompt + g.completion)}</div>
            <div class="bar"><div style="width:{((g.prompt - g.cached) / maxGroupTok) * 100}%;background:var(--acc)"></div><div style="width:{(g.cached / maxGroupTok) * 100}%;background:var(--info)"></div><div style="width:{(g.completion / maxGroupTok) * 100}%;background:var(--ok)"></div></div></td>
          <td class="small">{g.avg_tps ? g.avg_tps.toFixed(1) : '—'}</td>
          <td class="small" class:bad={g.errors}>{g.errors || ''}</td>
        </tr>
      {/each}
    </tbody>
  </table>
  {#if data && !groups.length}<div class="empty">{t('За выбранный период запросов через роутер не было')}</div>{/if}
</div>
<div class="tiny dim" style="margin-top:8px">{t('Учитываются все запросы через роутер (/v1/…, нативный API инстансов и встроенный чат). Кэш — токены промпта, взятые из KV-кэша llama-server (prompt cache), они не пересчитываются заново.')}</div>

<style>
  .cards { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 12px; }
  .stat { display: flex; flex-direction: column; gap: 4px; }
  .stat span { color: var(--fg3); font-size: 12.5px; }
  .stat b { font-size: 26px; font-weight: 600; }
  .stat i { font-style: normal; font-size: 12px; color: var(--fg2); }
  .seg { display: inline-flex; border: 1px solid var(--line); border-radius: 8px; overflow: hidden; }
  .seg button { border: none; border-radius: 0; background: none; padding: 6px 11px; color: var(--fg2); }
  .seg button.on { background: var(--bg4); color: var(--fg); }
  .chart { position: relative; }
  svg { width: 100%; height: 260px; display: block; }
  .grid { stroke: var(--line); stroke-width: 1; vector-effect: non-scaling-stroke; }
  .ax { fill: var(--fg3); font-size: 11px; text-anchor: middle; }
  .legend { gap: 14px; margin-top: 6px; }
  .legend i { display: inline-block; width: 10px; height: 10px; border-radius: 3px; margin-right: 5px; vertical-align: -1px; }
  .tip { position: absolute; top: 12px; right: 14px; background: var(--bg3); border: 1px solid var(--line); border-radius: 8px; padding: 8px 10px; pointer-events: none; }
  .dot { display: inline-block; width: 9px; height: 9px; border-radius: 50%; margin-right: 4px; }
  tr.clk { cursor: pointer; }
  tr.clk:hover, tr.sel { background: var(--bg3); }
  @media (max-width: 900px) { .cards { grid-template-columns: 1fr 1fr; } }
</style>
