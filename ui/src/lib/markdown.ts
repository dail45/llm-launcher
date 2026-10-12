// Markdown rendering for chat answers: marked (GFM) + DOMPurify (model output is untrusted).
import { Marked, type Tokens } from 'marked'
import DOMPurify from 'dompurify'

const esc = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')

let copyLabel = 'Копировать'
export function setCopyLabel(l: string) {
  copyLabel = l
}

const md = new Marked({
  gfm: true,
  breaks: true,
  renderer: {
    // fenced / indented code: header with the language and a copy button
    code({ text, lang }: Tokens.Code) {
      const l = (lang || '').trim().split(/\s+/)[0]
      return `<div class="cb"><div class="cbh"><span>${esc(l || 'text')}</span><button type="button" class="cbcopy">📋 ${esc(copyLabel)}</button></div><pre><code>${esc(text)}</code></pre></div>`
    },
  },
})

DOMPurify.addHook('afterSanitizeAttributes', (n) => {
  if (n.tagName === 'A') {
    n.setAttribute('target', '_blank')
    n.setAttribute('rel', 'noopener noreferrer')
  }
})

export function renderMarkdown(src: string): string {
  const html = md.parse(src || '', { async: false }) as string
  return DOMPurify.sanitize(html, { FORBID_TAGS: ['style', 'form', 'input', 'textarea', 'select', 'iframe'], FORBID_ATTR: ['style'] })
}
