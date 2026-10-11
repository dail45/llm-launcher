<script lang="ts">
  import { onMount } from 'svelte'
  import { api, q } from '../lib/api'
  import { app } from '../lib/state.svelte'
  import { t } from '../lib/i18n.svelte'
  import { bytes, num, date } from '../lib/fmt'
  import { confirmDlg, promptDlg } from '../lib/dialog.svelte'
  import Modal from '../components/Modal.svelte'

  let models = $state<any[]>([])
  let dirs = $state<any[]>([])
  let presets = $state<any[]>([])
  let filter = $state('')
  let sort = $state('name')
  let onlyFav = $state(false)
  let scanning = $state(false)
  let detail = $state<any>(null)
  let edit = $state<any>(null)

  async function load() {
    try {
      ;[models, dirs, presets] = await Promise.all([api.get('/api/models'), api.get('/api/model-dirs'), api.get('/api/presets')])
    } catch (e) { app.err(e) }
  }
  onMount(load)

  const shown = $derived(
    models
      .filter((m) => (!onlyFav || m.favorite) && (!filter || (m.file_name + ' ' + m.alias + ' ' + m.tags.join(' ') + ' ' + m.info.arch).toLowerCase().includes(filter.toLowerCase())))
      .sort((a, b) => (sort === 'size' ? b.size - a.size : sort === 'date' ? b.added_at - a.added_at : a.file_name.localeCompare(b.file_name))),
  )
  const usedBy = (id: string) => presets.filter((p) => p.model_id === id)

  async function addDir() {
    const path = await promptDlg(t('Папка с моделями'), '', t('Полный путь, например D:\\models. Подпапки сканируются.'))
    if (!path) return
    try { await api.post('/api/model-dirs', { path, recursive: true, enabled: true }); await scan() } catch (e) { app.err(e) }
  }
  async function addFile() {
    const path = await promptDlg(t('Добавить файл .gguf'), '', t('Полный путь к файлу'))
    if (!path) return
    try { await api.post('/api/models/add-file', { path }); load() } catch (e) { app.err(e) }
  }
  async function delDir(d: any) {
    if (!(await confirmDlg(t('Убрать папку из списка?'), d.path + '\n' + t('Файлы не удаляются.')))) return
    try { await api.del('/api/model-dirs/' + d.id); await scan() } catch (e) { app.err(e) }
  }
  async function toggleDir(d: any) {
    try { await api.put('/api/model-dirs/' + d.id, { ...d, enabled: !d.enabled }); await scan() } catch (e) { app.err(e) }
  }
  async function scan() {
    scanning = true
    try {
      const r = await api.post('/api/models/scan')
      app.toast('success', t('Сканирование завершено'), `+${r.added} · ${t('обновлено')} ${r.updated} · ${t('не найдено')} ${r.missing}` + (r.errors.length ? '\n' + r.errors.join('\n') : ''))
      await load()
    } catch (e) { app.err(e) } finally { scanning = false }
  }
  async function open(m: any) {
    try { detail = await api.get('/api/models/' + m.id) } catch (e) { app.err(e) }
  }
  async function fav(m: any) {
    try { await api.put('/api/models/' + m.id, { favorite: !m.favorite }); m.favorite = !m.favorite } catch (e) { app.err(e) }
  }
  function startEdit(m: any) {
    edit = { id: m.id, alias: m.alias, tags: m.tags.join(', '), notes: m.notes, mmproj_id: m.mmproj_id }
  }
  async function saveEdit() {
    try {
      await api.put('/api/models/' + edit.id, { alias: edit.alias, tags: edit.tags.split(',').map((s: string) => s.trim()).filter(Boolean), notes: edit.notes, mmproj_id: edit.mmproj_id || null })
      edit = null
      load()
    } catch (e) { app.err(e) }
  }
  async function del(m: any, files: boolean) {
    const ok = await confirmDlg(files ? t('Удалить файлы модели с диска?') : t('Убрать модель из списка?'), m.paths.join('\n'), { danger: true, ok: t('Удалить') })
    if (!ok) return
    try { await api.del('/api/models/' + m.id + q({ delete_files: files })); detail = null; load() } catch (e) { app.err(e) }
  }
  const metaStr = (v: any) => (typeof v === 'object' ? JSON.stringify(v) : String(v))
</script>

<div class="row" style="margin-bottom:16px">
  <h1 style="margin:0">{t('Модели')}</h1>
  <span class="dim">{models.length}</span>
  <span class="sp"></span>
  <button onclick={addFile}>{t('Добавить файл')}</button>
  <button onclick={addDir}>+ {t('Папка')}</button>
  <button class="primary" disabled={scanning} onclick={scan}>{#if scanning}<span class="spin"></span>{/if} {t('Пересканировать')}</button>
</div>

<div class="card" style="margin-bottom:14px">
  <h3 style="margin-top:0">{t('Папки')}</h3>
  {#if dirs.length === 0}<div class="dim small">{t('Добавьте папку, где лежат .gguf — модели найдутся автоматически.')}</div>{/if}
  {#each dirs as d}
    <div class="row small" style="padding:3px 0">
      <input type="checkbox" checked={d.enabled} onchange={() => toggleDir(d)} />
      <code class="ell">{d.path}</code>
      <span class="dim">{models.filter((m) => m.dir_id === d.id).length} {t('файлов')}</span>
      <span class="sp"></span>
      <button class="ghost sm danger" onclick={() => delDir(d)}>✕</button>
    </div>
  {/each}
</div>

<div class="row" style="margin-bottom:10px">
  <input placeholder={t('Поиск по имени, архитектуре, тегам…')} bind:value={filter} style="width:300px" />
  <select bind:value={sort}><option value="name">{t('по имени')}</option><option value="size">{t('по размеру')}</option><option value="date">{t('по дате')}</option></select>
  <label class="chk small"><input type="checkbox" bind:checked={onlyFav} /> ★ {t('избранные')}</label>
</div>

<div class="card scroll" style="padding:0">
  <table>
    <thead><tr><th></th><th>{t('Файл')}</th><th>{t('Алиас')}</th><th>{t('Архитектура')}</th><th>{t('Квант')}</th><th>{t('Размер')}</th><th>ctx</th><th>{t('Пресеты')}</th><th></th></tr></thead>
    <tbody>
      {#each shown as m (m.id)}
        <tr class="click" onclick={() => open(m)}>
          <td><button class="ghost sm" onclick={(e) => { e.stopPropagation(); fav(m) }}>{m.favorite ? '★' : '☆'}</button></td>
          <td>
            <div class="ell" style="max-width:380px" title={m.paths[0]}>{m.file_name}</div>
            <div class="row tiny">
              {#if m.missing}<span class="badge bad">{t('файл не найден')}</span>{/if}
              {#if m.info.is_mmproj}<span class="badge info">mmproj</span>{/if}
              {#if m.paths.length > 1}<span class="badge">{m.paths.length} {t('частей')}</span>{/if}
              {#if m.mmproj_id}<span class="badge info">👁 vision</span>{/if}
              {#if m.info.n_expert}<span class="badge">MoE {m.info.n_expert_used}/{m.info.n_expert}</span>{/if}
              {#each m.tags as tg}<span class="badge">{tg}</span>{/each}
            </div>
          </td>
          <td><code class="small">{m.alias}</code></td>
          <td class="small">{m.info.arch} {m.info.size_label || (m.info.n_params ? num(m.info.n_params) : '')}</td>
          <td class="small">{m.info.file_type || '—'}</td>
          <td class="small">{bytes(m.size)}</td>
          <td class="small">{m.info.ctx_max ? num(m.info.ctx_max) : '—'}</td>
          <td class="small">{usedBy(m.id).length || ''}</td>
          <td>
            {#if !m.info.is_mmproj}<button class="sm" onclick={(e) => { e.stopPropagation(); app.go('/presets/new?model=' + m.id) }}>+ {t('Пресет')}</button>{/if}
          </td>
        </tr>
      {/each}
    </tbody>
  </table>
  {#if shown.length === 0}<div class="empty">{t('Моделей нет')}</div>{/if}
</div>

{#if detail}
  <Modal title={detail.file_name} width="820px" onclose={() => (detail = null)}>
    <div class="grid g2">
      <div class="kv">
        <span>{t('Алиас')}</span><code>{detail.alias}</code>
        <span>{t('Архитектура')}</span><span>{detail.info.arch}</span>
        <span>{t('Название')}</span><span>{detail.info.name || '—'}</span>
        <span>{t('Параметров')}</span><span>{num(detail.info.n_params)} {detail.info.size_label ? `(${detail.info.size_label})` : ''}</span>
        <span>{t('Квантование')}</span><span>{detail.info.file_type || '—'}</span>
        <span>{t('Размер')}</span><span>{bytes(detail.size)}</span>
        <span>{t('Макс. контекст')}</span><span>{detail.info.ctx_max || '—'}</span>
        <span>{t('Слоёв')}</span><span>{detail.info.n_layers}</span>
        <span>embd / ff</span><span>{detail.info.n_embd} / {detail.info.n_ff}</span>
        <span>{t('Головы')} (q / kv)</span><span>{detail.info.n_head} / {[...new Set(detail.info.n_head_kv)].join(',')}</span>
        {#if detail.info.n_expert}<span>{t('Эксперты')}</span><span>{detail.info.n_expert_used} / {detail.info.n_expert} ({bytes(detail.tensor_summary?.expert_bytes)})</span>{/if}
        {#if detail.info.sliding_window}<span>SWA</span><span>{detail.info.sliding_window}{detail.info.swa_pattern ? ` / ${detail.info.swa_pattern}` : ''}</span>{/if}
        {#if detail.info.kv_lora_rank}<span>MLA rank</span><span>{detail.info.kv_lora_rank}</span>{/if}
        {#if detail.info.ssm_d_state}<span>SSM state</span><span>{detail.info.ssm_d_state}</span>{/if}
        <span>{t('Добавлена')}</span><span>{date(detail.added_at)}</span>
        {#if detail.hf_repo}<span>HF</span><a href="https://huggingface.co/{detail.hf_repo}" target="_blank">{detail.hf_repo}</a>{/if}
      </div>
      <div class="col">
        <div class="small muted">{t('Файлы')}</div>
        {#each detail.paths as pth}<code class="small" style="word-break:break-all">{pth}</code>{/each}
        {#if usedBy(detail.id).length}
          <div class="small muted">{t('Используется пресетами')}</div>
          <div class="row">{#each usedBy(detail.id) as p}<a href="#/presets/{p.id}" class="badge acc" onclick={() => (detail = null)}>{p.name}</a>{/each}</div>
        {/if}
        {#if detail.notes}<div class="alert">{detail.notes}</div>{/if}
      </div>
    </div>
    {#if detail.info.chat_template}
      <details style="margin-top:12px"><summary class="small">{t('Чат-шаблон')}</summary><pre style="max-height:240px">{detail.info.chat_template}</pre></details>
    {/if}
    {#if detail.meta}
      <details style="margin-top:8px"><summary class="small">{t('Метаданные GGUF')} ({Object.keys(detail.meta).length})</summary>
        <div class="kv mono" style="max-height:300px;overflow:auto;font-size:11.5px">
          {#each Object.entries(detail.meta) as [k, v]}<span>{k}</span><span class="ell" title={metaStr(v)}>{metaStr(v)}</span>{/each}
        </div>
      </details>
    {/if}
    {#snippet footer()}
      <button class="danger" onclick={() => del(detail, true)}>{t('Удалить файлы')}</button>
      <button class="danger" onclick={() => del(detail, false)}>{t('Убрать из списка')}</button>
      <span class="sp"></span>
      <button onclick={() => { startEdit(detail); detail = null }}>{t('Изменить')}</button>
      {#if !detail.info.is_mmproj}<button class="primary" onclick={() => { const id = detail.id; detail = null; app.go('/presets/new?model=' + id) }}>+ {t('Создать пресет')}</button>{/if}
    {/snippet}
  </Modal>
{/if}

{#if edit}
  <Modal title={t('Изменить модель')} width="520px" onclose={() => (edit = null)}>
    <div class="col">
      <label class="field"><span>{t('Алиас')}</span><input bind:value={edit.alias} /></label>
      <label class="field"><span>{t('Теги (через запятую)')}</span><input bind:value={edit.tags} /></label>
      <label class="field"><span>mmproj</span>
        <select bind:value={edit.mmproj_id}><option value={null}>—</option>{#each models.filter((m) => m.info.is_mmproj) as m}<option value={m.id}>{m.file_name}</option>{/each}</select></label>
      <label class="field"><span>{t('Заметки')}</span><textarea rows="3" bind:value={edit.notes} style="font-family:inherit"></textarea></label>
    </div>
    {#snippet footer()}
      <button onclick={() => (edit = null)}>{t('Отмена')}</button>
      <button class="primary" onclick={saveEdit}>{t('Сохранить')}</button>
    {/snippet}
  </Modal>
{/if}
