import { useQueryClient } from '@tanstack/react-query'
import { Download, PenLine, ScanFace } from 'lucide-react'
import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { applyProfilesToPhotos } from '@/lib/editActions'
import { useToasts } from '@/stores/toasts'
import { useUi } from '@/stores/ui'
import { MenuItem } from './MenuItem'

interface MenuState {
  id: number
  x: number
  y: number
}

/** Right-click menu of a library photo; acts on the multi-selection when the photo belongs to it. */
export function usePhotoMenu(sessionId: number): { open: (e: { clientX: number; clientY: number; preventDefault: () => void }, id: number) => void; node: ReactNode } {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const navigate = useNavigate()
  const [menu, setMenu] = useState<MenuState | null>(null)
  const ref = useRef<HTMLDivElement>(null)

  const open = useCallback((e: { clientX: number; clientY: number; preventDefault: () => void }, id: number) => {
    e.preventDefault()
    const ui = useUi.getState()
    // Right-clicking outside the selection makes that photo the (single) target.
    if (!ui.selection.ids.has(id)) ui.setSelection({ ids: new Set([id]), anchorId: id })
    ui.setActive(id)
    setMenu({ id, x: e.clientX, y: e.clientY })
  }, [])

  useEffect(() => {
    if (!menu) return
    const close = () => setMenu(null)
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && close()
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) close()
    }
    document.addEventListener('mousedown', onDown)
    window.addEventListener('keydown', onKey, true)
    window.addEventListener('blur', close)
    window.addEventListener('resize', close)
    return () => {
      document.removeEventListener('mousedown', onDown)
      window.removeEventListener('keydown', onKey, true)
      window.removeEventListener('blur', close)
      window.removeEventListener('resize', close)
    }
  }, [menu])

  const targets = (): number[] => {
    const ids = [...useUi.getState().selection.ids]
    return ids.length ? ids : menu ? [menu.id] : []
  }

  const applyProfiles = async () => {
    const ids = targets()
    setMenu(null)
    try {
      const n = await applyProfilesToPhotos(qc, ids, t('history.applyProfiles', { n: ids.length }))
      useToasts.getState().push(n > 0 ? 'success' : 'info', n > 0 ? t('beauty.profilesApplied', { n }) : t('beauty.profilesNone'), 3000)
    } catch (err) {
      useToasts.getState().push('error', err instanceof Error ? err.message : String(err), 5000)
    }
  }

  const count = useUi((s) => s.selection.ids.size)
  const node = menu
    ? createPortal(
        <div
          ref={ref}
          role="menu"
          aria-label={t('photoMenu.label')}
          className="anim-pop fixed z-[70] min-w-60 rounded-card border border-line bg-elevated p-1 shadow-[var(--shadow)]"
          style={{ left: Math.min(menu.x, window.innerWidth - 260), top: Math.min(menu.y, window.innerHeight - 140) }}
          data-testid="photo-menu"
        >
          <MenuItem icon={<PenLine size={14} />} onClick={() => navigate(`/s/${sessionId}/edit/${menu.id}`)}>
            {t('photoMenu.edit')}
          </MenuItem>
          <MenuItem icon={<ScanFace size={14} className="text-ai" />} onClick={() => void applyProfiles()}>
            <span data-testid="menu-apply-profiles">{t('beauty.applyProfilesToSelected', { n: Math.max(1, count) })}</span>
          </MenuItem>
          <MenuItem
            icon={<Download size={14} />}
            onClick={() => {
              setMenu(null)
              useUi.getState().setExportOpen(true)
            }}
          >
            {t('photoMenu.export')}
          </MenuItem>
        </div>,
        document.body,
      )
    : null
  return { open, node }
}
