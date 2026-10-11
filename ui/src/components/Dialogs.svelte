<script lang="ts">
  import { dlg, closeDlg } from '../lib/dialog.svelte'
  import { t } from '../lib/i18n.svelte'
  import Modal from './Modal.svelte'
  let input = $state<HTMLInputElement>()
  $effect(() => {
    if (dlg.cur?.kind === 'prompt') setTimeout(() => input?.focus(), 30)
  })
</script>

{#if dlg.cur}
  {@const c = dlg.cur}
  <Modal title={c.title} width="440px" onclose={() => closeDlg(c.kind === 'confirm' ? false : null)}>
    {#if c.text}<p class="muted" style="margin-top:0;white-space:pre-wrap">{c.text}</p>{/if}
    {#if c.kind === 'prompt'}
      <input bind:this={input} style="width:100%" bind:value={c.value} onkeydown={(e) => e.key === 'Enter' && closeDlg(c.value)} />
    {/if}
    {#snippet footer()}
      <button onclick={() => closeDlg(c.kind === 'confirm' ? false : null)}>{t('Отмена')}</button>
      <button class={c.danger ? 'danger solid' : 'primary'} onclick={() => closeDlg(c.kind === 'confirm' ? true : c.value)}>{c.ok === 'OK' ? 'OK' : c.ok}</button>
    {/snippet}
  </Modal>
{/if}
