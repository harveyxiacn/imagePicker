import { useQueryClient } from '@tanstack/react-query'
import { Ban, Check, Images, Pencil, ScanSearch, UserX } from 'lucide-react'
import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { useTranslation } from 'react-i18next'
import { api } from '@/api/client'
import type { Face } from '@/api/types'
import { qk } from '@/lib/cache'
import { useHistory } from '@/lib/history'
import { movedFacesEntry, personPatchEntry, refreshPeople } from '@/lib/structureUndo'
import { setPersonTri } from '@/lib/filter'
import { usePersonLabel } from '@/lib/usePersonLabel'
import { usePopover } from '@/lib/usePopover'
import { useToasts } from '@/stores/toasts'
import { useUi } from '@/stores/ui'
import { MenuItem } from './MenuItem'
import { Modal } from './Modal'
import { errorText } from '@/lib/errors'

interface MenuState {
  face: Face
  x: number
  y: number
}

/**
 * Right-click menu for a face: filter by / exclude / name this person (doc 04 3.6.1 "click a face to filter"), "not
 * this person", and search by this face (where "this is a new person" can gather its look-alikes).
 */
export function useFaceMenu(): { open: (e: { clientX: number; clientY: number; preventDefault: () => void }, face: Face) => void; node: ReactNode } {
  const { t } = useTranslation()
  const personLabel = usePersonLabel()
  const qc = useQueryClient()
  const [menu, setMenu] = useState<MenuState | null>(null)
  const [naming, setNaming] = useState<Face | null>(null)
  const [name, setName] = useState('')
  const ref = useRef<HTMLDivElement>(null)
  // Escape / click outside / focus into the menu + arrow keys; focus goes back to what had it before the right-click
  const pop = usePopover({ open: menu !== null, onClose: () => setMenu(null), menu: true, panelRef: ref })

  const open = useCallback((e: { clientX: number; clientY: number; preventDefault: () => void }, face: Face) => {
    e.preventDefault()
    setMenu({ face, x: e.clientX, y: e.clientY })
  }, [])

  useEffect(() => {
    if (!menu) return
    const close = () => setMenu(null)
    window.addEventListener('blur', close)
    window.addEventListener('resize', close)
    return () => {
      window.removeEventListener('blur', close)
      window.removeEventListener('resize', close)
    }
  }, [menu])

  const filterBy = (face: Face, tri: 'include' | 'exclude') => {
    if (face.person_id === null) return
    const ui = useUi.getState()
    ui.setFilter({ person: setPersonTri(ui.filter.person, face.person_id, tri) })
    setMenu(null)
  }

  const notThisPerson = async (face: Face) => {
    setMenu(null)
    try {
      await api.setFacePerson(face.id, null)
      void qc.invalidateQueries({ queryKey: qk.peopleAll })
      void qc.invalidateQueries({ queryKey: ['analysis', 'photo', face.photo_id] })
      if (face.person_id !== null) {
        useHistory.getState().push(
          movedFacesEntry({
            label: t('history.moveFace'),
            api,
            faces: [{ id: face.id, from: face.person_id }],
            again: async () => void (await api.setFacePerson(face.id, null)),
            after: () => refreshPeople(qc),
          }),
        )
      }
      useToasts.getState().push('info', t('face.splitDone'), 2200)
    } catch (err) {
      useToasts.getState().push('error', errorText(err))
    }
  }

  const saveName = async () => {
    const face = naming
    if (!face || face.person_id === null) return
    try {
      const next = name.trim() || null
      await api.patchPerson(face.person_id, { name: next })
      useHistory.getState().push(personPatchEntry({ label: t('history.renamePerson'), api, personId: face.person_id, before: { name: face.person_name }, next: { name: next }, after: () => refreshPeople(qc) }))
      void qc.invalidateQueries({ queryKey: qk.peopleAll })
      void qc.invalidateQueries({ queryKey: ['analysis', 'photo'] })
      void qc.invalidateQueries({ queryKey: ['burstFaces'] })
      setNaming(null)
    } catch (err) {
      useToasts.getState().push('error', errorText(err))
    }
  }

  const known = menu?.face.person_id != null
  const node = (
    <>
      {menu &&
        createPortal(
          <div
            ref={ref}
            {...pop.panelProps}
            role="menu"
            aria-label={t('face.menu')}
            className="anim-pop fixed z-(--z-menu) min-w-52 rounded-card border border-line bg-elevated p-1 shadow-[var(--shadow)]"
            style={{ left: Math.min(menu.x, window.innerWidth - 230), top: Math.min(menu.y, window.innerHeight - 245) }}
            data-testid="face-menu"
          >
            <div className="px-2 py-1 text-xs text-muted">{known || menu.face.person_name ? personLabel(menu.face.person_id, menu.face.person_name) : t('face.unassigned')}</div>
            <MenuItem icon={<Check size={14} className="text-success" />} disabled={!known} onClick={() => filterBy(menu.face, 'include')}>
              {t('face.filterBy')}
            </MenuItem>
            <MenuItem icon={<Ban size={14} className="text-danger" />} disabled={!known} onClick={() => filterBy(menu.face, 'exclude')}>
              {t('face.exclude')}
            </MenuItem>
            <MenuItem
              icon={<Pencil size={14} />}
              disabled={!known}
              onClick={() => {
                setName(menu.face.person_name ?? '')
                setNaming(menu.face)
                setMenu(null)
              }}
            >
              {t('face.name')}
            </MenuItem>
            <MenuItem icon={<UserX size={14} />} disabled={!known} onClick={() => void notThisPerson(menu.face)}>
              {t('face.notThisPerson')}
            </MenuItem>
            <MenuItem
              icon={<Images size={14} />}
              onClick={() => {
                useUi.getState().setFilter({ faceIds: [menu.face.id], faceSimilarity: null })
                setMenu(null)
              }}
            >
              {t('face.similarPhotos')}
            </MenuItem>
            <MenuItem
              icon={<ScanSearch size={14} />}
              onClick={() => {
                useUi.getState().openFaceSearch(menu.face)
                setMenu(null)
              }}
            >
              {t('face.searchSimilar')}
            </MenuItem>
          </div>,
          document.body,
        )}
      <Modal
        open={naming !== null}
        onOpenChange={(o) => !o && setNaming(null)}
        title={t('face.nameTitle')}
        width="max-w-sm"
        footer={
          <>
            <button className="btn" onClick={() => setNaming(null)}>
              {t('common.cancel')}
            </button>
            <button className="btn btn-primary" onClick={() => void saveName()} data-testid="name-save">
              {t('common.save')}
            </button>
          </>
        }
      >
        <input
          className="field w-full"
          autoFocus
          value={name}
          placeholder={t('face.namePlaceholder')}
          aria-label={t('face.name')}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && void saveName()}
          data-testid="name-input"
        />
      </Modal>
    </>
  )
  return { open, node }
}
