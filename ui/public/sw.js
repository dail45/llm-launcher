// Llama Launcher service worker: caches the app shell so the PWA opens instantly
// (and shows an offline screen). API, router and event-stream requests are never cached.
const CACHE = 'lll-shell-v1'
const SHELL = ['./', './manifest.webmanifest', './icon-192.png', './icon-512.png', './icon-maskable-512.png', './apple-touch-icon.png', './favicon-32.png']

self.addEventListener('install', (e) => {
  e.waitUntil(caches.open(CACHE).then((c) => c.addAll(SHELL)).then(() => self.skipWaiting()))
})

self.addEventListener('activate', (e) => {
  e.waitUntil(
    caches.keys().then((ks) => Promise.all(ks.filter((k) => k !== CACHE).map((k) => caches.delete(k)))).then(() => self.clients.claim()),
  )
})

// drop hashed bundles that the current index.html no longer references
async function prune(html) {
  const keep = new Set([...html.matchAll(/assets\/[\w.-]+/g)].map((m) => m[0]))
  const c = await caches.open(CACHE)
  for (const r of await c.keys()) {
    const p = new URL(r.url).pathname
    const i = p.indexOf('assets/')
    if (i >= 0 && !keep.has(p.slice(i))) await c.delete(r)
  }
}

self.addEventListener('fetch', (e) => {
  const req = e.request
  if (req.method !== 'GET') return
  const url = new URL(req.url)
  if (url.origin !== location.origin) return
  if (/\/(api|v1)\//.test(url.pathname)) return

  if (req.mode === 'navigate') {
    // network first: always get the fresh index.html, fall back to the cached shell
    e.respondWith(
      fetch(req)
        .then(async (res) => {
          if (res.ok) {
            const c = await caches.open(CACHE)
            await c.put('./', res.clone())
            res.clone().text().then(prune).catch(() => {})
          }
          return res
        })
        .catch(async () => (await caches.match('./')) || Response.error()),
    )
    return
  }

  if (url.pathname.includes('/assets/')) {
    // content-hashed → cache first
    e.respondWith(
      caches.match(req).then(
        (hit) =>
          hit ||
          fetch(req).then(async (res) => {
            if (res.ok) (await caches.open(CACHE)).put(req, res.clone())
            return res
          }),
      ),
    )
    return
  }

  // icons / manifest: stale-while-revalidate
  e.respondWith(
    caches.match(req).then((hit) => {
      const net = fetch(req)
        .then(async (res) => {
          if (res.ok) (await caches.open(CACHE)).put(req, res.clone())
          return res
        })
        .catch(() => hit || Response.error())
      return hit || net
    }),
  )
})
