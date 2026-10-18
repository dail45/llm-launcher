<script lang="ts">
  import { onMount, untrack } from 'svelte'
  import { api } from '../lib/api'
  import { app } from '../lib/state.svelte'
  import { t } from '../lib/i18n.svelte'
  import { bytes, copy, date } from '../lib/fmt'
  import { confirmDlg } from '../lib/dialog.svelte'
  import { startPreset, stopPreset, isAlive, stateTxt, stateCls } from '../lib/actions'
  import ParamField from '../components/ParamField.svelte'
  import MemBar from '../components/MemBar.svelte'
  import Modal from '../components/Modal.svelte'

  let { id }: { id: string } = $props()
  const isNew = id === 'new'

  let p = $state<any>(null)
  let models = $state<any[]>([])
  let backends = $state<any[]>([])
  let schema = $state<any>({ params: [] })
  let aliases = $state<any[]>([])
  let alias = $state('')
  let origAlias = ''
  let mem = $state<any>(null)
  let memErr = $state('')
  let cmd = $state<any>(null)
  let shell = $state<'cmd' | 'powershell'>('cmd')
  let ptab = $state('basic')
  let psearch = $state('')
  let envText = $state('')
  let saving = $state(false)
  let dirty = $state(false)
  let history = $state<any[] | null>(null)
  let ignored = $state<string[]>([])

  const backend = $derived(backends.find((b) => b.id === p?.backend_id))
  const model = $derived(models.find((m) => m.id === p?.model_id))
  const mainModels = $derived(models.filter((m) => !m.info?.is_mmproj))
  const mmprojs = $derived(models.filter((m) => m.info?.is_mmproj))
  const inst = $derived(p?.id ? app.instances[p.id] : null)
  const groups = $derived([...new Set(schema.params.map((d: any) => d.group))] as string[])
  const visible = $derived(
    schema.params.filter((d: any) => {
      if (['--model', '--host', '--port', '--alias', '--mmproj', '--model-draft', '--lora', '--api-key', '--help', '--version', '--list-devices', '--device'].includes(d.id)) return false
      if (psearch) return (d.flags.join(' ') + ' ' + d.description).toLowerCase().includes(psearch.toLowerCase())
      if (ptab === 'basic') return d.basic
      if (ptab === 'set') return p.args[d.id] !== undefined
      return d.group === ptab
    }),
  )
  const unknownArgs = $derived(p ? Object.keys(p.args).filter((k) => !schema.params.some((d: any) => d.id === k)) : [])

  function blank() {
    return { id: '', name: '', model_id: '', backend_id: '', args: {}, extra_args: '', system_prompt: '', system_prompt_append: false, env: {}, devices: [], mmproj_id: null, draft_model_id: null, lora_ids: [], ttl: null, pinned: false, autostart: false, required_features: [], group: '', icon: null, color: null, hotkey: null, order: 0, notes: '' }
  }

  onMount(async () => {
    try {
      const [m, b, a] = await Promise.all([api.get('/api/models'), api.get('/api/backends'), api.get('/api/aliases')])
      models = m.sort((x: any, y: any) => x.file_name.localeCompare(y.file_name))
      backends = b
      aliases = a
      let pr: any
      if (isNew) {
        pr = blank()
        pr.backend_id = b.find((x: any) => x.is_default)?.id || b[0]?.id || ''
        const qm = new URLSearchParams(app.route.split('?')[1] || '').get('model')
        if (qm) pr.model_id = qm
        const d = sessionStorage.getItem('lll_draft')
        if (d) {
          sessionStorage.removeItem('lll_draft')
          const r = JSON.parse(d)
          pr.backend_id = r.backend_id
          pr.args = r.imported.args || {}
          pr.extra_args = r.imported.extra_args || ''
          pr.env = r.imported.env || {}
          pr.model_id = r.model_id || ''
          pr.mmproj_id = r.mmproj_id || null
          pr.draft_model_id = r.draft_model_id || null
          pr.lora_ids = r.lora_ids || []
          ignored = r.imported.ignored || []
          if (!r.model_id && r.imported.model_path) app.toast('warning', t('Модель не найдена среди известных'), r.imported.model_path + '\n' + t('Добавьте папку с ней в «Модели» или выберите вручную.'))
        }
        const mm = models.find((x) => x.id === pr.model_id)
        if (mm) pr.name = mm.alias
      } else {
        pr = await api.get('/api/presets/' + id)
        const pa = a.find((x: any) => x.target_type === 'preset' && x.target_id === id && x.is_primary)
        alias = origAlias = pa?.name || ''
      }
      if (pr.mmproj_id === '') pr.mmproj_id = 'none'
      envText = Object.entries(pr.env || {}).map(([k, v]) => `${k}=${v}`).join('\n')
      p = pr
    } catch (e) { app.err(e) }
  })

  // load schema when backend changes
  $effect(() => {
    const bid = p?.backend_id
    if (!bid) return
    api.get(`/api/backends/${bid}/schema`).then((s) => (schema = s)).catch(() => (schema = { params: [] }))
  })

  // debounced estimate + command preview
  let timer: any
  $effect(() => {
    if (!p) return
    const snap = JSON.stringify(p)
    untrack(() => {
      clearTimeout(timer)
      timer = setTimeout(() => refresh(JSON.parse(snap)), 350)
    })
  })
  async function refresh(pr: any) {
    if (!pr.model_id || !pr.backend_id) { mem = null; cmd = null; return }
    try { mem = await api.post('/api/memory/estimate', { preset: pr }); memErr = '' } catch (e: any) { memErr = e.message; mem = null }
    try { cmd = await api.post('/api/preview-command', pr) } catch { cmd = null }
  }

  function keyFor(flag: string): string {
    const d = schema.params.find((x: any) => x.id === flag || x.flags.includes(flag) || x.neg_flag === flag)
    return d?.id || flag
  }
  function setArg(k: string, v: any) {
    if (v === undefined) delete p.args[k]
    else p.args[k] = v
    dirty = true
  }
  function applyFit(kind: string) {
    const a = mem?.autofit
    if (!a) return
    if (kind === 'ngl' && a.ngl != null) setArg(keyFor('--n-gpu-layers'), a.ngl)
    if (kind === 'ctx' && a.ctx != null) setArg(keyFor('--ctx-size'), a.ctx)
    if (kind === 'moe' && a.n_cpu_moe != null) setArg(keyFor('--n-cpu-moe'), a.n_cpu_moe)
    if (kind === 'split' && a.tensor_split) setArg(keyFor('--tensor-split'), a.tensor_split.map((x: number) => +x.toFixed(2)).join(','))
  }
  function onModel() {
    if (!p.name && model) p.name = model.alias
    dirty = true
  }
  function parseEnv() {
    const env: Record<string, string> = {}
    for (const line of envText.split('\n')) {
      const i = line.indexOf('=')
      if (i > 0) env[line.slice(0, i).trim()] = line.slice(i + 1).trim()
    }
    p.env = env
  }
  function toggleDev(d: string) {
    p.devices = p.devices.includes(d) ? p.devices.filter((x: string) => x !== d) : [...p.devices, d]
    dirty = true
  }

  async function save(andStart = false) {
    if (!p.name.trim()) return app.toast('error', t('Укажите название'))
    if (!p.model_id) return app.toast('error', t('Выберите модель'))
    if (!p.backend_id) return app.toast('error', t('Выберите сборку'))
    parseEnv()
    saving = true
    try {
      const saved = isNew ? await api.post('/api/presets', p) : await api.put('/api/presets/' + p.id, p)
      const a = alias.trim()
      if (a && a !== origAlias) {
        await api.post('/api/aliases', { name: a, target_type: 'preset', target_id: saved.id, is_primary: true })
        origAlias = a
      } else if (!a && origAlias) {
        await api.del('/api/aliases?name=' + encodeURIComponent(origAlias))
        origAlias = ''
      }
      p = saved
      dirty = false
      app.toast('success', t('Сохранено'), inst && isAlive(inst.state) ? t('Перезапустите пресет, чтобы применить изменения') : '')
      if (isNew) app.go('/presets/' + saved.id)
      if (andStart) {
        if (inst && isAlive(inst.state)) await stopPreset(saved.id)
        await startPreset(saved.id, saved.name)
      }
    } catch (e) { app.err(e, t('Не удалось сохранить')) } finally { saving = false }
  }
  async function del() {
    if (!(await confirmDlg(t('Удалить пресет?'), p.name, { danger: true, ok: t('Удалить') }))) return
    try { await api.del('/api/presets/' + p.id); app.go('/presets') } catch (e) { app.err(e) }
  }
  async function showHistory() {
    try { history = await api.get(`/api/presets/${p.id}/history`) } catch (e) { app.err(e) }
  }
  async function restore(v: number) {
    try { p = await api.post(`/api/presets/${p.id}/restore/${v}`); history = null; app.toast('success', t('Версия восстановлена')) } catch (e) { app.err(e) }
  }
  async function calibrate() {
    try { const r = await api.post(`/api/presets/${p.id}/calibrate`); app.toast('success', t('Калибровка сохранена'), r.message || '') ; refresh(p) } catch (e) { app.err(e) }
  }
</script>

<svelte:window onbeforeunload={(e) => { if (dirty) e.preventDefault() }} />

{#if !p}
  <div class="empty"><span class="spin"></span></div>
{:else}
  <div class="row" style="margin-bottom:14px">
    <a href="#/presets" class="muted">← {t('Пресеты')}</a>
    <h1 style="margin:0 8px">{isNew ? t('Новый пресет') : p.name}</h1>
    {#if inst}<span class="badge {stateCls(inst.state)}">{stateTxt(inst.state)}</span>{/if}
    {#if !isNew}<span class="dim small">v{p.version}</span>{/if}
    <span class="sp"></span>
    {#if !isNew}
      <button onclick={showHistory}>{t('История')}</button>
      {#if inst && isAlive(inst.state)}
        <button onclick={calibrate} title={t('Сравнить оценку с фактическим потреблением VRAM и уточнить коэффициент')}>{t('Калибровать')}</button>
        <button onclick={() => stopPreset(p.id)}>■ {t('Стоп')}</button>
      {/if}
      <button class="danger" onclick={del}>{t('Удалить')}</button>
    {/if}
    <button disabled={saving} onclick={() => save(false)}>{t('Сохранить')}</button>
    <button class="primary" disabled={saving} onclick={() => save(true)}>{inst && isAlive(inst.state) ? t('Сохранить и перезапустить') : t('Сохранить и запустить')}</button>
  </div>

  {#if ignored.length}<div class="alert warn" style="margin-bottom:12px">{t('Не распознано при импорте (перенесено в доп. аргументы)')}: <code>{ignored.join(' ')}</code></div>{/if}

  <div class="lay">
    <div class="col" style="gap:14px;min-width:0">
      <div class="card">
        <h3 style="margin-top:0">{t('Основное')}</h3>
        <div class="grid g2">
          <label class="field"><span>{t('Название')}</span><input bind:value={p.name} oninput={() => (dirty = true)} /></label>
          <label class="field"><span>{t('Алиас для API (model)')}</span><input bind:value={alias} placeholder={model?.alias || 'qwen3-30b'} oninput={() => (dirty = true)} />
            <span class="hint">{t('Имя, по которому клиенты обращаются к модели через роутер')}</span></label>
          <label class="field"><span>{t('Модель')}</span>
            <select bind:value={p.model_id} onchange={onModel}>
              <option value="">—</option>
              {#each mainModels as m}<option value={m.id}>{m.file_name} ({bytes(m.size)}){m.missing ? ' ⚠' : ''}</option>{/each}
            </select>
            {#if model}<span class="hint">{model.info.arch} · {model.info.size_label || ''} · {model.info.file_type || ''} · ctx max {model.info.ctx_max || '?'} · {model.info.n_layers} {t('слоёв')}{model.info.n_expert ? ` · MoE ${model.info.n_expert_used}/${model.info.n_expert}` : ''}</span>{/if}
          </label>
          <label class="field"><span>{t('Сборка llama.cpp')}</span>
            <select bind:value={p.backend_id} onchange={() => (dirty = true)}>
              {#each backends as b}<option value={b.id}>{b.name} {b.version ? `(${b.version})` : ''}</option>{/each}
            </select>
            {#if backend?.features?.filter((f: string) => !f.startsWith('--')).length}<span class="hint">{t('Особенности')}: {backend.features.filter((f: string) => !f.startsWith('--')).join(', ')}</span>{/if}
          </label>
          <label class="field"><span>mmproj ({t('зрение/аудио')})</span>
            <select bind:value={p.mmproj_id} onchange={() => (dirty = true)}>
              <option value={null}>{t('Авто (из настроек модели)')}</option>
              <option value="none">— {t('Без mmproj')}</option>
              {#each mmprojs as m}<option value={m.id}>{m.file_name}</option>{/each}
            </select></label>
          <label class="field"><span>{t('Draft-модель (speculative)')}</span>
            <select bind:value={p.draft_model_id} onchange={() => (dirty = true)}>
              <option value={null}>—</option>
              {#each mainModels.filter((m) => m.id !== p.model_id) as m}<option value={m.id}>{m.file_name}</option>{/each}
            </select></label>
        </div>
        {#if backend?.devices?.length}
          <div class="field" style="margin-top:10px"><span>{t('Устройства (пусто — все)')}</span>
            <div class="row">
              {#each backend.devices as d}
                <label class="chk dev"><input type="checkbox" checked={p.devices.includes(d.id)} onchange={() => toggleDev(d.id)} /> <b>{d.id}</b> <span class="dim small">{d.name}{d.total_mib ? ` · ${(d.total_mib / 1024).toFixed(0)} ГиБ` : ''}</span></label>
              {/each}
            </div>
          </div>
        {/if}
      </div>

      <div class="card col">
        <h3 style="margin:0">{t('Системный промпт')}</h3>
        <label class="field"><span>{t('Инструкции для модели')}</span>
          <textarea rows="5" bind:value={p.system_prompt} oninput={() => (dirty = true)} placeholder={t('Пусто — не добавлять инструкции пресета')} style="font-family:inherit"></textarea>
        </label>
        <label class="switch"><input type="checkbox" bind:checked={p.system_prompt_append} onchange={() => (dirty = true)} /> {t('Добавлять к промпту клиента')}</label>
        <div class="hint small muted">{p.system_prompt_append
          ? t('Сначала инструкции пресета, затем системный промпт чата или API-клиента. Оба сохраняются.')
          : t('Промпт пресета используется, только если чат или API-клиент не передал свой системный промпт.')}</div>
        <div class="small dim">{t('Применяется к следующему запросу через лаунчер без перезапуска модели.')}</div>
      </div>

      <div class="card">
        <div class="row">
          <h3 style="margin:0">{t('Параметры llama-server')}</h3>
          <span class="dim small">{schema.params.length} {t('флагов из --help')}</span>
          <span class="sp"></span>
          <input placeholder={t('Поиск параметра…')} bind:value={psearch} style="width:220px" />
        </div>
        <div class="tabs" style="margin-top:10px">
          <button class:on={ptab === 'basic'} onclick={() => (ptab = 'basic')}>{t('Основные')}</button>
          <button class:on={ptab === 'set'} onclick={() => (ptab = 'set')}>{t('Заданные')} ({Object.keys(p.args).length})</button>
          {#each groups as g}<button class:on={ptab === g} onclick={() => (ptab = g)}>{t(g)}</button>{/each}
        </div>
        {#if schema.params.length === 0}<div class="alert warn">{t('Схема параметров недоступна — пересканируйте сборку.')}</div>{/if}
        <div class="params">
          {#each visible as d (d.id)}
            <ParamField def={d} value={p.args[d.id]} onchange={(v) => setArg(d.id, v)} />
          {/each}
        </div>
        {#if unknownArgs.length}
          <div class="alert warn small" style="margin-top:8px">{t('Флаги не поддерживаются этой сборкой')}: {#each unknownArgs as k}<code>{k}</code> <button class="ghost sm" onclick={() => setArg(k, undefined)}>✕</button>{/each}</div>
        {/if}
        <div class="grid g2" style="margin-top:12px">
          <label class="field"><span>{t('Дополнительные аргументы')}</span><textarea rows="3" bind:value={p.extra_args} oninput={() => (dirty = true)} placeholder="--some-new-flag 1"></textarea>
            <span class="hint">{t('Добавляются в конец команды как есть')}</span></label>
          <label class="field"><span>{t('Переменные окружения')}</span><textarea rows="3" bind:value={envText} onchange={parseEnv} placeholder="GGML_CUDA_ENABLE_UNIFIED_MEMORY=1"></textarea></label>
        </div>
      </div>

      <div class="card">
        <h3 style="margin-top:0">{t('Поведение')}</h3>
        <div class="grid g3">
          <label class="field"><span>{t('Выгружать при простое, сек')}</span><input type="number" min="0" value={p.ttl ?? ''} placeholder={t('по умолчанию')} onchange={(e) => { const v = e.currentTarget.value; p.ttl = v === '' ? null : +v; dirty = true }} />
            <span class="hint">0 — {t('никогда')}</span></label>
          <label class="field"><span>{t('Группа')}</span><input bind:value={p.group} oninput={() => (dirty = true)} /></label>
          <label class="field"><span>{t('Иконка / цвет')}</span>
            <div class="row nw"><input style="width:60px" bind:value={p.icon} placeholder="🧠" /><input type="color" value={p.color || '#7c8cff'} onchange={(e) => { p.color = e.currentTarget.value; dirty = true }} style="width:44px;padding:2px" />
            {#if p.color}<button class="ghost sm" onclick={() => (p.color = null)}>✕</button>{/if}</div></label>
        </div>
        <div class="row" style="margin-top:10px;gap:18px">
          <label class="chk"><input type="checkbox" bind:checked={p.pinned} /> {t('Закреплён (не выгружать)')}</label>
          <label class="chk"><input type="checkbox" bind:checked={p.autostart} /> {t('Запускать при старте лаунчера')}</label>
        </div>
        <label class="field" style="margin-top:10px"><span>{t('Заметки')}</span><textarea rows="2" bind:value={p.notes} style="font-family:inherit"></textarea></label>
      </div>
    </div>

    <div class="side col" style="gap:14px">
      <div class="card">
        <div class="row"><h3 style="margin:0">{t('Память')}</h3><span class="sp"></span>{#if mem?.running}<span class="badge info">{t('запущен')}</span>{/if}</div>
        {#if memErr}<div class="alert bad small" style="margin-top:8px">{memErr}</div>{/if}
        {#if mem}
          <div style="margin-top:10px"><MemBar est={mem.estimate} /></div>
          <div class="row" style="margin-top:12px">
            {#if mem.autofit.ngl != null}<button class="sm" onclick={() => applyFit('ngl')}>-ngl {mem.autofit.ngl}</button>{/if}
            {#if mem.autofit.ctx != null}<button class="sm" onclick={() => applyFit('ctx')}>{t('макс.')} ctx {mem.autofit.ctx}</button>{/if}
            {#if mem.autofit.n_cpu_moe != null}<button class="sm" onclick={() => applyFit('moe')}>--n-cpu-moe {mem.autofit.n_cpu_moe}</button>{/if}
            {#if mem.autofit.tensor_split}<button class="sm" onclick={() => applyFit('split')}>split {mem.autofit.tensor_split.map((x: number) => x.toFixed(2)).join(',')}</button>{/if}
          </div>
          <div class="tiny dim" style="margin-top:6px">{t('Кнопки подбирают максимум, который влезает с запасом.')}</div>
        {:else if !memErr}<div class="dim small" style="margin-top:8px">{t('Выберите модель и сборку')}</div>{/if}
      </div>
      {#if cmd}
        <div class="card">
          <div class="row">
            <h3 style="margin:0">{t('Команда')}</h3><span class="sp"></span>
            <select bind:value={shell} style="padding:2px 6px"><option value="cmd">cmd</option><option value="powershell">PowerShell</option></select>
            <button class="sm" onclick={() => { copy(cmd[shell]); app.toast('success', t('Скопировано')) }}>📋</button>
          </div>
          <pre style="margin-top:8px;max-height:260px">{cmd[shell]}</pre>
          {#each cmd.warnings as w}<div class="alert warn small" style="margin-top:6px">{w}</div>{/each}
        </div>
      {/if}
      {#if inst?.error_hint || inst?.error}<div class="alert bad small">{inst.error_hint || inst.error}</div>{/if}
    </div>
  </div>
{/if}

{#if history}
  <Modal title={t('История версий')} onclose={() => (history = null)}>
    {#if history.length === 0}<div class="empty">{t('Нет сохранённых версий')}</div>{/if}
    <table>
      <tbody>
        {#each history as h}
          <tr>
            <td>v{h.version}</td><td class="small muted">{date(h.updated_at)}</td>
            <td class="small mono ell" style="max-width:320px">{Object.entries(h.args).map(([k, v]) => `${k} ${v}`).join(' ')}</td>
            <td><button class="sm" onclick={() => restore(h.version)}>{t('Восстановить')}</button></td>
          </tr>
        {/each}
      </tbody>
    </table>
  </Modal>
{/if}

<style>
  .lay { display: grid; grid-template-columns: minmax(0, 1fr) 380px; gap: 14px; align-items: start; }
  .side { position: sticky; top: 0; }
  .params { display: grid; grid-template-columns: repeat(auto-fill, minmax(260px, 1fr)); gap: 4px; max-height: 620px; overflow: auto; }
  .dev { padding: 6px 10px; border: 1px solid var(--line); border-radius: 7px; }
  @media (max-width: 1100px) { .lay { grid-template-columns: 1fr; } .side { position: static; } }
</style>
