// Promise-based confirm/prompt (native dialogs are unreliable inside webviews).
type D = { kind: 'confirm' | 'prompt'; title: string; text: string; value: string; danger: boolean; ok: string; resolve: (v: any) => void }
export const dlg = $state<{ cur: D | null }>({ cur: null })

export function confirmDlg(title: string, text = '', opts: { danger?: boolean; ok?: string } = {}): Promise<boolean> {
  return new Promise((resolve) => {
    dlg.cur = { kind: 'confirm', title, text, value: '', danger: !!opts.danger, ok: opts.ok || 'OK', resolve }
  })
}
export function promptDlg(title: string, value = '', text = ''): Promise<string | null> {
  return new Promise((resolve) => {
    dlg.cur = { kind: 'prompt', title, text, value, danger: false, ok: 'OK', resolve }
  })
}
export function closeDlg(v: any) {
  const c = dlg.cur
  dlg.cur = null
  c?.resolve(v)
}
