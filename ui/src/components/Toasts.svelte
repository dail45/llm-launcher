<script lang="ts">
  import { app } from '../lib/state.svelte'
</script>

<div class="ts">
  {#each app.toasts as x (x.id)}
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="t {x.level}" onclick={() => app.dismiss(x.id)}>
      <b>{x.title}</b>
      {#if x.message}<div class="small muted">{x.message}</div>{/if}
    </div>
  {/each}
</div>

<style>
  .ts { position: fixed; right: 16px; bottom: 16px; display: flex; flex-direction: column; gap: 8px; z-index: 100; max-width: 380px; }
  .t { background: var(--bg3); border: 1px solid var(--line); border-left: 4px solid var(--info); border-radius: 8px; padding: 9px 12px; cursor: pointer; box-shadow: 0 6px 24px rgba(0, 0, 0, 0.35); word-break: break-word; }
  .t.success { border-left-color: var(--ok); }
  .t.error { border-left-color: var(--bad); }
  .t.warning, .t.warn { border-left-color: var(--warn); }
</style>
