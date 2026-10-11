<script lang="ts">
  import { onMount, onDestroy, tick } from 'svelte'
  import { api } from '../lib/api'
  import { app } from '../lib/state.svelte'
  import { t } from '../lib/i18n.svelte'
  import { bytes, ago, copy, time, dur } from '../lib/fmt'
  import { confirmDlg } from '../lib/dialog.svelte'
  import { startPreset, stopPreset, stateCls, stateTxt, isAlive } from '../lib/actions'

  const insts = $derived(Object.values(app.instances).sort((a: any, b: any) => b.started_at - a.started_at) as any[])
  let sel = $state<string>(new URLSearchParams(app.route.split('?')[1] || '').get('id') || '')
  let logs = $state<any[]>([])
  let follow = $state(true)
  let filter = $state('')
  let onlyErr = $state(false)
  let box = $state<HTMLDivElement>()
  const cur = $derived(app.instances[sel])

  $effect(() => {
    if (!sel && insts.length) sel = insts[0].id
  })
  $effect(() => {
    const id = sel
    if (!id) return
    logs = []
    api.get(`/api/instances/${id}/logs?tail=2000`).then((l) => { logs = l; scroll() }).catch(() => {})
  })
  const onLog = (e: any) => {
    if (e.instance !== sel) return
    logs.push({ ts: e.ts, stream: e.stream, line: e.line })
    if (logs.length > 5000) logs.splice(0, logs.length - 5000)
    scroll()
  }
  onMount(() => app.logListeners.add(onLog))
  onDestroy(() => app.logListeners.delete(onLog))
  async function scroll() {
    if (!follow) return
    await tick()
    if (box) box.scrollTop = box.scrollHeight
  }
  const errRe = /error|failed|fatal|out of memory|exception|abort/i
  const shown = $derived(logs.filter((l) => (!onlyErr || errRe.test(l.line)) && (!filter || l.line.toLowerCase().includes(filter.toLowerCase()))))

  async function stopAll() {
    if (!(await confirmDlg(t('Остановить все инстансы?'), '', { danger: true }))) return
    try { await api.post('/api/instances/stop-all') } catch (e) { app.err(e) }
  }
  async function remove(i: any) {
    try { await api.del('/api/instances/' + i.id); if (sel === i.id) sel = '' } catch (e) { app.err(e) }
  }
  function onScroll() {
    if (!box) return
    follow = box.scrollHeight - box.scrollTop - box.clientHeight < 40
  }
</script>

<div class="row" style="margin-bottom:14px">
  <h1 style="margin:0">{t('Инстансы и логи')}</h1>
  <span class="sp"></span>
  {#if insts.some((i) => isAlive(i.state))}<button class="danger" onclick={stopAll}>■ {t('Остановить все')}</button>{/if}
</div>

{#if !insts.length}
  <div class="card empty">{t('Инстансов нет. Они появятся после запуска пресета.')}</div>
{:else}
  <div class="lay">
    <div class="col" style="gap:6px">
      {#each insts as i (i.id)}
        <button class="it" class:on={sel === i.id} onclick={() => (sel = i.id)}>
          <span class="dot {i.state}"></span>
          <span class="ell" style="flex:1;text-align:left">{i.preset_name}</span>
          <span class="badge {stateCls(i.state)}">{stateTxt(i.state)}</span>
        </button>
      {/each}
    </div>
    {#if cur}
      <div class="col" style="min-width:0">
        <div class="card">
          <div class="row">
            <b>{cur.preset_name}</b>{#if cur.alias}<code class="badge acc">{cur.alias}</code>{/if}
            <span class="sp"></span>
            {#if isAlive(cur.state)}
              <button class="sm" onclick={() => stopPreset(cur.id)}>■ {t('Стоп')}</button>
              <button class="sm" onclick={async () => { await stopPreset(cur.id); await startPreset(cur.id, cur.preset_name) }}>↻ {t('Перезапуск')}</button>
            {:else}
              <button class="sm primary" onclick={() => startPreset(cur.id, cur.preset_name)}>▶ {t('Запустить')}</button>
              <button class="sm ghost" onclick={() => remove(cur)}>{t('Убрать')}</button>
            {/if}
            <a class="sm btn" href="#/presets/{cur.preset_id}">{t('Пресет')}</a>
          </div>
          <div class="kv small" style="margin-top:8px">
            <span>{t('Порт / PID')}</span><span>{cur.port} / {cur.pid ?? '—'}</span>
            <span>{t('Запущен')}</span><span>{ago(cur.started_at)} {t('назад')}{cur.ready_at ? ` · ${t('загрузка заняла')} ${dur(cur.ready_at - cur.started_at)}` : ''}</span>
            {#if cur.offloaded}<span>{t('Слои на GPU')}</span><span>{cur.offloaded}</span>{/if}
            {#if cur.vram_bytes || cur.ram_bytes}<span>{t('Память')}</span><span>VRAM {bytes(cur.vram_bytes)} · RAM {bytes(cur.ram_bytes)}</span>{/if}
            {#if Object.keys(cur.mem_estimate || {}).length}<span>{t('Оценка')}</span><span>{Object.entries(cur.mem_estimate).map(([k, v]) => `${k}: ${bytes(v as number)}`).join(' · ')}</span>{/if}
            <span>{t('Запросов / токенов')}</span><span>{cur.requests_total} / {cur.tokens_generated}{cur.last_tps ? ` · ${cur.last_tps.toFixed(1)} tok/s` : ''}</span>
            {#if cur.restarts}<span>{t('Перезапусков')}</span><span>{cur.restarts}</span>{/if}
            {#if cur.exit_code != null}<span>{t('Код выхода')}</span><span>{cur.exit_code}</span>{/if}
          </div>
          {#if cur.error_hint}<div class="alert bad small" style="margin-top:8px"><b>{t('Подсказка')}:</b> {cur.error_hint}</div>{/if}
          {#if cur.error && cur.error !== cur.error_hint}<div class="alert bad small" style="margin-top:6px">{cur.error}</div>{/if}
          <details style="margin-top:8px"><summary class="small">{t('Команда запуска')}</summary>
            <div class="row" style="margin-top:6px"><pre style="flex:1">{cur.command}</pre><button class="sm" onclick={() => copy(cur.command)}>📋</button></div></details>
        </div>
        <div class="card" style="padding:0;display:flex;flex-direction:column;min-height:0">
          <div class="row" style="padding:8px 10px;border-bottom:1px solid var(--line)">
            <input placeholder={t('Фильтр логов…')} bind:value={filter} style="width:240px" />
            <label class="chk small"><input type="checkbox" bind:checked={onlyErr} /> {t('только ошибки')}</label>
            <span class="sp"></span>
            <label class="chk small"><input type="checkbox" bind:checked={follow} onchange={() => scroll()} /> {t('автопрокрутка')}</label>
            <button class="sm" onclick={() => copy(logs.map((l) => l.line).join('\n'))}>📋</button>
          </div>
          <div class="logs mono" bind:this={box} onscroll={onScroll}>
            {#each shown as l}
              <div class="ln" class:err={errRe.test(l.line)}><span class="dim">{time(l.ts)}</span> {l.line}</div>
            {/each}
            {#if !shown.length}<div class="dim">{t('пусто')}</div>{/if}
          </div>
        </div>
      </div>
    {/if}
  </div>
{/if}

<style>
  .lay { display: grid; grid-template-columns: 240px minmax(0, 1fr); gap: 14px; align-items: start; }
  .it { width: 100%; justify-content: flex-start; }
  .it.on { border-color: var(--acc); background: var(--bg3); }
  .logs { height: calc(100vh - 420px); min-height: 300px; overflow: auto; padding: 8px 10px; font-size: 12px; background: var(--bg); border-radius: 0 0 10px 10px; }
  .ln { white-space: pre-wrap; word-break: break-all; }
  .ln.err { color: var(--bad); }
  @media (max-width: 900px) { .lay { grid-template-columns: 1fr; } }
</style>
