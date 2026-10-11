<script lang="ts">
  import { onMount } from 'svelte'
  import { api } from '../lib/api'
  import { app } from '../lib/state.svelte'
  import { t } from '../lib/i18n.svelte'
  import { bytes, ago, copy } from '../lib/fmt'
  import { startPreset, stopPreset, stateCls, stateTxt, isAlive } from '../lib/actions'

  let presets = $state<any[]>([])
  let reqs = $state<any[]>([])
  let busy = $state<Record<string, boolean>>({})
  let updates = $state<any[]>([])

  const insts = $derived(Object.values(app.instances).sort((a: any, b: any) => b.started_at - a.started_at) as any[])
  const host = $derived(app.status.router_bind === '127.0.0.1' ? '127.0.0.1' : app.status.lan_ip || location.hostname)
  const routerUrl = $derived(`http://${host}:${app.status.router_port}/v1`)

  onMount(async () => {
    try {
      presets = await api.get('/api/presets')
      reqs = await api.get('/api/router/requests?limit=8')
    } catch (e) { app.err(e) }
    api.get('/api/github/updates').then((u) => (updates = u)).catch(() => {})
  })

  async function toggle(p: any) {
    busy[p.id] = true
    const i = app.instances[p.id]
    if (i && isAlive(i.state)) await stopPreset(p.id)
    else await startPreset(p.id, p.name)
    busy[p.id] = false
  }
  async function pin(i: any) {
    try { await api.post(`/api/instances/${i.id}/pin`, { pinned: !i.pinned }) } catch (e) { app.err(e) }
  }
</script>

<h1>{t('Дашборд')}</h1>

{#if updates.length}
  <div class="alert info" style="margin-bottom:12px">
    {t('Доступны обновления сборок')}: {updates.map((u) => `${u.backend_name} ${u.current} → ${u.latest}`).join(', ')} — <a href="#/backends">{t('открыть')}</a>
  </div>
{/if}

<div class="grid g3">
  {#each app.system.gpus || [] as g}
    <div class="card">
      <div class="row"><b class="ell">{g.name}</b><span class="sp"></span><span class="badge">{g.vendor}</span></div>
      <div class="big">{bytes(g.used_bytes)} <span class="dim small">/ {bytes(g.total_bytes)}</span></div>
      <div class="bar"><div style="width:{(g.used_bytes / Math.max(1, g.total_bytes)) * 100}%;background:var(--acc)"></div></div>
      <div class="row small muted" style="margin-top:6px">
        {#if g.util != null}<span>{t('Загрузка')} {Math.round(g.util)}%</span>{/if}
        {#if g.temp_c != null}<span>{g.temp_c}°C</span>{/if}
        {#if g.power_w != null}<span>{Math.round(g.power_w)} W</span>{/if}
        <span class="sp"></span><span class="dim tiny">{g.source}</span>
      </div>
    </div>
  {/each}
  <div class="card">
    <div class="row"><b>RAM</b><span class="sp"></span><span class="dim small ell">{app.system.cpu_name}</span></div>
    <div class="big">{bytes(app.system.ram_used)} <span class="dim small">/ {bytes(app.system.ram_total)}</span></div>
    <div class="bar"><div style="width:{(app.system.ram_used / Math.max(1, app.system.ram_total)) * 100}%;background:var(--ok)"></div></div>
    <div class="row small muted" style="margin-top:6px">
      <span>CPU {Math.round(app.system.cpu_usage || 0)}%</span>
      <span>{app.system.cpu_physical}C/{app.system.cpu_logical}T</span>
    </div>
  </div>
  <div class="card">
    <b>{t('Роутер (OpenAI API)')}</b>
    <div class="row" style="margin-top:8px">
      <code class="ell">{routerUrl}</code>
      <button class="sm" onclick={() => copy(routerUrl)}>{t('Копировать')}</button>
    </div>
    <div class="small muted" style="margin-top:6px">{t('Укажите алиас модели в поле model — лаунчер сам запустит нужный пресет.')}</div>
  </div>
</div>

<h2>{t('Запущено')}</h2>
{#if insts.length === 0}
  <div class="card empty">{t('Ничего не запущено. Запустите пресет ниже или отправьте запрос в роутер.')}</div>
{:else}
  <div class="cards">
    {#each insts as i (i.id)}
      <div class="card">
        <div class="row nw">
          <span class="dot {i.state}"></span>
          <b class="ell">{i.preset_name}</b>
          {#if i.alias}<code class="badge acc">{i.alias}</code>{/if}
          <span class="sp"></span>
          <span class="badge {stateCls(i.state)}">{stateTxt(i.state)}</span>
        </div>
        <div class="kv" style="margin-top:8px">
          <span>{t('Порт')}</span><span>{i.port}{i.pid ? ` · PID ${i.pid}` : ''}</span>
          {#if i.offloaded}<span>{t('Слои на GPU')}</span><span>{i.offloaded}</span>{/if}
          {#if i.vram_bytes}<span>VRAM</span><span>{bytes(i.vram_bytes)}</span>{/if}
          {#if i.ram_bytes}<span>RAM</span><span>{bytes(i.ram_bytes)}</span>{/if}
          <span>{t('Запросов')}</span><span>{i.requests_total}{i.active_requests ? ` (${t('активно')} ${i.active_requests})` : ''}</span>
          {#if i.last_tps}<span>{t('Скорость')}</span><span>{i.last_tps.toFixed(1)} tok/s</span>{/if}
          {#if isAlive(i.state)}<span>{t('Простой')}</span><span>{ago(i.last_used)}{i.ttl && !i.pinned ? ` / TTL ${Math.round(i.ttl / 60)} ${t('мин')}` : ''}</span>{/if}
        </div>
        {#if i.error_hint || i.error}<div class="alert bad small" style="margin-top:8px">{i.error_hint || i.error}</div>{/if}
        <div class="row" style="margin-top:10px">
          {#if isAlive(i.state)}
            <button class="sm" onclick={() => stopPreset(i.id)}>■ {t('Стоп')}</button>
            <button class="sm" class:primary={i.pinned} onclick={() => pin(i)} title={t('Закреплённый инстанс не выгружается по TTL и при нехватке памяти')}>📌 {i.pinned ? t('Закреплён') : t('Закрепить')}</button>
          {:else}
            <button class="sm" onclick={() => startPreset(i.id, i.preset_name)}>▶ {t('Запустить снова')}</button>
            <button class="sm ghost" onclick={() => api.del(`/api/instances/${i.id}`)}>✕</button>
          {/if}
          <span class="sp"></span>
          <a class="small" href="#/instances?id={i.id}">{t('Логи')}</a>
        </div>
      </div>
    {/each}
  </div>
{/if}

<h2>{t('Быстрый запуск')}</h2>
{#if presets.length === 0}
  <div class="card empty">
    {t('Пресетов пока нет.')} <a href="#/backends">{t('Добавьте сборку llama.cpp')}</a>, <a href="#/models">{t('папку с моделями')}</a>, {t('затем')} <a href="#/presets/new">{t('создайте пресет')}</a>.
  </div>
{:else}
  <div class="row">
    {#each presets as p (p.id)}
      {@const i = app.instances[p.id]}
      <button class="qs" class:on={i && isAlive(i.state)} disabled={busy[p.id]} onclick={() => toggle(p)}>
        {#if busy[p.id]}<span class="spin"></span>{:else}<span class="dot {i?.state || ''}"></span>{/if}
        {p.icon || ''} {p.name}
      </button>
    {/each}
  </div>
{/if}

{#if reqs.length}
  <h2>{t('Последние запросы')}</h2>
  <div class="card">
    <table>
      <thead><tr><th>{t('Время')}</th><th>{t('Модель')}</th><th>{t('Клиент')}</th><th>{t('Статус')}</th><th>{t('Токены')}</th><th>tok/s</th><th>{t('мс')}</th></tr></thead>
      <tbody>
        {#each reqs as r}
          <tr>
            <td class="small">{new Date(r.ts * 1000).toLocaleTimeString()}</td>
            <td><code>{r.alias}</code></td>
            <td class="small muted">{r.principal} · {r.ip}</td>
            <td><span class="badge {r.status < 400 ? 'ok' : 'bad'}">{r.status}</span></td>
            <td class="small">{r.prompt_tokens ?? '—'} / {r.completion_tokens ?? '—'}</td>
            <td>{r.tps ? r.tps.toFixed(1) : '—'}</td>
            <td class="small">{r.ms}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}

<style>
  .big { font-size: 20px; font-weight: 600; margin: 6px 0; }
  .qs { padding: 9px 14px; }
  .qs.on { border-color: var(--ok); }
</style>
