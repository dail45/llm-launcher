// PWA helpers: service worker registration, install prompt, environment flags.
class Pwa {
  /** deferred `beforeinstallprompt` event (Chrome/Edge/Android) */
  prompt = $state<any>(null)
  installed = $state(false)
  standalone = $state(false)
  readonly ios = /iphone|ipad|ipod/i.test(navigator.userAgent) || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1)
  readonly secure = window.isSecureContext
  readonly tauri = '__TAURI_INTERNALS__' in window || '__TAURI__' in window
  swOk = $state(false)

  init() {
    const mq = matchMedia('(display-mode: standalone)')
    this.standalone = mq.matches || (navigator as any).standalone === true
    mq.addEventListener?.('change', (e) => (this.standalone = e.matches))
    window.addEventListener('beforeinstallprompt', (e) => {
      e.preventDefault()
      this.prompt = e
    })
    window.addEventListener('appinstalled', () => {
      this.installed = true
      this.prompt = null
    })
    if ('serviceWorker' in navigator && this.secure && !this.tauri && !import.meta.env.DEV) {
      navigator.serviceWorker
        .register('./sw.js', { scope: './' })
        .then(() => (this.swOk = true))
        .catch((e) => console.warn('service worker:', e))
    }
  }

  async install() {
    const p = this.prompt
    if (!p) return false
    p.prompt()
    const r = await p.userChoice.catch(() => null)
    this.prompt = null
    return r?.outcome === 'accepted'
  }
}

export const pwa = new Pwa()
