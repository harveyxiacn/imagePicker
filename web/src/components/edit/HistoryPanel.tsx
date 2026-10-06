import { useQueryClient } from '@tanstack/react-query'
import { Pencil, X } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { redo, undo } from '@/lib/actions'
import { useHistory } from '@/lib/history'
import { useEdit } from '@/stores/edit'

/** History list (▸ 历史): click an entry to revert to the state after it (repeated undo / redo). */
export function HistoryPanel() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const undoStack = useHistory((s) => s.undoStack)
  const redoStack = useHistory((s) => s.redoStack)
  const close = useEdit((s) => s.setHistoryOpen)

  const revertTo = async (k: number) => {
    const n = undoStack.length - 1 - k
    for (let i = 0; i < n; i++) await undo(qc)
  }
  const redoTo = async (j: number) => {
    const n = redoStack.length - j
    for (let i = 0; i < n; i++) await redo(qc)
  }

  return (
    <div className="anim-pop absolute top-2 right-[328px] z-20 flex max-h-[70%] w-72 flex-col rounded-card border border-line bg-elevated shadow-[var(--shadow)]" data-testid="history-panel">
      <div className="flex items-center border-b border-line px-3 py-2">
        <span className="flex-1 font-semibold">{t('edit.history')}</span>
        <button className="btn btn-ghost btn-icon !h-6 !w-6" aria-label={t('common.close')} onClick={() => close(false)}>
          <X size={14} />
        </button>
      </div>
      <ul className="min-h-0 flex-1 overflow-y-auto p-1">
        {redoStack.map((e, j) => (
          <li key={`r${j}`}>
            <button className="flex w-full items-center gap-2 rounded px-2 py-1 text-left text-xs text-faint hover:bg-panel" onClick={() => void redoTo(j)}>
              {e.edits ? <Pencil size={11} /> : <span className="w-[11px]" />}
              <span className="flex-1 truncate italic">{e.label}</span>
            </button>
          </li>
        ))}
        {undoStack.length === 0 && redoStack.length === 0 && <li className="px-2 py-3 text-center text-xs text-muted">{t('edit.historyEmpty')}</li>}
        {[...undoStack].reverse().map((e, i) => {
          const k = undoStack.length - 1 - i
          const current = i === 0
          return (
            <li key={`u${k}`}>
              <button
                className={`flex w-full items-center gap-2 rounded px-2 py-1 text-left text-xs hover:bg-panel ${current ? 'bg-accent/15 font-medium' : ''}`}
                onClick={() => void revertTo(k)}
                aria-current={current}
                data-testid="history-item"
              >
                {e.edits ? <Pencil size={11} className="text-accent" /> : <span className="w-[11px]" />}
                <span className="flex-1 truncate">{e.label}</span>
              </button>
            </li>
          )
        })}
      </ul>
    </div>
  )
}
