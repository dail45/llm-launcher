<script lang="ts">
  import { api, tokens } from '../lib/api'
  import { app } from '../lib/state.svelte'
  import { t } from '../lib/i18n.svelte'
  let password = $state('')
  let err = $state('')
  let busy = $state(false)
  async function submit(e: Event) {
    e.preventDefault()
    busy = true
    err = ''
    try {
      const r = await api.post('/api/auth/login', { password })
      tokens.set(r.access_token, r.refresh_token)
      await app.boot()
    } catch (e: any) {
      err = e.message
    } finally {
      busy = false
    }
  }
</script>

<div class="wrap">
  <form class="card" onsubmit={submit}>
    <h1>🦙 Llama Launcher</h1>
    {#if app.auth.version}<div class="muted small" style="margin:-8px 0 8px">v{app.auth.version}</div>{/if}
    {#if !app.auth.has_password}
      <div class="alert warn">{t('Пароль не задан. Задайте его на компьютере с лаунчером (Настройки → Безопасность) или командой launcherd --set-password.')}</div>
    {:else}
      <p class="muted">{t('Удалённый доступ. Введите пароль администратора.')}</p>
      <!-- svelte-ignore a11y_autofocus -->
      <input type="password" autocomplete="current-password" placeholder={t('Пароль')} bind:value={password} autofocus />
      {#if err}<div class="bad small">{err}</div>{/if}
      <button class="primary" disabled={busy || !password}>{busy ? '…' : t('Войти')}</button>
    {/if}
  </form>
</div>

<style>
  .wrap { min-height: 100%; display: flex; align-items: center; justify-content: center; padding: 20px; }
  form { width: 360px; display: flex; flex-direction: column; gap: 12px; }
  button { justify-content: center; }
</style>
