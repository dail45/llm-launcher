// Regression test for chat width and CSS-only code wrapping.
// Start the UI (cd ui && npm run dev -- --host 127.0.0.1), then run:
// NODE_PATH=/path/to/node_modules CHROMIUM=/path/to/chromium node tests/ui_chat_wrap.cjs
// Requires playwright-core; API responses are mocked, no launcher/model needed.
const { chromium } = require('playwright-core')
const assert = require('node:assert/strict')

const long = '0123456789abcdef'.repeat(320)
const code = `    const value = "${long}";\n\t${'word '.repeat(400)}\n  <script>alert("unsafe")</script>`
const messages = [
  { role: 'user', content: long },
  { role: 'assistant', content: long },
  { role: 'assistant', content: '`' + long + '`' },
  { role: 'assistant', content: `[link](https://example.com/${long})\n\n> ${long}\n\n- ${long}` },
  { role: 'assistant', content: '```' + 'lang'.repeat(200) + '\n' + code + '\n```', reasoning: long, thinkOpen: true },
  { role: 'assistant', content: `| A | B |\n|---|---|\n| ${long} | value |` },
]

;(async () => {
  const browser = await chromium.launch({ executablePath: process.env.CHROMIUM || '/usr/local/bin/chromium', headless: true, args: ['--no-sandbox'] })
  try {
    for (const width of [320, 360, 390, 430, 768, 800, 1280]) {
      const context = await browser.newContext({ viewport: { width, height: 844 } })
      const page = await context.newPage()
      const errors = []
      page.on('pageerror', e => errors.push(e.message))
      await page.route('**/api/**', route => {
        const path = new URL(route.request().url()).pathname
        if (path === '/api/events') return route.fulfill({ contentType: 'text/event-stream', body: ': mock\n\n' })
        const body = path === '/api/auth/status' ? { authenticated: true, local: true, version: 'test' }
          : path === '/api/router/models' ? { data: [{ id: 'test-model' }] }
          : ['/api/instances', '/api/jobs', '/api/downloads'].includes(path) ? [] : {}
        return route.fulfill({ json: body })
      })
      await page.addInitScript(({ messages }) => {
        localStorage.setItem('lll_chats', JSON.stringify([{ id: 'wrap', title: 'Wrap regression', model: 'test-model', system: '', messages }]))
        Object.defineProperty(navigator, 'clipboard', { value: { writeText: async text => { window.__copied = text } } })
      }, { messages })
      await page.goto((process.env.UI_URL || 'http://127.0.0.1:5173') + '/#/chat')
      await page.locator('.cb pre code').waitFor()
      const bounds = await page.evaluate(() => {
        const sels = ['html', 'body', 'main', '.chat', '.main', '.msgs', '.msg', '.md', '.tx', '.tt', '.cb', '.cbh', '.cb pre']
        return sels.flatMap(sel => [...document.querySelectorAll(sel)].map(el => ({ sel, client: el.clientWidth, scroll: el.scrollWidth })))
      })
      for (const b of bounds) assert.ok(b.scroll <= b.client + 1, `${width}px: ${b.sel} overflows ${b.scroll} > ${b.client}`)
      const pre = page.locator('.cb pre')
      assert.equal(await pre.evaluate(el => getComputedStyle(el).whiteSpace), 'pre-wrap')
      assert.equal(await page.locator('.cb pre code').textContent(), code)
      assert.ok(await pre.evaluate(el => el.getBoundingClientRect().height > parseFloat(getComputedStyle(el).lineHeight) * 3), 'long lines must wrap, not be clipped')
      await page.locator('.cbcopy').click()
      await page.waitForFunction(expected => window.__copied === expected, code)
      assert.deepEqual(errors, [])
      console.log(`PASS ${width}px: no horizontal overflow, code wraps, copy unchanged`)
      await context.close()
    }
  } finally { await browser.close() }
})().catch(e => { console.error(e); process.exitCode = 1 })
