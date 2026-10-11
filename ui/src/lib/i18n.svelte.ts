// Keys are Russian strings; English is a lookup table. Missing keys fall back to Russian.
import { EN } from './en'

export const lang = $state({ v: localStorage.getItem('lll_lang') || (navigator.language.startsWith('ru') ? 'ru' : 'en') })

export function setLang(l: string) {
  lang.v = l
  localStorage.setItem('lll_lang', l)
  document.documentElement.lang = l
}

export function t(key: string, vars?: Record<string, any>): string {
  let s = lang.v === 'en' ? (EN[key] ?? key) : key
  if (vars) for (const [k, v] of Object.entries(vars)) s = s.replaceAll('{' + k + '}', String(v))
  return s
}
