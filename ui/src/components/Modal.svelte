<script lang="ts">
  import type { Snippet } from 'svelte'
  let { title = '', width = '640px', onclose, children, footer }: { title?: string; width?: string; onclose: () => void; children: Snippet; footer?: Snippet } = $props()
  function key(e: KeyboardEvent) {
    if (e.key === 'Escape') onclose()
  }
</script>

<svelte:window onkeydown={key} />
<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="ov" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="m" style="width:{width}">
    <div class="hd">
      <b>{title}</b>
      <button class="ghost sm" onclick={onclose} aria-label="close">✕</button>
    </div>
    <div class="bd">{@render children()}</div>
    {#if footer}<div class="ft">{@render footer()}</div>{/if}
  </div>
</div>

<style>
  .ov { position: fixed; inset: 0; background: rgba(0, 0, 0, 0.55); display: flex; align-items: flex-start; justify-content: center; z-index: 50; padding: 6vh 16px; overflow: auto; }
  .m { background: var(--bg2); border: 1px solid var(--line); border-radius: 12px; max-width: 100%; box-shadow: 0 20px 60px rgba(0, 0, 0, 0.5); display: flex; flex-direction: column; max-height: 88vh; }
  .hd { display: flex; justify-content: space-between; align-items: center; padding: 12px 16px; border-bottom: 1px solid var(--line); }
  .bd { padding: 16px; overflow: auto; }
  .ft { padding: 12px 16px; border-top: 1px solid var(--line); display: flex; justify-content: flex-end; gap: 8px; }
</style>
