import { api, eventsUrl, setUnauthorizedHandler, tokens } from './api'
import { t } from './i18n.svelte'

export type Toast = { id: number; level: string; title: string; message: string }

class AppState {
  ready = $state(false)
  needLogin = $state(false)
  /** launcher unreachable (PWA opened outside the LAN, PC asleep, …) */
  offline = $state(false)
  auth = $state<any>({})
  status = $state<any>({})
  system = $state<any>({ gpus: [] })
  instances = $state<Record<string, any>>({})
  jobs = $state<Record<string, any>>({})
  downloads = $state<Record<string, any>>({})
  game = $state<any>({})
  update = $state<any>({})
  toasts = $state<Toast[]>([])
  logListeners = new Set<(e: any) => void>()
  route = $state(location.hash.slice(1) || '/')
  sseOk = $state(false)
  private es: EventSource | null = null
  private tid = 1

  constructor() {
    window.addEventListener('hashchange', () => (this.route = location.hash.slice(1) || '/'))
    setUnauthorizedHandler(() => {
      this.needLogin = true
      this.es?.close()
    })
  }

  go(path: string) {
    location.hash = path
  }

  toast(level: string, title: string, message = '') {
    const id = this.tid++
    this.toasts.push({ id, level, title, message })
    setTimeout(() => this.dismiss(id), level === 'error' ? 9000 : 4500)
  }
  dismiss(id: number) {
    this.toasts = this.toasts.filter((x) => x.id !== id)
  }
  err(e: any, title = t('Ошибка')) {
    this.toast('error', title, e?.message || String(e))
  }

  async boot() {
    try {
      this.auth = await api.get('/api/auth/status')
      this.offline = false
    } catch (e) {
      this.auth = {}
      if (e instanceof TypeError) {
        // network error, not an auth problem
        this.offline = true
        this.ready = true
        return
      }
    }
    if (!this.auth.authenticated) {
      this.needLogin = true
      this.ready = true
      return
    }
    this.needLogin = false
    await this.load()
    this.ready = true
  }

  async load() {
    try {
      const [st, inst, jobs, dls, game] = await Promise.all([
        api.get('/api/status'),
        api.get('/api/instances'),
        api.get('/api/jobs'),
        api.get('/api/downloads'),
        api.get('/api/gamemode').catch(() => ({})),
      ])
      api.get('/api/update').then((u) => (this.update = u)).catch(() => {})
      this.game = game
      this.status = st
      this.instances = Object.fromEntries(inst.map((i: any) => [i.id, i]))
      this.jobs = Object.fromEntries(jobs.map((j: any) => [j.id, j]))
      this.downloads = Object.fromEntries(dls.map((d: any) => [d.id, d]))
    } catch (e) {
      this.err(e)
    }
    this.connect()
  }

  connect() {
    this.es?.close()
    const es = new EventSource(eventsUrl())
    this.es = es
    es.onopen = () => {
      this.sseOk = true
      // after a self-update the server comes back with a new version: reload the UI
      api.get('/api/version').then((v) => { if (this.auth.version && v.version && v.version !== this.auth.version) location.reload() }).catch(() => {})
    }
    es.onerror = () => {
      this.sseOk = false
      if (es.readyState === EventSource.CLOSED) {
        setTimeout(async () => {
          if (this.es !== es) return
          // token may have expired; ping an endpoint so refresh logic runs
          try { await api.get('/api/auth/status') } catch {}
          if (!this.needLogin) this.connect()
        }, 3000)
      }
    }
    es.onmessage = (m) => {
      let e: any
      try { e = JSON.parse(m.data) } catch { return }
      switch (e.type) {
        case 'system': this.system = e.snapshot; break
        case 'instance': this.instances[e.info.id] = e.info; break
        case 'instance_removed': delete this.instances[e.id]; break
        case 'job': this.jobs[e.info.id] = e.info; break
        case 'download': this.downloads[e.info.id] = e.info; break
        case 'toast': this.toast(e.level, e.title, e.message); break
        case 'game': this.game = e.info; break
        case 'update': this.update = e.info; break
        case 'log': for (const f of this.logListeners) f(e); break
      }
    }
  }

  async logout() {
    try { await api.post('/api/auth/logout') } catch {}
    tokens.clear()
    this.es?.close()
    this.needLogin = true
  }
}

export const app = new AppState()
