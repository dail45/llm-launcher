// Browser regression: preset prompt UI + persistence, with mocked launcher API.
// Start UI, then NODE_PATH=/path/to/node_modules node tests/ui_preset_prompt.cjs
// Requires playwright-core; UI_URL and CHROMIUM are optional overrides.
const { chromium } = require('playwright-core')
const assert = require('node:assert/strict')
const blank = () => ({ id: 'test', name: 'Test', model_id: 'model', backend_id: 'backend', args: {}, extra_args: '', env: {}, devices: [], lora_ids: [], required_features: [], ttl: null, group: '', notes: '', system_prompt: '', system_prompt_append: false, version: 1 })
;(async () => {
  const browser = await chromium.launch({ executablePath: process.env.CHROMIUM || '/usr/local/bin/chromium', headless: true, args: ['--no-sandbox'] })
  try {
    for (const width of [360, 1280]) {
      const context = await browser.newContext({ viewport: { width, height: 844 }, locale: 'ru-RU' })
      const page = await context.newPage()
      const errors = []
      let stored = blank(), writes = 0
      page.on('pageerror', e => errors.push(e.message))
      await page.route('**/api/**', route => {
        const path = new URL(route.request().url()).pathname
        const method = route.request().method()
        if (path === '/api/events') return route.fulfill({ contentType: 'text/event-stream', body: ': mock\n\n' })
        if ((path === '/api/presets/test' && method === 'PUT') || (path === '/api/presets' && method === 'POST')) {
          stored = { ...route.request().postDataJSON(), id: 'test' }; writes++
          return route.fulfill({ json: stored })
        }
        const body = path === '/api/auth/status' ? { authenticated: true, local: true }
          : path === '/api/models' ? [{ id: 'model', file_name: 'test.gguf', size: 1024, alias: 'test-model', info: {} }]
          : path === '/api/backends' ? [{ id: 'backend', name: 'Fake', is_default: true }]
          : path === '/api/backends/backend/schema' ? { params: [] }
          : path === '/api/presets/test' ? stored
          : path === '/api/memory/estimate' ? null
          : path === '/api/preview-command' ? { cmd: 'llama-server', powershell: 'llama-server', warnings: [] }
          : ['/api/instances', '/api/jobs', '/api/downloads', '/api/aliases'].includes(path) ? [] : {}
        return route.fulfill({ json: body })
      })
      const base = process.env.UI_URL || 'http://127.0.0.1:5173'
      await page.goto(base + '/#/presets/new')
      const prompt = page.getByRole('textbox', { name: 'Инструкции для модели', exact: true })
      const toggle = page.getByRole('checkbox', { name: 'Добавлять к промпту клиента', exact: true })
      await prompt.waitFor()
      assert.equal(await prompt.inputValue(), '')
      assert.equal(await toggle.isChecked(), false)
      await page.getByRole('textbox', { name: 'Название', exact: true }).fill('Test')
      await page.getByRole('combobox', { name: 'Модель', exact: true }).selectOption('model')
      const text = 'Ты помощник.\nОтвечай кратко.\n' + 'X'.repeat(2000)
      await prompt.fill(text)
      await toggle.check()
      await page.getByRole('button', { name: 'Сохранить', exact: true }).click()
      await page.waitForFunction(() => location.hash === '#/presets/test')
      assert.equal(stored.system_prompt, text)
      assert.equal(stored.system_prompt_append, true)
      await page.reload()
      await prompt.waitFor()
      assert.equal(await prompt.inputValue(), text)
      assert.equal(await toggle.isChecked(), true)
      await toggle.uncheck()
      await page.getByRole('button', { name: 'Сохранить', exact: true }).click()
      await page.waitForTimeout(150)
      assert.equal(stored.system_prompt_append, false)
      assert.equal(stored.system_prompt, text)
      assert.equal(writes, 2)
      assert.deepEqual(errors, [])
      const sizes = await page.evaluate(() => [document.documentElement.scrollWidth, document.documentElement.clientWidth])
      assert.ok(sizes[0] <= sizes[1] + 1, 'preset editor must fit screen')
      console.log(`PASS ${width}px: default off, prompt and mode save/reload, no overflow`)
      await context.close()
    }
  } finally { await browser.close() }
})().catch(e => { console.error(e); process.exitCode = 1 })
