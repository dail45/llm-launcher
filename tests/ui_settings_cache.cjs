// Regression: editing custom KV-cache quantization bits must enable Save.
// Start UI; run with playwright-core on NODE_PATH. Optional: UI_URL, CHROMIUM.
const { chromium } = require('playwright-core')
const assert = require('node:assert/strict')
;(async () => {
  const browser = await chromium.launch({ executablePath: process.env.CHROMIUM || '/usr/local/bin/chromium', headless: true, args: ['--no-sandbox'] })
  try {
    for (const width of [360, 1280]) for (const initial of [{}, { turbo3: 3.5 }]) {
      const context = await browser.newContext({ viewport: { width, height: 844 }, locale: 'ru-RU' })
      const page = await context.newPage()
      page.setDefaultTimeout(5000)
      const errors = []
      let stored = {
        language: 'ru', theme: 'dark', custom_cache_bits: initial,
        game_processes: [], game_ignore: [], allowed_subnets: [], trusted_proxies: [], cors_origins: [],
        default_ttl: 600, max_instances: 2, load_timeout_secs: 120,
        vram_margin_mib: 512, gpu_overhead_mib: 256, poll_interval_ms: 1000,
        usage_retention_days: 30, swap_mode: false, auto_restart: false, max_restart_attempts: 3,
      }
      let failSave = false, writes = 0
      page.on('pageerror', e => errors.push(e.message))
      await page.route('**/api/**', route => {
        const path = new URL(route.request().url()).pathname
        if (path === '/api/events') return route.fulfill({ contentType: 'text/event-stream', body: ': mock\n\n' })
        if (path === '/api/settings' && route.request().method() === 'PUT') {
          writes++
          if (failSave) return route.fulfill({ status: 500, json: { error: { message: 'Test save failure' } } })
          stored = route.request().postDataJSON()
          return route.fulfill({ json: { restarting: false } })
        }
        const body = path === '/api/auth/status' ? { authenticated: true, local: true }
          : path === '/api/settings' ? stored
          : path === '/api/system/autostart' ? { enabled: false }
          : ['/api/instances', '/api/jobs', '/api/downloads', '/api/aliases', '/api/keys', '/api/auth/sessions'].includes(path) ? [] : {}
        return route.fulfill({ json: body })
      })
      const waitSave = enabled => page.waitForFunction(enabled => {
        const save = [...document.querySelectorAll('button')].find(el => el.textContent.trim() === 'Сохранить')
        return save && save.disabled === !enabled
      }, enabled, { timeout: 5000 })
      await page.goto((process.env.UI_URL || 'http://127.0.0.1:5173') + '/#/settings?tab=models')
      const input = page.getByPlaceholder('turbo3=3.5', { exact: true })
      const save = page.getByRole('button', { name: 'Сохранить', exact: true })
      await input.waitFor()
      const original = Object.entries(initial).map(([k, v]) => `${k}=${v}`).join('\n')
      assert.equal(await input.inputValue(), original)
      await waitSave(false)
      await input.fill('turbo3=4.25\nnew_quant=2.75')
      await waitSave(true) // must work before blur, without touching other settings
      await input.fill(original)
      await waitSave(false)
      await input.fill('turbo3=4.25\nnew_quant=2.75')
      await waitSave(true)
      failSave = true
      await save.click()
      await page.getByText('Test save failure', { exact: true }).waitFor()
      await waitSave(true) // failed requests must retain unsaved state
      failSave = false
      await save.click()
      await waitSave(false)
      assert.deepEqual(stored.custom_cache_bits, { turbo3: 4.25, new_quant: 2.75 })
      await page.reload()
      await input.waitFor()
      assert.equal(await input.inputValue(), 'turbo3=4.25\nnew_quant=2.75')
      await waitSave(false)
      await input.fill('')
      await waitSave(true)
      await save.click()
      await waitSave(false)
      assert.deepEqual(stored.custom_cache_bits, {})
      assert.equal(writes, 3)
      assert.deepEqual(errors, [])
      console.log(`PASS ${width}px, initial ${JSON.stringify(initial)}: edit/revert, retry, save/reload, clear`)
      await context.close()
    }
  } finally { await browser.close() }
})().catch(e => { console.error(e); process.exitCode = 1 })
