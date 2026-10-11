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
        <div class="bar big" title={t('Занято другими процессами')}>
          <div style="width:{(used / total) * 100}%;background:var(--fg3);opacity:.45"></div>
          {#each parts as [k, , c]}
            <div style="width:{(d.usage[k] / total) * 100}%;background:{c}"></div>
          {/each}
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
        {#each parts as [k, , c]}
          <div style="width:{(est.cpu[k] / Math.max(est.cpu_total || est.ram_free, 1)) * 100}%;background:{c}"></div>
        {/each}
      </div>
    </div>
    <div class="row tiny">
      {#each parts as [, l, c]}<span class="row" style="gap:4px"><span class="dot" style="background:{c}"></span>{t(l)}</span>{/each}
    </div>
    <div class="row small muted">
      <span>{t('Слоёв на GPU')}: <b>{est.gpu_layers}/{est.n_layers}</b></span>
      <span>KV/{t('токен')}: {bytes(est.kv_per_token_bytes)}</span>
      <span>{t('Итог')}: <b class={fitCls(est.fit)}>{fitTxt(est.fit)}</b></span>
    </div>
    {#each est.warnings as w}<div class="alert warn small">{w}</div>{/each}
  </div>
{/if}

<style>
  .bar.big { height: 12px; margin: 4px 0 2px; }
</style>
