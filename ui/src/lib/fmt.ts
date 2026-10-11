export function bytes(n: number | null | undefined, digits = 1): string {
  if (n == null || isNaN(n as number)) return '—'
  const u = ['Б', 'КиБ', 'МиБ', 'ГиБ', 'ТиБ']
  let i = 0
  let v = n as number
  while (Math.abs(v) >= 1024 && i < u.length - 1) { v /= 1024; i++ }
  return v.toFixed(i === 0 ? 0 : digits) + ' ' + u[i]
}
export const gib = (n: number) => (n / 1024 ** 3).toFixed(1)
export function num(n: number | null | undefined): string {
  if (n == null) return '—'
  if (n >= 1e9) return (n / 1e9).toFixed(1) + 'B'
  if (n >= 1e6) return (n / 1e6).toFixed(1) + 'M'
  if (n >= 1e3) return (n / 1e3).toFixed(1) + 'K'
  return String(n)
}
export function ago(ts: number | null | undefined): string {
  if (!ts) return '—'
  const s = Math.floor(Date.now() / 1000 - ts)
  if (s < 60) return s + ' с'
  if (s < 3600) return Math.floor(s / 60) + ' мин'
  if (s < 86400) return Math.floor(s / 3600) + ' ч'
  return Math.floor(s / 86400) + ' дн'
}
export function dur(s: number): string {
  if (s < 60) return Math.round(s) + ' с'
  if (s < 3600) return Math.floor(s / 60) + ' мин ' + Math.round(s % 60) + ' с'
  return Math.floor(s / 3600) + ' ч ' + Math.floor((s % 3600) / 60) + ' мин'
}
export function date(ts: number | null | undefined): string {
  if (!ts) return '—'
  const ms = ts > 1e12 ? ts : ts * 1000
  return new Date(ms).toLocaleString()
}
export function time(ts: number): string {
  const ms = ts > 1e12 ? ts : ts * 1000
  return new Date(ms).toLocaleTimeString()
}
export function speed(bps: number): string {
  return bytes(bps) + '/с'
}
export function pct(a: number, b: number): number {
  return b > 0 ? Math.min(100, Math.max(0, (a / b) * 100)) : 0
}
export async function copy(text: string) {
  try { await navigator.clipboard.writeText(text) } catch {
    const ta = document.createElement('textarea'); ta.value = text; document.body.appendChild(ta); ta.select(); document.execCommand('copy'); ta.remove()
  }
}
