// Regression: Auto and No mmproj must be distinct and survive save/reload.
// Requires playwright-core; start UI then run node with NODE_PATH as needed.
const { chromium } = require('playwright-core')
const assert = require('node:assert/strict')
;(async () => {
  const browser = await chromium.launch({ executablePath: process.env.CHROMIUM || '/usr/local/bin/chromium', headless: true, args: ['--no-sandbox'] })
  try {
    for (const width of [360, 1280]) {
      const context = await browser.newContext({ viewport: { width, height: 844 }, locale: 'ru-RU' })
      const page = await context.newPage(), errors = []
      let stored = { id: 'test', name: 'Vision', model_id: 'model1', backend_id: 'backend', args: {}, extra_args: '', env: {}, devices: [], lora_ids: [], required_features: [], ttl: null, group: '', notes: '', system_prompt: '', system_prompt_append: false, mmproj_id: 'proj1', version: 1 }
      let preview = null
      const models = [
        { id: 'model1', file_name: 'model1.gguf', alias: 'model1', size: 1024, info: {}, mmproj_id: 'proj1' },
        { id: 'model2', file_name: 'model2.gguf', alias: 'model2', size: 1024, info: {}, mmproj_id: 'proj2' },
        ...['proj1', 'proj2'].map(id => ({ id, file_name: id + '.gguf', size: 512, info: { is_mmproj: true } })),
      ]
      page.on('pageerror', e => errors.push(e.message))
      await page.route('**/api/**', route => {
        const path = new URL(route.request().url()).pathname
        if (path === '/api/events') return route.fulfill({ contentType: 'text/event-stream', body: ': mock\n\n' })
        if (path === '/api/presets/test' && route.request().method() === 'PUT') {
          stored = route.request().postDataJSON()
          return route.fulfill({ json: stored })
        }
        if (path === '/api/preview-command') {
          preview = route.request().postDataJSON()
          const id = preview.mmproj_id == null ? models.find(m => m.id === preview.model_id)?.mmproj_id : preview.mmproj_id
          const cmd = `llama-server --model ${preview.model_id}.gguf` + (id && id !== 'none' ? ` --mmproj ${id}.gguf` : '')
          return route.fulfill({ json: { cmd, powershell: cmd, warnings: [] } })
        }
        const body = path === '/api/auth/status' ? { authenticated: true, local: true }
          : path === '/api/models' ? models
          : path === '/api/backends' ? [{ id: 'backend', name: 'Fake', is_default: true }]
          : path === '/api/backends/backend/schema' ? { params: [] }
          : path === '/api/presets/test' ? stored
          : path === '/api/memory/estimate' ? null
          : ['/api/instances', '/api/jobs', '/api/downloads', '/api/aliases'].includes(path) ? [] : {}
        return route.fulfill({ json: body })
      })
      const expectCommand = (model, proj) => page.waitForFunction(({ model, proj }) => {
        const text = document.querySelector('.side pre')?.textContent || ''
        return text.includes(`--model ${model}.gguf`) && (proj ? text.includes(`--mmproj ${proj}.gguf`) : !text.includes('--mmproj'))
      }, { model, proj })
      const base = process.env.UI_URL || 'http://127.0.0.1:5173'
      await page.goto(base + '/#/presets/test')
      const projector = page.getByRole('combobox', { name: 'mmproj (зрение/аудио)', exact: true })
      const model = page.getByRole('combobox', { name: /^Модель(?:\s|$)/ })
      const save = page.getByRole('button', { name: 'Сохранить', exact: true })
      await expectCommand('model1', 'proj1')
      await projector.selectOption('none')
      await expectCommand('model1', null)
      await model.selectOption('model2')
      await expectCommand('model2', null)
      assert.equal(await projector.inputValue(), 'none')
      await save.click()
      await page.getByText('Сохранено', { exact: true }).waitFor()
      assert.equal(stored.mmproj_id, 'none')
      await page.reload()
      await expectCommand('model2', null)
      assert.equal(await projector.inputValue(), 'none')
      await projector.selectOption({ label: 'Авто (из настроек модели)' })
      await expectCommand('model2', 'proj2')
      assert.equal(preview.mmproj_id, null)
      await projector.selectOption('proj1')
      await expectCommand('model2', 'proj1')
      await save.click()
      await page.getByText('Сохранено', { exact: true }).waitFor()
      await page.reload()
      await expectCommand('model2', 'proj1')
      assert.equal(await projector.inputValue(), 'proj1')
      await page.goto(base + '/#/presets/new')
      await model.waitFor()
      await model.selectOption('model1')
      await expectCommand('model1', 'proj1')
      assert.equal(preview.mmproj_id, null) // auto must not become a pinned projector ID
      await model.selectOption('model2')
      await expectCommand('model2', 'proj2')
      assert.equal(preview.mmproj_id, null)
      assert.deepEqual(errors, [])
      console.log(`PASS ${width}px: disable persists, no fallback, auto follows model, explicit projector preserved`)
      await context.close()
    }
  } finally { await browser.close() }
})().catch(e => { console.error(e); process.exitCode = 1 })
