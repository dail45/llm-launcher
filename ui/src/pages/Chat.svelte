<script module lang="ts">
  // Chat state is module-level: generation keeps streaming into it while the page is closed,
  // and reopening the chat shows the live message.
  type Call = { id: string; fn: string; args: string; server_id?: string; tool?: string; server?: string; status: 'pending' | 'running' | 'done' | 'error' | 'denied'; result?: string; open?: boolean }
  type Msg = { role: string; content: string; reasoning?: string; reasoningTok?: number; images?: string[]; stats?: string; error?: string; thinkOpen?: boolean; thinking?: boolean; tool_calls?: Call[]; tool_call_id?: string }
  type Conv = { id: string; title: string; model: string; system: string; messages: Msg[] }
  let convs = $state<Conv[]>(JSON.parse(localStorage.getItem('lll_chats') || '[]'))
  let cur = $state<Conv | null>(null)
  let busy = $state(false)
  let temp = $state(0.7)
  let maxTokens = $state(0)
  let ctrl: AbortController | null = null
  let useTools = $state(localStorage.getItem('lll_chat_tools') === '1')
  let tools = $state<any[]>([])
  let toolErrors = $state<any[]>([])
  let rounds = 0
  // module-level so a stream started on a previous visit scrolls the current page
  let box = $state<HTMLDivElement>()
</script>

<script lang="ts">
  import { onMount, tick } from 'svelte'
  import { api, raw } from '../lib/api'
  import { app } from '../lib/state.svelte'
  import { t } from '../lib/i18n.svelte'
  import { copy } from '../lib/fmt'
  import { renderMarkdown, setCopyLabel } from '../lib/markdown'


  let models = $state<any[]>([])
  let input = $state('')
  let images = $state<string[]>([])
  let showSettings = $state(false)
  let fileEl = $state<HTMLInputElement>()
  let toolsLoading = $state(false)

  async function loadTools() {
    toolsLoading = true
    try { const r = await api.get('/api/mcp/tools'); tools = r.tools; toolErrors = r.errors } catch (e) { app.err(e) } finally { toolsLoading = false }
  }
  function setTools(on: boolean) {
    useTools = on
    localStorage.setItem('lll_chat_tools', on ? '1' : '0')
    if (on) loadTools()
  }

  onMount(async () => {
    try { models = (await api.get('/api/router/models')).data } catch (e) { app.err(e) }
    if (!cur || !convs.includes(cur)) cur = convs[0] || newConv()
    if (useTools) loadTools()
    scroll(true)
  })
  function newConv(): Conv {
    const c: Conv = { id: Math.random().toString(36).slice(2), title: t('Новый чат'), model: cur?.model || models.find((m) => m.meta?.loaded)?.id || models[0]?.id || '', system: '', messages: [] }
    convs.unshift(c)
    // use the proxied element from the list: assigning `c` itself would create a second, diverging proxy
    cur = convs[0]
    persist()
    return convs[0]
  }
  function persist() {
    localStorage.setItem('lll_chats', JSON.stringify(convs.slice(0, 50).map((c) => ({ ...c, messages: c.messages.map((m) => ({ ...m, thinking: false })) }))))
  }
  function delConv(c: Conv) {
    convs = convs.filter((x) => x.id !== c.id)
    cur = convs[0] || newConv()
    persist()
  }
  async function scroll(force = false) {
    await tick()
    if (box && box.isConnected && (force || box.scrollHeight - box.scrollTop - box.clientHeight < 200)) box.scrollTop = box.scrollHeight
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
      if (m.error && !m.tool_calls?.length) continue
      if (m.role === 'tool') { out.push({ role: 'tool', tool_call_id: m.tool_call_id, content: m.content }); continue }
      if (m.tool_calls?.length) { out.push({ role: 'assistant', content: m.content || '', tool_calls: m.tool_calls.map((x) => ({ id: x.id, type: 'function', function: { name: x.fn, arguments: x.args || '{}' } })) }); continue }
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
    rounds = 0
    await generate(c)
  }
  async function regenerate() {
    if (!cur || busy) return
    while (cur.messages.length && ['assistant', 'tool'].includes(cur.messages[cur.messages.length - 1].role)) cur.messages.pop()
    rounds = 0
    await generate(cur)
  }

  async function generate(c: Conv) {
    busy = true
    c.messages.push({ role: 'assistant', content: '', reasoning: '', reasoningTok: 0, thinking: true, thinkOpen: true })
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
      if (useTools && tools.length) body.tools = tools.map((x) => ({ type: 'function', function: { name: x.function, description: `[${x.server}] ${x.description}`.slice(0, 1024), parameters: x.input_schema } }))
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
          for (const tc of d.tool_calls || []) {
            m.tool_calls ||= []
            const k = tc.index ?? 0
            if (!m.tool_calls[k]) {
              const def = tools.find((x) => x.function === tc.function?.name)
              m.tool_calls[k] = { id: tc.id || 'call_' + Math.random().toString(36).slice(2), fn: tc.function?.name || '', args: '', server_id: def?.server_id, tool: def?.name, server: def?.server, status: 'pending' }
            }
            const cc = m.tool_calls[k]
            if (tc.function?.name && !cc.fn) cc.fn = tc.function.name
            if (tc.function?.arguments) cc.args += tc.function.arguments
            if (!firstTok) firstTok = performance.now()
            if (m.thinking) { m.thinking = false; m.thinkOpen = false }
          }
          const rc = d.reasoning_content ?? d.reasoning
          if (rc) { m.reasoning += rc; m.reasoningTok = (m.reasoningTok || 0) + 1; nTok++; if (!firstTok) firstTok = performance.now() }
          // … or inline <think> tags inside content
          if (d.content) {
            if (!firstTok) firstTok = performance.now()
            nTok++
            let s: string = d.content
            const before = m.reasoning!.length
            while (s.length) {
              if (inThink) {
                const e = s.indexOf('</think>')
                if (e < 0) { m.reasoning += s; s = '' } else { m.reasoning += s.slice(0, e); s = s.slice(e + 8); inThink = false }
              } else {
                const b = s.indexOf('<think>')
                if (b < 0) { m.content += s; s = '' } else { m.content += s.slice(0, b); s = s.slice(b + 7); inThink = true }
              }
            }
            // llama-server streams one token per chunk
            if (m.reasoning!.length > before) m.reasoningTok = (m.reasoningTok || 0) + 1
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
      if (firstTok) m.stats = (m.stats ? m.stats + ' · ' : '') + `TTFT ${((firstTok - started) / 1000).toFixed(2)} s`
      busy = false
      ctrl = null
      persist()
    }
    if (m.tool_calls?.length && !m.error) await runCalls(c, m)
  }

  async function runCalls(c: Conv, m: Msg) {
    for (const call of m.tool_calls || []) {
      const def = tools.find((x) => x.function === call.fn)
      if (!def) { call.status = 'error'; call.result = 'Error: unknown tool ' + call.fn; continue }
      Object.assign(call, { server_id: def.server_id, tool: def.name, server: def.server })
      if (def.auto_approve && call.status === 'pending') await execCall(call)
    }
    await maybeContinue(c, m)
  }
  async function execCall(call: Call) {
    call.status = 'running'
    let args: any = {}
    try { args = call.args ? JSON.parse(call.args) : {} } catch { call.status = 'error'; call.result = 'Error: invalid JSON arguments'; return }
    try {
      const r = await api.post('/api/mcp/call', { server_id: call.server_id, name: call.tool, arguments: args })
      call.result = r.text + (r.images?.length ? `\n[${r.images.length} image(s)]` : '')
      call.status = r.is_error ? 'error' : 'done'
    } catch (e: any) { call.status = 'error'; call.result = 'Error: ' + e.message }
  }
  async function decide(c: Conv, m: Msg, call: Call, ok: boolean) {
    if (ok) await execCall(call)
    else { call.status = 'denied'; call.result = 'The user declined to run this tool.' }
    await maybeContinue(c, m)
  }
  async function maybeContinue(c: Conv, m: Msg) {
    const calls = m.tool_calls || []
    if (calls.some((x) => x.status === 'pending' || x.status === 'running')) { persist(); return }
    if (c.messages[c.messages.length - 1] !== m) return
    for (const x of calls) c.messages.push({ role: 'tool', tool_call_id: x.id, content: x.result || '' })
    persist()
    if (++rounds > 10) return app.toast('error', t('Слишком много вызовов инструментов подряд'))
    await generate(c)
  }
  let showList = $state(false)
  const touch = matchMedia('(pointer: coarse)').matches
  function stop() {
    ctrl?.abort()
  }
  setCopyLabel(t('Копировать'))
  // copy buttons inside rendered code blocks (event delegation: the HTML comes from {@html})
  async function onMsgsClick(e: MouseEvent) {
    const b = (e.target as HTMLElement).closest('.cbcopy') as HTMLButtonElement | null
    if (!b) return
    const code = b.closest('.cb')?.querySelector('code')?.textContent ?? ''
    await copy(code)
    b.textContent = '✓ ' + t('Скопировано')
    b.classList.add('done')
    setTimeout(() => { b.textContent = '📋 ' + t('Копировать'); b.classList.remove('done') }, 1500)
  }
  function key(e: KeyboardEvent) {
    // on touch keyboards Enter inserts a new line; sending is done with the button
    if (e.key === 'Enter' && !e.shiftKey && !touch) { e.preventDefault(); send() }
  }
</script>

<div class="chat">
  {#if showList}<button class="scrim" aria-label="close" onclick={() => (showList = false)}></button>{/if}
  <div class="list col" class:open={showList}>
    <button class="primary" onclick={() => { newConv(); showList = false }}>+ {t('Новый чат')}</button>
    {#each convs as c (c.id)}
      <div class="row nw ci" class:on={cur?.id === c.id}>
        <button class="ghost" style="flex:1;justify-content:flex-start;overflow:hidden" onclick={() => { cur = c; showList = false }}><span class="ell">{c.title}</span></button>
        <button class="ghost sm" onclick={() => delConv(c)}>✕</button>
      </div>
    {/each}
  </div>
  {#if cur}
    <div class="main">
      <div class="row top">
        <button class="sm mob" title={t('Чаты')} onclick={() => (showList = true)}><svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor"><rect x="1" y="3" width="14" height="1.6" rx=".8"/><rect x="1" y="7.2" width="14" height="1.6" rx=".8"/><rect x="1" y="11.4" width="14" height="1.6" rx=".8"/></svg> <span class="badge">{convs.length}</span></button>
        <select bind:value={cur.model} onchange={persist} class="msel">
          {#each models as m}<option value={m.id}>{m.meta?.loaded ? '● ' : ''}{m.id}</option>{/each}
        </select>
        <span class="small dim desk">{models.find((m) => m.id === cur?.model)?.meta?.loaded ? t('загружена') : t('загрузится при первом запросе')}</span>
        <span class="sp"></span>
        <label class="switch small" title={t('Инструменты MCP-серверов (Настройки → MCP)')}><input type="checkbox" checked={useTools} onchange={(e) => setTools((e.target as HTMLInputElement).checked)} /> 🔧 <span class="desk">{t('Инструменты')}</span>
          {#if useTools}{#if toolsLoading}<span class="spin"></span>{:else}<span class="badge {tools.length ? 'ok' : ''}">{tools.length}</span>{/if}{/if}</label>
        <button class="sm" onclick={() => (showSettings = !showSettings)}>⚙ <span class="desk">{t('Параметры')}</span></button>
      </div>
      {#if useTools && !toolsLoading && (toolErrors.length || !tools.length)}
        <div class="alert warn small" style="margin-bottom:8px">
          {#if !tools.length && !toolErrors.length}{t('Нет подключённых MCP-серверов.')} <a href="#/settings">{t('Добавить в настройках')}</a>{/if}
          {#each toolErrors as e}<div><b>{e.server}</b>: {e.error}</div>{/each}
        </div>
      {/if}
      {#if showSettings}
        <div class="card col" style="margin:8px 0">
          <label class="field"><span>{t('Системный промпт')}</span><textarea rows="3" bind:value={cur.system} onchange={persist} style="font-family:inherit"></textarea>
            <span class="hint">{t('Пусто — использовать промпт пресета. Объединение со своим промптом настраивается в пресете.')}</span></label>
          <div class="row">
            <label class="row small">temperature <input type="range" min="0" max="2" step="0.05" bind:value={temp} /> {temp}</label>
            <label class="row small">max_tokens <input type="number" bind:value={maxTokens} style="width:90px" placeholder="0 = ∞" /></label>
          </div>
        </div>
      {/if}
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
      <div class="msgs" bind:this={box} onclick={onMsgsClick}>
        {#if !cur.messages.length}<div class="empty">{t('Напишите сообщение. Ответ и размышления модели отображаются по мере генерации.')}</div>{/if}
        {#each cur.messages as m, i}
          {#if m.role !== 'tool'}
          <div class="msg {m.role}">
            {#if m.images?.length}<div class="row">{#each m.images as u}<img src={u} alt="" class="thumb" />{/each}</div>{/if}
            {#if m.reasoning}
              <details class="think" bind:open={m.thinkOpen}>
                <summary class="small">{#if m.thinking}<span class="spin"></span> {t('Думает…')}{:else}💭 {t('Размышления')}{/if} <span class="dim">({m.reasoningTok ? m.reasoningTok : '≈' + Math.round(m.reasoning.length / 4)} {t('ток.')})</span></summary>
                <div class="tt">{m.reasoning}</div>
              </details>
            {:else if m.thinking && !m.content}
              <div class="dim small"><span class="spin"></span> {t('Ожидание ответа… (модель может загружаться)')}</div>
            {/if}
            {#if m.role === 'assistant'}
              {#if m.content}<div class="md">{@html renderMarkdown(m.content)}</div>{/if}
            {:else if m.content}<div class="tx">{m.content}</div>{/if}
            {#each m.tool_calls || [] as call}
              <div class="call {call.status}">
                <button class="ghost sm hdr" onclick={() => (call.open = !call.open)}>
                  🔧 <b>{call.tool || call.fn}</b>{#if call.server}<span class="dim"> · {call.server}</span>{/if}
                  <span class="dim mono ell" style="max-width:360px">{call.args}</span>
                  <span class="sp"></span>
                  {#if call.status === 'running'}<span class="spin"></span>{:else if call.status === 'done'}<span class="badge ok">✓</span>{:else if call.status === 'error'}<span class="badge bad">{t('ошибка')}</span>{:else if call.status === 'denied'}<span class="badge">{t('отклонён')}</span>{/if}
                </button>
                {#if call.status === 'pending' && !busy}
                  <div class="row small" style="margin-top:6px"><span class="muted">{t('Модель хочет вызвать инструмент. Выполнить?')}</span><span class="sp"></span>
                    <button class="sm primary" onclick={() => decide(cur!, m, call, true)}>{t('Выполнить')}</button><button class="sm" onclick={() => decide(cur!, m, call, false)}>{t('Отклонить')}</button></div>
                {/if}
                {#if call.open || call.status === 'pending'}<pre class="code small">{call.args}</pre>{/if}
                {#if call.open && call.result}<pre class="code small res">{call.result}</pre>{/if}
              </div>
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
          {/if}
        {/each}
      </div>
      {#if images.length}<div class="row" style="margin-bottom:6px">{#each images as u, k}<button class="ghost" onclick={() => (images = images.filter((_, j) => j !== k))}><img src={u} alt="" class="thumb" /></button>{/each}</div>{/if}
      <div class="row nw inp">
        <button class="ghost" title={t('Прикрепить изображение (нужен mmproj)')} onclick={() => fileEl?.click()}>🖼</button>
        <input type="file" accept="image/*" multiple hidden bind:this={fileEl} onchange={pickImages} />
        <textarea rows="2" bind:value={input} onkeydown={key} placeholder={touch ? t('Сообщение…') : t('Сообщение… (Enter — отправить, Shift+Enter — новая строка)')} style="font-family:inherit;font-size:14px"></textarea>
        {#if busy}<button class="danger" onclick={stop}>■ {t('Стоп')}</button>{:else}<button class="primary" onclick={send} title={t('Отправить')}><span class="desk">{t('Отправить')}</span><span class="mob"><svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor"><path d="M3.4 20.4 21 12 3.4 3.6 3.4 10l12.6 2-12.6 2z"/></svg></span></button>{/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .chat { display: grid; grid-template-columns: 220px minmax(0, 1fr); gap: 14px; height: var(--avail-h, calc(100vh - 62px)); }
  .msel { max-width: 320px; }
  .mob, .scrim { display: none; }
  .list { overflow: auto; gap: 2px; }
  .ci.on { background: var(--bg4); border-radius: 6px; }
  .main { display: flex; flex-direction: column; min-height: 0; min-width: 0; }
  .top { margin-bottom: 8px; }
  .msgs { flex: 1; min-width: 0; overflow: auto; display: flex; flex-direction: column; gap: 12px; padding: 4px 4px 12px; }
  .msg { min-width: 0; max-width: min(860px, 100%); overflow-wrap: anywhere; padding: 10px 14px; border-radius: 12px; background: var(--bg2); border: 1px solid var(--line); }
  .msg.user { align-self: flex-end; background: color-mix(in srgb, var(--acc) 14%, var(--bg2)); }
  .tx { white-space: pre-wrap; word-break: break-word; }
  /* rendered markdown */
  .md { min-width: 0; overflow-wrap: anywhere; word-break: break-word; line-height: 1.55; }
  .md :global(> :first-child) { margin-top: 0; }
  .md :global(> :last-child) { margin-bottom: 0; }
  .md :global(p) { margin: 0 0 .6em; }
  .md :global(h1), .md :global(h2), .md :global(h3), .md :global(h4) { margin: .9em 0 .4em; line-height: 1.3; color: var(--fg); }
  .md :global(h1) { font-size: 1.3em; } .md :global(h2) { font-size: 1.18em; } .md :global(h3) { font-size: 1.06em; } .md :global(h4) { font-size: 1em; }
  .md :global(ul), .md :global(ol) { margin: 0 0 .6em; padding-left: 1.5em; }
  .md :global(li) { margin: .15em 0; }
  .md :global(li > p) { margin: 0; }
  .md :global(blockquote) { margin: 0 0 .6em; padding: 2px 12px; border-left: 3px solid var(--line); color: var(--fg2); }
  .md :global(hr) { border: 0; border-top: 1px solid var(--line); margin: .8em 0; }
  .md :global(a) { color: var(--acc); }
  .md :global(:not(pre) > code) { background: var(--bg4); padding: 1px 5px; border-radius: 4px; font-size: .9em; }
  .md :global(table) { border-collapse: collapse; margin: 0 0 .6em; display: block; overflow-x: auto; width: max-content; max-width: 100%; }
  .md :global(th), .md :global(td) { border: 1px solid var(--line); padding: 4px 9px; }
  .md :global(th) { background: var(--bg3); color: var(--fg); }
  .md :global(img) { max-width: 100%; }
  .md :global(.cb) { min-width: 0; max-width: 100%; margin: .5em 0 .7em; border: 1px solid var(--line); border-radius: 8px; overflow: hidden; background: var(--bg); }
  .md :global(.cbh) { display: flex; align-items: center; justify-content: space-between; padding: 3px 6px 3px 10px; background: var(--bg3); border-bottom: 1px solid var(--line); font-size: 12px; color: var(--fg3); font-family: var(--mono); }
  .md :global(.cbh > span) { min-width: 0; overflow-wrap: anywhere; }
  .md :global(.cbcopy) { flex-shrink: 0; padding: 2px 8px; font-size: 12px; background: transparent; border-color: transparent; color: var(--fg2); }
  .md :global(.cbcopy:hover) { background: var(--bg4); color: var(--fg); }
  .md :global(.cbcopy.done) { color: var(--ok); }
  /* Preserve indentation/newlines but wrap even a single token longer than the screen.
     CSS-only wrapping keeps the original code intact when copied. */
  .md :global(.cb pre) { min-width: 0; max-width: 100%; margin: 0; border: 0; border-radius: 0; padding: 10px 12px; white-space: pre-wrap; overflow-wrap: anywhere; overflow-x: auto; word-break: normal; background: transparent; }
  .code { margin: 6px 0; }
  .think { margin-bottom: 6px; border-left: 3px solid var(--fg3); padding-left: 10px; }
  .think summary { cursor: pointer; color: var(--fg2); }
  .tt { white-space: pre-wrap; color: var(--fg3); font-size: 13px; max-height: 360px; overflow: auto; margin-top: 4px; }
  .meta { margin-top: 6px; }
  .thumb { width: 64px; height: 64px; object-fit: cover; border-radius: 6px; }
  .inp textarea { flex: 1; }
  .call { border: 1px solid var(--line); border-radius: 8px; padding: 4px 8px; margin: 6px 0; background: var(--bg3); }
  .call.error { border-color: color-mix(in srgb, var(--bad) 50%, var(--line)); }
  .call .hdr { width: 100%; justify-content: flex-start; gap: 6px; padding: 2px 0; }
  .call pre { max-height: 240px; overflow: auto; margin: 4px 0; white-space: pre-wrap; }
  .call .res { color: var(--fg2); }
  @media (max-width: 800px) {
    .chat { grid-template-columns: minmax(0, 1fr); }
    .list { display: none; }
    .list.open { display: flex; position: fixed; z-index: 40; left: 0; top: 0; bottom: 0; width: min(300px, 82vw); background: var(--bg2); border-right: 1px solid var(--line); padding: calc(12px + env(safe-area-inset-top)) 10px 12px; box-shadow: 0 0 40px rgba(0, 0, 0, .5); }
    .scrim { display: block; position: fixed; inset: 0; z-index: 39; background: rgba(0, 0, 0, .45); border: 0; border-radius: 0; padding: 0; }
    .mob { display: inline-flex; }
    .desk { display: none; }
    .msel { flex: 1; min-width: 0; max-width: none; }
    .top { flex-wrap: nowrap; gap: 6px; }
    .top .sp { display: none; }
    .msg { max-width: 100%; padding: 8px 11px; }
    .msg.user { max-width: calc(100% - 28px); margin-left: 28px; }
    .msgs { gap: 9px; padding: 2px 0 10px; }
    .call .hdr .mono { display: none; }
    .inp { gap: 4px; }
    .inp textarea { font-size: 16px !important; }
  }
</style>
