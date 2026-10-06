import { Ban, Check, Search, Users } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { faceCropUrl } from '@/api/client'
import { usePeople } from '@/api/queries'
import type { PersonState } from '@/api/types'
import { personLabel } from '@/lib/ai'
import {
  applyFacesPreset,
  cyclePerson,
  DEFAULT_PERSON_FILTER,
  facesPreset,
  isPersonFilterActive,
  personTri,
  toggleState,
  type FacesPreset,
  type PersonFilter,
} from '@/lib/filter'
import { useUi } from '@/stores/ui'

const STATES: PersonState[] = ['eyes_open', 'smiling', 'looking', 'subject']
const PRESETS: FacesPreset[] = ['any', 'single', 'few', 'many', 'none']

interface Props {
  sessionId: number
  /** Result count for the current filter (the library total). */
  resultCount: number
}

/** Person filter (doc 04 3.6.1): button + popover. Opens with Shift+P. */
export function PersonFilterButton({ sessionId, resultCount }: Props) {
  const { t } = useTranslation()
  const open = useUi((s) => s.personFilterOpen)
  const setOpen = useUi((s) => s.setPersonFilterOpen)
  const filter = useUi((s) => s.filter.person)
  const wrap = useRef<HTMLDivElement>(null)
  const active = isPersonFilterActive(filter)

  // Click outside / Escape / Shift+P close the popover (global shortcuts are paused while a dialog is open).
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (wrap.current && !wrap.current.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape' || (e.shiftKey && e.key.toLowerCase() === 'p' && !(e.target instanceof HTMLInputElement))) {
        e.preventDefault()
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', onDown)
    window.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      window.removeEventListener('keydown', onKey)
    }
  }, [open, setOpen])

  return (
    <div className="relative" ref={wrap}>
      <button
        className="btn"
        aria-pressed={active || open}
        aria-haspopup="dialog"
        aria-expanded={open}
        title={`${t('person.filter')} (Shift+P)`}
        onClick={() => setOpen(!open)}
        data-testid="person-filter-button"
      >
        <Users size={14} />
        {t('person.filter')}
      </button>
      {open && <Popover sessionId={sessionId} resultCount={resultCount} />}
    </div>
  )
}

function Popover({ sessionId, resultCount }: Props) {
  const { t, i18n } = useTranslation()
  const people = usePeople(sessionId)
  const filter = useUi((s) => s.filter.person)
  const setFilter = useUi((s) => s.setFilter)
  const setOpen = useUi((s) => s.setPersonFilterOpen)
  const [query, setQuery] = useState('')
  const [manyMin, setManyMin] = useState(4)
  const set = (p: PersonFilter) => setFilter({ person: p })

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase()
    return (people.data ?? [])
      .filter((p) => !p.hidden)
      .filter((p) => !q || (p.name ?? '').toLowerCase().includes(q))
  }, [people.data, query])

  const preset = facesPreset(filter)
  const hasInclude = filter.include.length > 0

  return (
    <div
      role="dialog"
      aria-label={t('person.filter')}
      className="anim-pop absolute top-full left-0 z-40 mt-1.5 w-[440px] max-w-[calc(100vw-24px)] rounded-card border border-line bg-elevated p-3 shadow-[var(--shadow)]"
      data-testid="person-filter-popover"
    >
      <div className="mb-2 flex items-center gap-4">
        <span className="font-medium">{t('person.match')}</span>
        {(['all', 'any'] as const).map((m) => (
          <label key={m} className="flex cursor-pointer items-center gap-1.5">
            <input type="radio" name="person-mode" checked={filter.mode === m} onChange={() => set({ ...filter, mode: m })} />
            {t(`person.mode_${m}`)}
          </label>
        ))}
      </div>

      <div className="relative mb-2">
        <Search size={13} className="absolute top-1/2 left-2 -translate-y-1/2 text-faint" />
        <input className="field w-full !pl-7" value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t('person.search')} aria-label={t('person.search')} />
      </div>

      <div className="grid max-h-[210px] grid-cols-5 gap-2 overflow-y-auto pr-1" data-testid="person-grid">
        {people.isPending && <div className="col-span-5 py-4 text-center text-muted">{t('library.loading')}</div>}
        {!people.isPending && visible.length === 0 && <div className="col-span-5 py-4 text-center text-muted">{t('person.empty')}</div>}
        {visible.map((p) => {
          const tri = personTri(filter, p.id)
          return (
            <button
              key={p.id}
              type="button"
              className="group relative flex flex-col items-center gap-1 rounded-control p-1 hover:bg-panel"
              aria-pressed={tri !== 'off'}
              title={`${personLabel(p, t('person.unnamed'))} · ${t(`person.tri_${tri}`)}`}
              onClick={() => set(cyclePerson(filter, p.id))}
              data-tri={tri}
            >
              <span className="relative block h-12 w-12 overflow-hidden rounded-full bg-bg" style={{ outline: tri === 'include' ? '2px solid var(--success)' : tri === 'exclude' ? '2px solid var(--danger)' : 'none', outlineOffset: 1 }}>
                <img src={faceCropUrl(p.cover_face_id, 128)} alt="" className={`h-full w-full object-cover ${tri === 'exclude' ? 'opacity-40' : ''}`} draggable={false} />
                {tri !== 'off' && (
                  <span className={`absolute inset-0 flex items-center justify-center bg-black/35 ${tri === 'include' ? 'text-success' : 'text-danger'}`}>
                    {tri === 'include' ? <Check size={22} strokeWidth={3} /> : <Ban size={22} strokeWidth={3} />}
                  </span>
                )}
              </span>
              <span className="w-full truncate text-center text-xs">{personLabel(p, t('person.unnamed'))}</span>
              <span className="tnum text-[10px] text-faint">{p.photo_count.toLocaleString(i18n.language)}</span>
            </button>
          )
        })}
      </div>
      <div className="mt-1 text-xs text-faint">{t('person.triHint')}</div>

      <div className="mt-3 flex flex-wrap items-center gap-1.5">
        <span className={hasInclude ? 'text-muted' : 'text-faint'}>{t('person.mustBe')}</span>
        {STATES.map((s) => (
          <button
            key={s}
            className="btn !h-6"
            disabled={!hasInclude}
            aria-pressed={filter.states.includes(s)}
            onClick={() => set(toggleState(filter, s))}
          >
            {filter.states.includes(s) && <Check size={12} />}
            {t(`person.state_${s}`)}
          </button>
        ))}
      </div>

      <div className="mt-3 flex flex-wrap items-center gap-x-3 gap-y-1">
        <span className="text-muted">{t('person.count')}</span>
        {PRESETS.map((pr) => (
          <label key={pr} className="flex cursor-pointer items-center gap-1">
            <input type="radio" name="faces-preset" checked={preset === pr} onChange={() => set(applyFacesPreset(filter, pr, manyMin))} />
            {pr === 'many' ? (
              <span className="flex items-center gap-1">
                ≥
                <input
                  type="number"
                  min={2}
                  max={20}
                  value={manyMin}
                  className="field !h-6 w-12 !px-1"
                  aria-label={t('person.count_many')}
                  onChange={(e) => {
                    const n = Math.max(2, Math.min(20, Number(e.target.value) || 4))
                    setManyMin(n)
                    if (preset === 'many') set(applyFacesPreset(filter, 'many', n))
                  }}
                />
                {t('person.people')}
              </span>
            ) : (
              t(`person.count_${pr}`)
            )}
          </label>
        ))}
      </div>

      <label className="mt-3 flex cursor-pointer items-center gap-2">
        <input type="checkbox" checked={filter.includeBackground} onChange={(e) => set({ ...filter, includeBackground: e.target.checked })} />
        {t('person.background')}
      </label>

      <div className="mt-3 flex items-center justify-between border-t border-line pt-2">
        <span className="tnum text-muted" data-testid="person-result">
          {t('person.result', { n: resultCount.toLocaleString(i18n.language) })}
        </span>
        <div className="flex gap-2">
          <button className="btn btn-ghost" onClick={() => set(DEFAULT_PERSON_FILTER)} disabled={!isPersonFilterActive(filter)}>
            {t('person.reset')}
          </button>
          <button className="btn btn-primary" onClick={() => setOpen(false)}>
            {t('person.done')}
          </button>
        </div>
      </div>
    </div>
  )
}
