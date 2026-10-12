// API client: same-origin by default (UI is served by the launcher itself).
const BASE: string = (window as any).__LLL_API__ ?? ''

export class ApiErr extends Error {
  status: number
  code?: string
  constructor(status: number, message: string, code?: string) {
    super(message)
    this.status = status
    this.code = code
  }
}

let onUnauthorized: () => void = () => {}
export function setUnauthorizedHandler(f: () => void) {
  onUnauthorized = f
}

export const tokens = {
  get access() { return localStorage.getItem('lll_access') || '' },
  get refresh() { return localStorage.getItem('lll_refresh') || '' },
  set(a: string, r: string) {
    localStorage.setItem('lll_access', a)
    localStorage.setItem('lll_refresh', r)
  },
  clear() {
    localStorage.removeItem('lll_access')
    localStorage.removeItem('lll_refresh')
  },
}

let refreshing: Promise<boolean> | null = null
async function tryRefresh(): Promise<boolean> {
  if (!tokens.refresh) return false
  if (!refreshing) {
    refreshing = (async () => {
      try {
        const r = await fetch(BASE + '/api/auth/refresh', {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ refresh_token: tokens.refresh }),
        })
        if (!r.ok) { tokens.clear(); return false }
        const t = await r.json()
        tokens.set(t.access_token, t.refresh_token)
        return true
      } catch { return false } finally { setTimeout(() => (refreshing = null), 0) }
    })()
  }
  return refreshing
}

function headers(extra?: Record<string, string>): Record<string, string> {
  const h: Record<string, string> = { ...(extra || {}) }
  if (tokens.access) h['authorization'] = 'Bearer ' + tokens.access
  return h
}

export async function raw(path: string, init: RequestInit = {}, retry = true): Promise<Response> {
  const r = await fetch(BASE + path, { ...init, headers: headers(init.headers as any) })
  if (r.status === 401 && retry && !path.startsWith('/api/auth/login')) {
    if (await tryRefresh()) return raw(path, init, false)
    onUnauthorized()
  }
  return r
}

export async function req<T = any>(method: string, path: string, body?: any): Promise<T> {
  const init: RequestInit = { method }
  if (body !== undefined) {
    init.body = JSON.stringify(body)
    init.headers = { 'content-type': 'application/json' }
  }
  const r = await raw(path, init)
  const text = await r.text()
  let data: any = null
  try { data = text ? JSON.parse(text) : null } catch { data = text }
  if (!r.ok) {
    const msg = data?.error?.message || (typeof data?.error === 'string' && data.error) || data?.message || (typeof data === 'string' && data) || r.statusText
    throw new ApiErr(r.status, msg, data?.error?.code)
  }
  return data as T
}

export const api = {
  get: <T = any>(p: string) => req<T>('GET', p),
  post: <T = any>(p: string, b: any = {}) => req<T>('POST', p, b),
  put: <T = any>(p: string, b: any) => req<T>('PUT', p, b),
  del: <T = any>(p: string) => req<T>('DELETE', p),
}

export function eventsUrl(): string {
  const t = tokens.access
  return BASE + '/api/events' + (t ? '?token=' + encodeURIComponent(t) : '')
}

export async function download(path: string, filename: string) {
  const r = await raw(path)
  if (!r.ok) throw new ApiErr(r.status, await r.text())
  const blob = await r.blob()
  const a = document.createElement('a')
  a.href = URL.createObjectURL(blob)
  a.download = filename
  a.click()
  setTimeout(() => URL.revokeObjectURL(a.href), 5000)
}

export function q(o: Record<string, any>): string {
  const p = new URLSearchParams()
  for (const [k, v] of Object.entries(o)) if (v !== undefined && v !== null && v !== '') p.set(k, String(v))
  const s = p.toString()
  return s ? '?' + s : ''
}
