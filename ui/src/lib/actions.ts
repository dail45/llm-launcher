import { api } from './api'
import { app } from './state.svelte'
import { t } from './i18n.svelte'
import { confirmDlg } from './dialog.svelte'

export async function startPreset(id: string, name = '') {
  try {
    const r = await api.post(`/api/presets/${id}/start`, {})
    if (r.evicted?.length) app.toast('info', t('Выгружены для освобождения памяти'), r.evicted.map((x: string) => app.instances[x]?.preset_name || x).join(', '))
    for (const w of r.warnings || []) app.toast('warning', name || t('Запуск'), w)
    return r
  } catch (e: any) {
    const m = String(e.message || '')
    if (m.includes('force') || m.includes('не поддерживает')) {
      if (await confirmDlg(t('Запустить всё равно?'), m, { ok: t('Запустить'), danger: true })) {
        try {
          return await api.post(`/api/presets/${id}/start`, { force: true })
        } catch (e2) {
          app.err(e2, t('Не удалось запустить'))
        }
      }
    } else app.err(e, t('Не удалось запустить'))
  }
}

export async function stopPreset(id: string) {
  try {
    await api.post(`/api/instances/${id}/stop`)
  } catch (e) {
    app.err(e)
  }
}

export const stateCls = (s: string) => (s === 'ready' ? 'ok' : s === 'loading' || s === 'starting' ? 'warn' : s === 'crashed' || s === 'failed' ? 'bad' : '')
export const stateTxt = (s: string) =>
  ({ ready: t('готов'), loading: t('загрузка'), starting: t('запуск'), stopping: t('остановка'), stopped: t('остановлен'), crashed: t('упал'), failed: t('ошибка') })[s] || s || '—'
export const isAlive = (s: string) => s === 'ready' || s === 'loading' || s === 'starting'
