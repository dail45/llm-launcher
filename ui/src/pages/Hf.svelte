<script lang="ts">
  import { onMount } from 'svelte'
  import { api, q } from '../lib/api'
  import { app } from '../lib/state.svelte'
  import { t } from '../lib/i18n.svelte'
  import { bytes, num, speed, date } from '../lib/fmt'
  import { confirmDlg } from '../lib/dialog.svelte'
  import MemBar from '../components/MemBar.svelte'
  import Modal from '../components/Modal.svelte'

  let tab = $state('search')
  let query = $state(localStorage.getItem('lll_hf_q') || '')
  let sort = $state('downloads')
  let hits = $state<any[]>([])
  let searching = $state(false)
  let repo = $state<string | null>(null)
  let files = $state<any[]>([])
  let filesLoading = $state(false)
  let readme = $state('')
  let showReadme = $state(false)
  let est = $state<any>(null)
  let estFor = $state('')
  let estCtx = $state(8192)
  let estLoading = $state(false)
  let backends = $state<any[]>([])
  let estBackend = $state('')

  const dls = $derived(Object.values(app.downloads).sort((a: any, b: any) => b.created_at - a.created_at) as any[])

  onMount(async () => {
    backends = await api.get('/api/backends').catch(() => [])
    estBackend = backends.find((b) => b.is_default)?.id || backends[0]?.id || ''
    if (query) search()
  })

  async function search(e?: Event) {
    e?.preventDefault()
    localStorage.setItem('lll_hf_q', query)
    searching = true
    try { hits = await api.get('/api/hf/search' + q({ q: query, sort, limit: 50 })) } catch (er) { app.err(er) } finally { searching = false }
  }
  async function openRepo(id: string) {
    repo = id
    files = []
    readme = ''
    est = null
    filesLoading = true
    try { files = await api.get('/api/hf/files' + q({ repo: id, backend: estBackend })) } catch (e) { app.err(e) } finally { filesLoading = false }
  }
  async function loadReadme() {
    showReadme = true
    if (!readme) try { readme = (await api.get('/api/hf/readme' + q({ repo }))).readme } catch (e) { app.err(e) }
  }
  async function estimate(f: any) {
    estFor = f.name
    estLoading = true
    try { est = await api.post('/api/hf/estimate', { repo, parts: f.parts, n_ctx: estCtx, backend_id: estBackend || null }) } catch (e) { app.err(e); est = null } finally { estLoading = false }
  }
  async function dl(f: any, withMmproj = true) {
    const parts = [...f.parts]
    if (withMmproj) {
      const mm = files.find((x) => x.is_mmproj && !x.local_model_id)
      if (mm && !f.is_mmproj && (await confirmDlg(t('Скачать и mmproj?'), t('В репозитории есть проектор для зрения: {f}', { f: mm.name }), { ok: t('Да, вместе') }))) parts.push(...mm.parts)
    }
    try {
      const r = await api.post('/api/hf/download', { repo, parts })
      app.toast('success', t('Добавлено в загрузки'), r.length ? `${r.length} ${t('файл(ов)')}` : t('файлы уже скачаны'))
      tab = 'downloads'
    } catch (e) { app.err(e) }
  }
  const fitCls = (f: string) => (f === 'good' ? 'ok' : f === 'tight' ? 'warn' : 'bad')
  const fitTxt = (f: string) => (f === 'good' ? t('влезет в VRAM') : f === 'tight' ? t('частично / впритык') : t('не влезет'))

  async function pause(d: any) { try { await api.post(`/api/downloads/${d.id}/pause`) } catch (e) { app.err(e) } }
  async function resume(d: any) { try { await api.post(`/api/downloads/${d.id}/resume`) } catch (e) { app.err(e) } }
  async function remove(d: any) {
    const partial = d.status !== 'done'
    if (partial && !(await confirmDlg(t('Отменить загрузку?'), d.file + '\n' + t('Недокачанный файл будет удалён.'), { danger: true }))) return
    try { await api.del(`/api/downloads/${d.id}` + q({ delete_partial: partial })); delete app.downloads[d.id] } catch (e) { app.err(e) }
  }
</script>

<div class="row" style="margin-bottom:12px">
  <h1 style="margin:0">Hugging Face</h1>
</div>
<div class="tabs">
  <button class:on={tab === 'search'} onclick={() => (tab = 'search')}>{t('Поиск')}</button>
  <button class:on={tab === 'downloads'} onclick={() => (tab = 'downloads')}>{t('Загрузки')} {#if dls.filter((d) => d.status === 'running').length}<span class="badge info">{dls.filter((d) => d.status === 'running').length}</span>{/if}</button>
</div>

{#if tab === 'search'}
  {#if !repo}
    <form class="row" onsubmit={search} style="margin-bottom:12px">
      <input placeholder={t('Например: qwen3 gguf, unsloth, bartowski…')} bind:value={query} style="flex:1;max-width:520px" />
      <select bind:value={sort} onchange={() => search()}>
        <option value="downloads">{t('по скачиваниям')}</option><option value="likes">{t('по лайкам')}</option>
        <option value="trendingScore">{t('в тренде')}</option><option value="lastModified">{t('по обновлению')}</option><option value="createdAt">{t('новые')}</option>
      </select>
      <button class="primary" disabled={searching}>{#if searching}<span class="spin"></span>{/if}{t('Найти')}</button>
    </form>
    <div class="card" style="padding:0">
      <table>
        <tbody>
          {#each hits as h (h.id)}
            <tr class="click" onclick={() => openRepo(h.id)}>
              <td><b>{h.id}</b>{#if h.gated}<span class="badge warn" style="margin-left:6px">gated</span>{/if}
                <div class="row tiny" style="margin-top:2px">{#each h.tags.filter((x: string) => !x.includes(':') && x !== 'gguf').slice(0, 6) as tg}<span class="badge">{tg}</span>{/each}</div></td>
              <td class="small muted">⇩ {num(h.downloads)}</td>
              <td class="small muted">♥ {num(h.likes)}</td>
              <td class="small dim">{h.last_modified?.slice(0, 10)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
      {#if !hits.length && !searching}<div class="empty">{t('Ищите GGUF-репозитории. Для закрытых моделей укажите HF-токен в настройках.')}</div>{/if}
    </div>
  {:else}
    <div class="row" style="margin-bottom:12px">
      <button class="ghost" onclick={() => (repo = null)}>←</button>
      <h2 style="margin:0">{repo}</h2>
      <a href="https://huggingface.co/{repo}" target="_blank" class="small">{t('на сайте')} ↗</a>
      <span class="sp"></span>
      <button onclick={loadReadme}>README</button>
    </div>
    <div class="row small" style="margin-bottom:10px">
      <span class="muted">{t('Оценка для')}:</span>
      ctx <input type="number" bind:value={estCtx} style="width:90px" step="1024" />
      <select bind:value={estBackend} onchange={() => repo && openRepo(repo)}>{#each backends as b}<option value={b.id}>{b.name}</option>{/each}</select>
    </div>
    <div class="grid" style="grid-template-columns:minmax(0,1fr) {est ? '380px' : '0px'}">
      <div class="card" style="padding:0">
        {#if filesLoading}<div class="empty"><span class="spin"></span></div>{/if}
        <table>
          <tbody>
            {#each files as f (f.name)}
              <tr>
                <td>
                  <div class="ell" style="max-width:420px" title={f.name}>{f.name}</div>
                  <div class="row tiny">
                    {#if f.quant}<span class="badge acc">{f.quant}</span>{/if}
                    {#if f.parts.length > 1}<span class="badge">{f.parts.length} {t('частей')}</span>{/if}
                    {#if f.is_mmproj}<span class="badge info">mmproj</span>{/if}
                    {#if f.local_model_id}<span class="badge ok">{t('скачано')}</span>{/if}
                  </div>
                </td>
                <td class="small">{bytes(f.size)}</td>
                <td>{#if !f.is_mmproj}<span class="badge {fitCls(f.fit)}" title={f.fit_note}>{fitTxt(f.fit)}</span>{/if}</td>
                <td class="nw" style="text-align:right">
                  <div class="row" style="justify-content:flex-end">
                    {#if !f.is_mmproj}<button class="sm" disabled={estLoading && estFor === f.name} onclick={() => estimate(f)}>{estLoading && estFor === f.name ? '…' : t('Оценить')}</button>{/if}
                    <button class="sm primary" disabled={!!f.local_model_id} onclick={() => dl(f)}>⇩</button>
                  </div>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
        {#if !filesLoading && !files.length}<div class="empty">{t('GGUF-файлов не найдено')}</div>{/if}
      </div>
      {#if est}
        <div class="card" style="align-self:start;position:sticky;top:12px">
          <div class="row"><b class="ell small">{estFor}</b><span class="sp"></span><button class="ghost sm" onclick={() => (est = null)}>✕</button></div>
          <div class="small muted" style="margin:6px 0">{est.info.arch} · {est.info.n_layers} {t('слоёв')} · ctx max {est.info.ctx_max || '?'}</div>
          <MemBar est={est.estimate} />
          <div class="kv small" style="margin-top:10px">
            <span>{t('Макс. -ngl')}</span><span>{est.max_ngl ?? '—'}</span>
            <span>{t('Макс. контекст')}</span><span>{est.max_ctx ?? '—'}</span>
            {#if est.n_cpu_moe != null}<span>--n-cpu-moe</span><span>{est.n_cpu_moe}</span>{/if}
          </div>
        </div>
      {/if}
    </div>
  {/if}
{:else}
  <div class="card" style="padding:0">
    <table>
      <tbody>
        {#each dls as d (d.id)}
          <tr>
            <td>
              <div class="ell" style="max-width:420px">{d.file}</div>
              <div class="tiny dim">{d.repo || ''} · {date(d.created_at)}</div>
              {#if d.error}<div class="tiny bad">{d.error}</div>{/if}
            </td>
            <td style="width:34%">
              <div class="bar"><div style="width:{d.total ? (d.done / d.total) * 100 : 0}%;background:{d.status === 'failed' ? 'var(--bad)' : d.status === 'done' ? 'var(--ok)' : 'var(--acc)'}"></div></div>
              <div class="tiny muted" style="margin-top:3px">{bytes(d.done)} / {bytes(d.total)} {d.status === 'running' ? '· ' + speed(d.speed_bps) : ''}
                {#if d.status === 'running' && d.speed_bps > 0}· {Math.round((d.total - d.done) / d.speed_bps / 60)} {t('мин')}{/if}</div>
            </td>
            <td><span class="badge {d.status === 'done' ? 'ok' : d.status === 'failed' ? 'bad' : d.status === 'running' ? 'info' : ''}">{t(d.status)}</span></td>
            <td class="nw" style="text-align:right"><div class="row" style="justify-content:flex-end">
              {#if d.status === 'running' || d.status === 'queued'}<button class="sm" onclick={() => pause(d)}>⏸</button>{/if}
              {#if d.status === 'paused' || d.status === 'failed'}<button class="sm" onclick={() => resume(d)}>▶</button>{/if}
              <button class="sm ghost" onclick={() => remove(d)}>✕</button>
            </div></td>
          </tr>
        {/each}
      </tbody>
    </table>
    {#if !dls.length}<div class="empty">{t('Загрузок нет')}</div>{/if}
  </div>
{/if}

{#if showReadme}
  <Modal title="README — {repo}" width="860px" onclose={() => (showReadme = false)}>
    {#if !readme}<span class="spin"></span>{:else}<pre style="white-space:pre-wrap;font-size:12px">{readme}</pre>{/if}
  </Modal>
{/if}
