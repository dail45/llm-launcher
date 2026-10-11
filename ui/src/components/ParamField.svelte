<script lang="ts">
  import { t } from '../lib/i18n.svelte'
  let { def, value, onchange }: { def: any; value: any; onchange: (v: any) => void } = $props()

  const set = $derived(value !== undefined && value !== null)
  const label = $derived(def.flags.filter((f: string) => f.startsWith('--')).join(', ') || def.id)

  function parseNum(s: string) {
    s = s.trim()
    if (s === '') return undefined
    const n = Number(s)
    return isNaN(n) ? s : n
  }
  function onRepeat(s: string) {
    const lines = s.split('\n').map((x) => x.trim()).filter(Boolean)
    onchange(lines.length ? lines : undefined)
  }
</script>

<div class="pf" class:set>
  <div class="top">
    <code class="fl" title={def.flags.join(', ')}>{label}</code>
    {#if set}<button class="ghost sm reset" title={t('Сбросить к значению по умолчанию')} onclick={() => onchange(undefined)}>↺</button>{/if}
  </div>
  <div class="inp">
    {#if def.kind === 'flag'}
      <label class="chk"><input type="checkbox" checked={value === true} onchange={(e) => onchange(e.currentTarget.checked ? true : undefined)} /> {t('включить')}</label>
    {:else if def.kind === 'toggle'}
      <select value={value === true ? 'on' : value === false ? 'off' : ''} onchange={(e) => { const v = e.currentTarget.value; onchange(v === 'on' ? true : v === 'off' ? false : undefined) }}>
        <option value="">{t('по умолчанию')}{def.default ? ` (${def.default})` : ''}</option>
        <option value="on">{t('вкл')} ({def.id})</option>
        {#if def.neg_flag}<option value="off">{t('выкл')} ({def.neg_flag})</option>{/if}
      </select>
    {:else if def.kind === 'choice'}
      <select value={set ? String(value) : ''} onchange={(e) => onchange(e.currentTarget.value === '' ? undefined : e.currentTarget.value)}>
        <option value="">{t('по умолчанию')}{def.default ? ` (${def.default})` : ''}</option>
        {#each def.choices as c}<option value={c}>{c}</option>{/each}
        {#if set && !def.choices.includes(String(value))}<option value={String(value)}>{value}</option>{/if}
      </select>
    {:else if def.repeatable}
      <textarea rows="2" placeholder={t('по одному значению в строке')} value={Array.isArray(value) ? value.join('\n') : set ? String(value) : ''} onchange={(e) => onRepeat(e.currentTarget.value)}></textarea>
    {:else if def.kind === 'int' || def.kind === 'float'}
      <input inputmode="decimal" placeholder={def.default ?? def.value_hint ?? ''} value={set ? String(value) : ''} onchange={(e) => onchange(parseNum(e.currentTarget.value))} />
    {:else}
      <input placeholder={def.default ?? def.value_hint ?? ''} value={set ? String(value) : ''} onchange={(e) => onchange(e.currentTarget.value.trim() === '' ? undefined : e.currentTarget.value)} />
    {/if}
  </div>
  {#if def.description}<div class="d">{def.description}</div>{/if}
</div>

<style>
  .pf { padding: 8px 10px; border-radius: 8px; border: 1px solid transparent; display: flex; flex-direction: column; gap: 4px; }
  .pf:hover { background: var(--bg3); }
  .pf.set { border-color: color-mix(in srgb, var(--acc) 45%, transparent); background: color-mix(in srgb, var(--acc) 6%, transparent); }
  .top { display: flex; align-items: center; gap: 6px; min-height: 22px; }
  .fl { color: var(--fg); font-size: 12.5px; }
  .reset { margin-left: auto; padding: 0 6px; }
  .inp :global(input:not([type='checkbox'])), .inp select, .inp textarea { width: 100%; }
  .d { font-size: 11.5px; color: var(--fg3); white-space: pre-wrap; max-height: 4.5em; overflow: hidden; }
  .pf:hover .d { max-height: none; }
</style>
