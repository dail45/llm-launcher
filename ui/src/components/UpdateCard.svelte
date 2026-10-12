<script lang="ts">
  import { api } from '../lib/api'
  import { app } from '../lib/state.svelte'
  import { t } from '../lib/i18n.svelte'
  import { bytes, date } from '../lib/fmt'
  let { s }: { s: any } = $props()
  let checking = $state(false)
  let jobId = $state('')
  const u = $derived(app.update || {})
  const job = $derived(jobId ? app.jobs[jobId] : null)
  const kinds: Record<string, string> = {
    nsis: 'Установленная версия (setup.exe)',
    msi: 'Установленная версия (MSI)',
    portable: 'Портативная версия (exe)',
    headless: 'Headless-сервер (launcherd.exe)',
    unsupported: 'Не Windows (Docker/Linux)',
  }
  async function check() {
    checking = true
    try { app.update = await api.post('/api/update/check') } catch (e) { app.err(e) }
    checking = false
  }
  async function install() {
    try { jobId = (await api.post('/api/update/install')).job_id } catch (e) { app.err(e) }
  }
</script>

<div class="card col">
  <h3 style="margin:0">⬆ {t('Обновление Llama Launcher')}</h3>
  <div class="kv small">
    <span>{t('Текущая версия')}</span><span><b>v{u.current || app.auth.version}</b></span>
    <span>{t('Тип установки')}</span><span>{t(kinds[u.kind] || '—')}</span>
    {#if u.checked_at}<span>{t('Проверено')}</span><span>{date(u.checked_at)}</span>{/if}
  </div>
  <div class="row">
    <label class="switch small"><input type="checkbox" bind:checked={s.app_update_check} /> {t('Проверять обновления автоматически')}</label>
    <label class="switch small"><input type="checkbox" bind:checked={s.app_update_prerelease} /> {t('Включая пре-релизы')}</label>
  </div>
  <div class="row">
    <button onclick={check} disabled={checking}>{#if checking}<span class="spin"></span>{/if} {t('Проверить сейчас')}</button>
    {#if u.error}<span class="small bad">{u.error}</span>{/if}
    {#if u.checked_at && !u.error && !u.available}<span class="small ok">✓ {t('Установлена последняя версия')}</span>{/if}
  </div>
  {#if u.available}
    <div class="alert ok col" style="gap:8px">
      <div class="row">
        <b>{t('Доступна версия')} v{u.latest}</b>
        {#if u.published_at}<span class="dim small">{u.published_at.slice(0, 10)}</span>{/if}
        <span class="sp"></span>
        <a href={u.url} target="_blank" rel="noreferrer" class="small">{t('Страница релиза')} ↗</a>
      </div>
      {#if u.notes}<pre class="small notes">{u.notes}</pre>{/if}
      {#if u.kind === 'unsupported'}
        <div class="small muted">{t('Автообновление работает только в Windows. Для Docker: docker compose pull && docker compose up -d')}</div>
      {:else if !u.asset}
        <div class="small warn">{t('В релизе нет файла для этого типа установки — скачайте вручную со страницы релиза.')}</div>
      {:else if job && job.status === 'running'}
        <div class="small">{job.message}</div>
        <div class="bar"><div style="width:{(job.progress || 0) * 100}%;background:var(--acc)"></div></div>
      {:else if job && job.status === 'done'}
        <div class="small"><span class="spin"></span> {t('Устанавливается, лаунчер перезапустится. Страница обновится сама.')}</div>
      {:else}
        {#if job && job.status === 'failed'}<div class="small bad">{job.message}</div>{/if}
        <div class="row">
          <button class="primary" onclick={install}>⬇ {t('Скачать и установить')} {u.asset_size ? `(${bytes(u.asset_size)})` : ''}</button>
          <span class="small dim">{u.kind === 'portable' || u.kind === 'headless' ? t('exe будет заменён новым, лаунчер перезапустится. Запущенные модели остановятся.') : t('Установщик обновит программу без вопросов, затем лаунчер запустится снова. Запущенные модели остановятся.')}</span>
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .notes { max-height: 220px; overflow: auto; background: var(--bg2); }
</style>
