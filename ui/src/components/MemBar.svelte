<script lang="ts">
  import { bytes } from '../lib/fmt'
  import { t } from '../lib/i18n.svelte'
  let { est }: { est: any } = $props()
  const parts = [
    ['weights', 'Веса', '#7c8cff'],
    ['kv', 'KV-кэш', '#3ecf8e'],
    ['compute', 'Буферы', '#f2b84b'],
    ['overhead', 'Накладные', '#a07cff'],
    ['extra', 'mmproj/draft', '#4fb3ff'],
  ] as const
  const fitCls = (f: string) => (f === 'good' ? 'ok' : f === 'tight' ? 'warn' : 'bad')
  const fitTxt = (f: string) => (f === 'good' ? t('влезает') : f === 'tight' ? t('впритык') : t('не влезает'))
  // instant tooltip (native title appears with a delay and is easy to miss on a thin bar)
  let tipText = $state('')
  let tipX = $state(0)
  let tipY = $state(0)
  function tip(node: HTMLElement, text: string) {
    let cur = text
    const show = () => {
      const r = node.getBoundingClientRect()
      tipText = cur
      tipX = Math.min(Math.max(r.left + r.width / 2, 130), window.innerWidth - 130)
      tipY = r.top
    }
    const hide = () => (tipText = '')
    node.addEventListener('mouseenter', show)
    node.addEventListener('mouseleave', hide)
    return {
      update(t: string) { cur = t; if (tipText) show() },
      destroy() { node.removeEventListener('mouseenter', show); node.removeEventListener('mouseleave', hide) },
    }
  }
  const sum = (u: any) => (u ? u.weights + u.kv + u.compute + u.overhead + u.extra : 0)
</script>

{#if est}
  <div class="col" style="gap:10px">
    {#each est.devices as d}
      {@const total = Math.max(d.total, 1)}
      {@const used = d.total - d.free}
      <div>
        <div class="row small">
          <b>{d.id}</b><span class="dim ell">{d.name}</span>
          <span class="sp"></span>
          <span>{bytes(sum(d.usage))} / {t('свободно')} {bytes(d.free)}</span>
          <span class="badge {fitCls(d.fit)}">{fitTxt(d.fit)}</span>
        </div>
        <div class="bar big">
          <div class="seg" use:tip={`${t('Уже занято (другие программы и запущенные модели)')}: ${bytes(used)}`} style="width:{(used / total) * 100}%;background:var(--fg3);opacity:.45"></div>
          {#each parts as [k, l, c]}
            {#if d.usage[k] > 0}<div class="seg" use:tip={`${t(l)}: ${bytes(d.usage[k])}`} style="width:{(d.usage[k] / total) * 100}%;background:{c}"></div>{/if}
          {/each}
          <div class="seg rest" use:tip={`${t('Останется свободно')}: ${bytes(Math.max(d.free - sum(d.usage), 0))}`}></div>
        </div>
        <div class="tiny dim">{t('Слоёв')}: {d.layers} · {t('всего')} {bytes(d.total)}</div>
      </div>
    {/each}
    <div>
      <div class="row small">
        <b>CPU / RAM</b>
        <span class="sp"></span>
        <span>{bytes(sum(est.cpu))} / {t('свободно')} {bytes(est.ram_free)}</span>
        <span class="badge {fitCls(est.cpu_fit)}">{fitTxt(est.cpu_fit)}</span>
      </div>
      <div class="bar big">
        {#each parts as [k, l, c]}
          {#if est.cpu[k] > 0}<div class="seg" use:tip={`${t(l)}: ${bytes(est.cpu[k])}`} style="width:{(est.cpu[k] / Math.max(est.cpu_total || est.ram_free, 1)) * 100}%;background:{c}"></div>{/if}
        {/each}
        <div class="seg rest" use:tip={`${t('Останется свободно')}: ${bytes(Math.max(est.ram_free - sum(est.cpu), 0))}`}></div>
      </div>
    </div>
    <div class="row tiny">
      {#each parts as [k, l, c]}
        {@const tot = est.devices.reduce((a: number, d: any) => a + (d.usage[k] || 0), 0) + (est.cpu[k] || 0)}
        <span class="row" style="gap:4px" title="{t(l)}: {bytes(tot)}"><span class="dot" style="background:{c}"></span>{t(l)}</span>
      {/each}
    </div>
    <div class="row small muted">
      <span>{t('Слоёв на GPU')}: <b>{est.gpu_layers}/{est.n_layers}</b></span>
      <span>KV/{t('токен')}: {bytes(est.kv_per_token_bytes)}</span>
      <span>{t('Итог')}: <b class={fitCls(est.fit)}>{fitTxt(est.fit)}</b></span>
    </div>
    {#each est.warnings as w}<div class="alert warn small">{w}</div>{/each}
  </div>
  {#if tipText}<div class="mtip" style="left:{tipX}px;top:{tipY}px">{tipText}</div>{/if}
{/if}

<style>
  .bar.big { height: 12px; margin: 4px 0 2px; }
  .seg { cursor: help; transition: filter .1s; }
  .seg:hover { filter: brightness(1.35); }
  .seg.rest { flex: 1; }
  .mtip { position: fixed; transform: translate(-50%, calc(-100% - 6px)); background: var(--bg4); color: var(--fg); border: 1px solid var(--line); border-radius: 6px; padding: 3px 8px; font-size: 12px; white-space: nowrap; pointer-events: none; z-index: 200; box-shadow: 0 4px 14px rgba(0, 0, 0, .35); }
</style>
