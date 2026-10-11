<script lang="ts">
  import { onMount, tick } from 'svelte'
  import { api, raw } from '../lib/api'
  import { app } from '../lib/state.svelte'
  import { t } from '../lib/i18n.svelte'
  import { copy } from '../lib/fmt'

  type Msg = { role: string; content: string; reasoning?: string; images?: string[]; stats?: string; error?: string; thinkOpen?: boolean; thinking?: boolean }
  type Conv = { id: string; title: string; model: string; system: string; messages: Msg[] }

  let models = $state<any[]>([])
  let convs = $state<Conv[]>(JSON.parse(localStorage.getItem('lll_chats') || '[]'))
  let cur = $state<Conv | null>(null)
  let input = $state('')
  let images = $state<string[]>([])
  let busy = $state(false)
  let temp = $state(0.7)
  let maxTokens = $state(0)
  let showSettings = $state(false)
  let ctrl: AbortController | null = null
  let box = $state<HTMLDivElement>()
  let fileEl = $state<HTMLInputElement>()

  onMount(async () => {
    try { models = (await api.get('/api/router/models')).data } catch (e) { app.err(e) }
    cur = convs[0] || newConv()
  })
  function newConv(): Conv {
    const c: Conv = { id: Math.random().toString(36).slice(2), title: t('Новый чат'), model: cur?.model || models.find((m) => m.meta?.loaded)?.id || models[0]?.id || '', system: '', messages: [] }
    convs.unshift(c)
    cur = c
    persist()
    return c
  }
  function persist() {
    localStorage.setItem('lll_chats', JSON.stringify(convs.slice(0, 50).map((c) => ({ ...c, messages: c.messages.map((m) => ({ ...m, thinking: false })) }))))
  }
  function delConv(c: Conv) {
    convs = convs.filter((x) => x.id !== c.id)
    cur = convs[0] || newConv()
    persist()
  }
  async function scroll() {
    await tick()
    if (box && box.scrollHeight - box.scrollTop - box.clientHeight < 200) box.scrollTop = box.scrollHeight
  }
  async function pickImages(e: Event) {
    for (const f of Array.from((e.target as HTMLInputElement).files || [])) {
      const url = await new Promise<string>((res) => { const r = new FileReader(); r.onload = () => res(r.result as string); r.readAsDataURL(f) })
      images = [...images, url]
    }
    ;(e.target as HTMLInputElement).value = ''
  }

  function payloadMessages(c: Conv) {
    const out: any[] = []
    if (c.system.trim()) out.push({ role: 'system', content: c.system })
    for (const m of c.messages) {
      if (m.error) continue
      if (m.images?.length) out.push({ role: m.role, content: [{ type: 'text', text: m.content }, ...m.images.map((u) => ({ type: 'image_url', image_url: { url: u } }))] })
      else out.push({ role: m.role, content: m.content })
    }
    return out
  }

  async function send() {
    if (!cur || busy || (!input.trim() && !images.length)) return
    if (!cur.model) return app.toast('error', t('Выберите модель'))
    const c = cur
    c.messages.push({ role: 'user', content: input, images: images.length ? images : undefined })
    if (c.messages.length === 1) c.title = input.slice(0, 40) || t('Изображение')
    input = ''
    images = []
    await generate(c)
  }
  async function regenerate() {
    if (!cur || busy) return
    while (cur.messages.length && cur.messages[cur.messages.length - 1].role === 'assistant') cur.messages.pop()
    await generate(cur)
  }

  async function generate(c: Conv) {
    busy = true
    c.messages.push({ role: 'assistant', content: '', reasoning: '', thinking: true, thinkOpen: true })
    const m = c.messages[c.messages.length - 1]
    scroll()
    ctrl = new AbortController()
    const started = performance.now()
    let firstTok = 0
    let nTok = 0
    let timings: any = null
    let inThink = false
    try {
      const body: any = { model: c.model, stream: true, messages: payloadMessages({ ...c, messages: c.messages.slice(0, -1) }), temperature: temp, stream_options: { include_usage: true } }
      if (maxTokens > 0) body.max_tokens = maxTokens
      const r = await raw('/api/v1/chat/completions', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body), signal: ctrl.signal })
      if (!r.ok || !r.body) {
        const tx = await r.text()
        let msg = tx
        try { msg = JSON.parse(tx).error?.message || tx } catch {}
        throw new Error(msg || r.statusText)
      }
      const reader = r.body.getReader()
      const dec = new TextDecoder()
      let buf = ''
      while (true) {
        const { done, value } = await reader.read()
        if (done) break
        buf += dec.decode(value, { stream: true })
        let idx
        while ((idx = buf.indexOf('\n')) >= 0) {
          const line = buf.slice(0, idx).trim()
          buf = buf.slice(idx + 1)
          if (!line.startsWith('data:')) continue
          const data = line.slice(5).trim()
          if (data === '[DONE]') continue
          let j: any
          try { j = JSON.parse(data) } catch { continue }
          if (j.timings) timings = j.timings
          if (j.error) throw new Error(j.error.message || JSON.stringify(j.error))
          const d = j.choices?.[0]?.delta
          if (!d) continue
          // reasoning streamed separately (--reasoning-format deepseek) …
          const rc = d.reasoning_content ?? d.reasoning
          if (rc) { m.reasoning += rc; nTok++; if (!firstTok) firstTok = performance.now() }
          // … or inline <think> tags inside content
          if (d.content) {
            if (!firstTok) firstTok = performance.now()
            nTok++
            let s: string = d.content
            while (s.length) {
              if (inThink) {
                const e = s.indexOf('</think>')
                if (e < 0) { m.reasoning += s; s = '' } else { m.reasoning += s.slice(0, e); s = s.slice(e + 8); inThink = false }
              } else {
                const b = s.indexOf('<think>')
                if (b < 0) { m.content += s; s = '' } else { m.content += s.slice(0, b); s = s.slice(b + 7); inThink = true }
              }
            }
            if (m.content.trim() && m.thinking) { m.thinking = false; m.thinkOpen = false }
          }
          scroll()
        }
      }
    } catch (e: any) {
      if (e.name !== 'AbortError') m.error = e.message
    } finally {
      m.thinking = false
      const secs = (performance.now() - (firstTok || started)) / 1000
      if (timings?.predicted_per_second) m.stats = `${timings.predicted_n} tok · ${timings.predicted_per_second.toFixed(1)} tok/s${timings.prompt_per_second ? ' · pp ' + timings.prompt_per_second.toFixed(0) + ' tok/s' : ''}`
      else if (nTok) m.stats = `~${nTok} tok · ${(nTok / Math.max(secs, 0.01)).toFixed(1)} tok/s`
      if (firstTok) m.stats = (m.stats || '') + ` · TTFT ${((firstTok - started) / 1000).toFixed(2)} s`
      busy = false
      ctrl = null
      persist()
    }
  }
  function stop() {
    ctrl?.abort()
  }
  function blocks(s: string) {
    const parts = s.split(/```/)
    return parts.map((p, i) => (i % 2 ? { code: true, text: p.replace(/^[\w+-]*\n/, '') } : { code: false, text: p }))
  }
  function key(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); send() }
  }
</script>

<div class="chat">
  <div class="list col">
    <button class="primary" onclick={newConv}>+ {t('Новый чат')}</button>
    {#each convs as c (c.id)}
      <div class="row nw ci" class:on={cur?.id === c.id}>
        <button class="ghost" style="flex:1;justify-content:flex-start;overflow:hidden" onclick={() => (cur = c)}><span class="ell">{c.title}</span></button>
        <button class="ghost sm" onclick={() => delConv(c)}>✕</button>
      </div>
    {/each}
  </div>
  {#if cur}
    <div class="main">
      <div class="row top">
        <select bind:value={cur.model} onchange={persist} style="max-width:320px">
          {#each models as m}<option value={m.id}>{m.meta?.loaded ? '● ' : ''}{m.id}</option>{/each}
        </select>
        <span class="small dim">{models.find((m) => m.id === cur?.model)?.meta?.loaded ? t('загружена') : t('загрузится при первом запросе')}</span>
        <span class="sp"></span>
        <button class="sm" onclick={() => (showSettings = !showSettings)}>⚙ {t('Параметры')}</button>
      </div>
      {#if showSettings}
        <div class="card col" style="margin:8px 0">
          <label class="field"><span>{t('Системный промпт')}</span><textarea rows="3" bind:value={cur.system} onchange={persist} style="font-family:inherit"></textarea></label>
          <div class="row">
            <label class="row small">temperature <input type="range" min="0" max="2" step="0.05" bind:value={temp} /> {temp}</label>
            <label class="row small">max_tokens <input type="number" bind:value={maxTokens} style="width:90px" placeholder="0 = ∞" /></label>
          </div>
        </div>
      {/if}
      <div class="msgs" bind:this={box}>
        {#if !cur.messages.length}<div class="empty">{t('Напишите сообщение. Ответ и размышления модели отображаются по мере генерации.')}</div>{/if}
        {#each cur.messages as m, i}
          <div class="msg {m.role}">
            {#if m.images?.length}<div class="row">{#each m.images as u}<img src={u} alt="" class="thumb" />{/each}</div>{/if}
            {#if m.reasoning}
              <details class="think" bind:open={m.thinkOpen}>
                <summary class="small">{#if m.thinking}<span class="spin"></span> {t('Думает…')}{:else}💭 {t('Размышления')}{/if} <span class="dim">({m.reasoning.length} {t('симв.')})</span></summary>
                <div class="tt">{m.reasoning}</div>
              </details>
            {:else if m.thinking && !m.content}
              <div class="dim small"><span class="spin"></span> {t('Ожидание ответа… (модель может загружаться)')}</div>
            {/if}
            {#each blocks(m.content) as b}
              {#if b.code}<pre class="code">{b.text}</pre>{:else if b.text}<div class="tx">{b.text}</div>{/if}
            {/each}
            {#if m.error}<div class="alert bad small">{m.error}</div>{/if}
            {#if m.role === 'assistant' && !m.thinking}
              <div class="row tiny dim meta">
                {#if m.stats}<span>{m.stats}</span>{/if}
                <span class="sp"></span>
                <button class="ghost sm" onclick={() => copy(m.content)}>📋</button>
                {#if i === cur.messages.length - 1}<button class="ghost sm" onclick={regenerate}>↻</button>{/if}
              </div>
            {/if}
          </div>
        {/each}
      </div>
      {#if images.length}<div class="row" style="margin-bottom:6px">{#each images as u, k}<button class="ghost" onclick={() => (images = images.filter((_, j) => j !== k))}><img src={u} alt="" class="thumb" /></button>{/each}</div>{/if}
      <div class="row nw inp">
        <button class="ghost" title={t('Прикрепить изображение (нужен mmproj)')} onclick={() => fileEl?.click()}>🖼</button>
        <input type="file" accept="image/*" multiple hidden bind:this={fileEl} onchange={pickImages} />
        <textarea rows="2" bind:value={input} onkeydown={key} placeholder={t('Сообщение… (Enter — отправить, Shift+Enter — новая строка)')} style="font-family:inherit;font-size:14px"></textarea>
        {#if busy}<button class="danger" onclick={stop}>■ {t('Стоп')}</button>{:else}<button class="primary" onclick={send}>{t('Отправить')}</button>{/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .chat { display: grid; grid-template-columns: 220px minmax(0, 1fr); gap: 14px; height: calc(100vh - 62px); }
  .list { overflow: auto; gap: 2px; }
  .ci.on { background: var(--bg4); border-radius: 6px; }
  .main { display: flex; flex-direction: column; min-height: 0; }
  .top { margin-bottom: 8px; }
  .msgs { flex: 1; overflow: auto; display: flex; flex-direction: column; gap: 12px; padding: 4px 4px 12px; }
  .msg { max-width: 860px; padding: 10px 14px; border-radius: 12px; background: var(--bg2); border: 1px solid var(--line); }
  .msg.user { align-self: flex-end; background: color-mix(in srgb, var(--acc) 14%, var(--bg2)); }
  .tx { white-space: pre-wrap; word-break: break-word; }
  .code { margin: 6px 0; }
  .think { margin-bottom: 6px; border-left: 3px solid var(--fg3); padding-left: 10px; }
  .think summary { cursor: pointer; color: var(--fg2); }
  .tt { white-space: pre-wrap; color: var(--fg3); font-size: 13px; max-height: 360px; overflow: auto; margin-top: 4px; }
  .meta { margin-top: 6px; }
  .thumb { width: 64px; height: 64px; object-fit: cover; border-radius: 6px; }
  .inp textarea { flex: 1; }
  @media (max-width: 800px) { .chat { grid-template-columns: 1fr; } .list { display: none; } }
</style>
