import { ChevronsUpDown, FolderPlus, Layers, Search, Star, X } from 'lucide-react'
import { useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useDevices, usePeople, useSession } from '@/api/queries'
import { searchLayout } from '@/lib/search'
import { ISSUES, SCENE_TYPES, type FlagFilter, type Issue, type SceneType, type SortKey } from '@/api/types'
import { personLabel } from '@/lib/ai'
import { clampFaceSimilarity, DEFAULT_FACE_SIMILARITY, facesPreset, FACE_SIMILARITY_MAX, FACE_SIMILARITY_MIN, isFilterActive, setPersonTri, type FilterState } from '@/lib/filter'
import { useUi } from '@/stores/ui'
import { COLOR_NAMES, colorVar } from '@/lib/colors'
import { DeviceFilterButton } from './DeviceFilterPopover'
import { DateFilterButton, LensFilterButton, SharpFilter, TagFilterButton } from './FilterPopovers'
import { PersonFilterButton } from './PersonFilterPopover'

const FLAGS: ('all' | FlagFilter)[] = ['all', 'picked', 'not_rejected', 'unflagged', 'rejected']
const SORTS: SortKey[] = ['taken_at', '-taken_at', 'name', 'rating', 'ai', 'taste']

interface Props {
  sessionId: number
  shown: number
  total: number
  showSize: boolean
}

/** Select value of the combined issues control: all | none | any | <single issue>. */
function issueValue(f: FilterState): string {
  if (f.issueMode === 'none') return 'none'
  if (f.issueMode === 'any') return f.issues.length === 1 ? f.issues[0] : 'any'
  return 'all'
}

function Pill({ children, onRemove, label, tone }: { children: React.ReactNode; onRemove: () => void; label: string; tone?: 'ai' | 'danger' }) {
  const { t } = useTranslation()
  return (
    <span
      className={`inline-flex h-6 items-center gap-1 rounded-full border px-2 text-xs ${
        tone === 'danger'
          ? 'border-danger/50 bg-danger/10 text-danger'
          : tone === 'ai'
            ? 'border-ai/50 bg-ai/10 text-ai'
            : 'border-line bg-elevated text-fg'
      }`}
      data-testid="filter-pill"
      title={label}
    >
      {children}
      <button className="rounded-full p-0.5 hover:bg-white/10" aria-label={`${t('filter.remove')}: ${label}`} onClick={onRemove}>
        <X size={11} />
      </button>
    </span>
  )
}

/** Similarity threshold of the similar-faces filter; the query only changes when the slider is released. */
function FaceSimilaritySlider({ value, onCommit }: { value: number; onCommit: (v: number) => void }) {
  const { t } = useTranslation()
  const [live, setLive] = useState<number | null>(null)
  const shown = clampFaceSimilarity(live ?? value)
  const commit = () => {
    if (live !== null && live !== clampFaceSimilarity(value)) onCommit(live)
    setLive(null)
  }
  return (
    <>
      <input
        type="range"
        className="w-20"
        min={FACE_SIMILARITY_MIN}
        max={FACE_SIMILARITY_MAX}
        step={0.02}
        value={shown}
        onChange={(e) => setLive(clampFaceSimilarity(Number(e.target.value)))}
        onPointerUp={commit}
        onKeyUp={commit}
        onBlur={commit}
        aria-label={t('filter.faceSimilarity')}
        title={t('filter.faceSimilarityHint')}
        data-testid="face-similarity"
      />
      <span className="tnum">{Math.round(shown * 100)}%</span>
    </>
  )
}

export function FilterBar({ sessionId, shown, total, showSize }: Props) {
  const { t, i18n } = useTranslation()
  const filter = useUi((s) => s.filter)
  const setFilter = useUi((s) => s.setFilter)
  const resetFilter = useUi((s) => s.resetFilter)
  const thumbSize = useUi((s) => s.thumbSize)
  const setThumbSize = useUi((s) => s.setThumbSize)
  const groupedPref = useUi((s) => s.grouped)
  const { byRelevance, grouped } = searchLayout(filter.search, groupedPref)
  const setGrouped = useUi((s) => s.setGrouped)
  const expandAll = useUi((s) => s.expandAll)
  const expanded = useUi((s) => s.expandedStacks)
  const setStacks = useUi((s) => s.setStacks)
  const people = usePeople(sessionId)
  const session = useSession(sessionId)
  const devices = useDevices(sessionId, session.data?.photo_count)
  const devNames = useMemo(() => new Map((devices.data ?? []).map((d) => [d.id, d.name])), [devices.data])
  const fmt = (n: number) => n.toLocaleString(i18n.language)
  const active = isFilterActive(filter)
  const p = filter.person
  const names = useMemo(() => new Map((people.data ?? []).map((x) => [x.id, personLabel(x, t('person.unnamed'))])), [people.data, t])
  const nameOf = (id: number) => names.get(id) ?? t('person.unnamed')

  const setIssue = (v: string) => {
    if (v === 'all') setFilter({ issueMode: 'all', issues: [] })
    else if (v === 'none') setFilter({ issueMode: 'none', issues: [] })
    else if (v === 'any') setFilter({ issueMode: 'any', issues: [] })
    else setFilter({ issueMode: 'any', issues: [v as Issue] })
  }

  const personPill = p.include.length > 0
  const preset = facesPreset(p)
  const showPills = personPill || p.exclude.length > 0 || preset !== 'any' || filter.issueMode !== 'all' || filter.sceneType !== null || filter.device.length > 0 || filter.lens.length > 0 || filter.tags.length > 0 || filter.takenFrom !== null || filter.takenTo !== null || filter.sharp !== null || filter.faceIds.length > 0 || filter.bestOnly || filter.edited || filter.search !== ''

  return (
    <div className="shrink-0 border-b border-line">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5 px-3 py-1.5">
        <div className="flex items-center gap-1" role="group" aria-label={t('filter.rating')}>
          <span className="text-muted">{t('filter.rating')}</span>
          <Star size={13} className="text-accent-text" fill="currentColor" />
          <span className="text-muted">≥</span>
          {[0, 1, 2, 3, 4, 5].map((n) => (
            <button
              key={n}
              className="btn hit !h-6 !w-6 justify-center !px-0 tnum"
              aria-pressed={filter.ratingGte === n}
              onClick={() => setFilter({ ratingGte: n })}
              title={n === 0 ? t('filter.any') : `≥ ${n}`}
            >
              {n === 0 ? '–' : n}
            </button>
          ))}
        </div>

        <label className="flex items-center gap-1.5">
          <span className="text-ai">✨ {t('filter.aiRating')} ≥</span>
          <select
            className="field !px-1.5"
            value={filter.aiRatingGte}
            onChange={(e) => setFilter({ aiRatingGte: Number(e.target.value) })}
            aria-label={t('filter.aiRating')}
            data-testid="ai-rating-filter"
          >
            <option value={0}>{t('filter.any')}</option>
            {[1, 2, 3, 3.5, 4, 4.5, 5].map((n) => (
              <option key={n} value={n}>
                ≥ {n}
              </option>
            ))}
          </select>
        </label>

        <label className="flex items-center gap-1.5">
          <span className="text-muted">{t('filter.flag')}</span>
          <select
            className="field !px-1.5"
            value={filter.flag}
            onChange={(e) => setFilter({ flag: e.target.value as 'all' | FlagFilter })}
            aria-label={t('filter.flag')}
          >
            {FLAGS.map((f) => (
              <option key={f} value={f}>
                {t(`filter.flag_${f}`)}
              </option>
            ))}
          </select>
        </label>

        <div className="flex items-center gap-2" role="group" aria-label={t('filter.color')}>
          <span className="text-muted">{t('filter.color')}</span>
          {COLOR_NAMES.map((c) => (
            <button
              key={c}
              className="hit h-4 w-4 rounded-full border-2 transition-transform hover:scale-110"
              style={{ background: colorVar(c), borderColor: filter.color === c ? 'var(--fg)' : 'transparent' }}
              aria-label={t(`color.${c}`)}
              aria-pressed={filter.color === c}
              onClick={() => setFilter({ color: filter.color === c ? null : c })}
            />
          ))}
        </div>

        <PersonFilterButton sessionId={sessionId} resultCount={shown} />
        <DeviceFilterButton sessionId={sessionId} />
        <LensFilterButton sessionId={sessionId} />
        <TagFilterButton sessionId={sessionId} />
        <DateFilterButton sessionId={sessionId} />
        <SharpFilter />

        <label className="flex items-center gap-1.5">
          <span className="text-muted">{t('filter.issues')}</span>
          <select className="field !px-1.5" value={issueValue(filter)} onChange={(e) => setIssue(e.target.value)} aria-label={t('filter.issues')} data-testid="issues-filter">
            <option value="all">{t('filter.issues_all')}</option>
            <option value="none">{t('filter.issues_none')}</option>
            <option value="any">{t('filter.issues_any')}</option>
            {ISSUES.map((i) => (
              <option key={i} value={i}>
                {t(`issue.${i}`)}
              </option>
            ))}
          </select>
        </label>

        <label className="flex items-center gap-1.5">
          <span className="text-muted">{t('filter.scene')}</span>
          <select
            className="field !px-1.5"
            value={filter.sceneType ?? ''}
            onChange={(e) => setFilter({ sceneType: (e.target.value || null) as SceneType | null })}
            aria-label={t('filter.scene')}
          >
            <option value="">{t('filter.scene_all')}</option>
            {SCENE_TYPES.map((s) => (
              <option key={s} value={s}>
                {t(`scene_type.${s}`)}
              </option>
            ))}
          </select>
        </label>

        <label className="flex items-center gap-1.5">
          <span className="text-muted">{t('filter.sort')}</span>
          <select
            className="field !px-1.5"
            value={byRelevance ? 'relevance' : filter.sort}
            onChange={(e) => setFilter({ sort: e.target.value as SortKey })}
            aria-label={t('filter.sort')}
            disabled={byRelevance}
            title={byRelevance ? t('search.byRelevanceHint') : undefined}
            data-testid="sort-select"
          >
            {byRelevance && <option value="relevance">{t('search.byRelevance')}</option>}
            {SORTS.map((s) => (
              <option key={s} value={s}>
                {t(`filter.sort_${s}`)}
              </option>
            ))}
          </select>
        </label>

        <div className="flex items-center gap-1">
          <button
            className="btn"
            aria-pressed={grouped}
            onClick={() => setGrouped(!groupedPref)}
            disabled={byRelevance}
            title={byRelevance ? t('search.byRelevanceHint') : grouped ? t('stack.flat') : t('stack.grouped')}
            data-testid="grouped-toggle"
            data-coach="stacks"
          >
            <Layers size={14} />
            {grouped ? t('stack.groupedShort') : t('stack.flatShort')}
          </button>
          {grouped && (
            <button
              className="btn btn-icon"
              aria-pressed={expandAll}
              onClick={() => setStacks(expandAll || expanded.size > 0 ? { expandAll: false, expanded: new Set() } : { expandAll: true, expanded: new Set() })}
              title={`${expandAll ? t('stack.collapseAll') : t('stack.expandAll')} (Shift+S)`}
              aria-label={expandAll ? t('stack.collapseAll') : t('stack.expandAll')}
            >
              <ChevronsUpDown size={14} />
            </button>
          )}
        </div>

        {active && (
          <>
            <button className="btn btn-ghost" onClick={resetFilter}>
              <X size={13} />
              {t('filter.clear')}
            </button>
            <button className="btn" onClick={() => useUi.getState().setSaveCollectionOpen(true)} title={t('collections.save')} data-testid="filter-save-collection">
              <FolderPlus size={14} />
              {t('collections.save')}
            </button>
          </>
        )}

        <div className="ml-auto flex items-center gap-4">
          {showSize && (
            <label className="flex items-center gap-2">
              <span className="text-muted">{t('filter.size')}</span>
              <input
                type="range"
                min={96}
                max={320}
                step={8}
                value={thumbSize}
                onChange={(e) => setThumbSize(Number(e.target.value))}
                aria-label={t('filter.size')}
                className="w-28"
              />
            </label>
          )}
          <span className="tnum whitespace-nowrap text-muted" data-testid="showing">
            {t('filter.showing', { x: fmt(shown), y: fmt(total) })}
          </span>
        </div>
      </div>

      {showPills && (
        <div className="flex flex-wrap items-center gap-1.5 border-t border-line/60 px-3 py-1.5" data-testid="filter-pills">
          {filter.search !== '' && (
            <Pill tone="ai" label={t('search.pill')} onRemove={() => setFilter({ search: '' })}>
              <Search size={11} />
              <span className="max-w-[320px] truncate" data-testid="search-pill">
                {t('search.pill')}: {filter.search}
              </span>
            </Pill>
          )}
          {personPill && (
            <Pill
              tone="ai"
              label={t('person.filter')}
              onRemove={() => setFilter({ person: { ...p, include: [], states: [] } })}
            >
              <span>
                👤 {p.include.map(nameOf).join(p.mode === 'all' ? ' + ' : ' | ')}
                {p.states.map((s) => ` · ${t(`person.state_short_${s}`)}`).join('')}
              </span>
            </Pill>
          )}
          {p.exclude.map((id) => (
            <Pill key={id} tone="danger" label={nameOf(id)} onRemove={() => setFilter({ person: setPersonTri(p, id, 'off') })}>
              <span>⊘ {nameOf(id)}</span>
            </Pill>
          ))}
          {preset !== 'any' && (
            <Pill
              label={t('person.count')}
              onRemove={() => setFilter({ person: { ...p, facesMin: null, facesMax: null } })}
            >
              <span>
                {t('person.count')}: {preset === 'custom' ? `${p.facesMin ?? 0}–${p.facesMax ?? '∞'}` : preset === 'many' ? `≥ ${p.facesMin}` : t(`person.count_${preset}`)}
              </span>
            </Pill>
          )}
          {filter.issueMode !== 'all' && (
            <Pill label={t('filter.issues')} onRemove={() => setFilter({ issueMode: 'all', issues: [] })}>
              <span>
                {filter.issueMode === 'none' ? t('filter.issues_none') : filter.issues.length ? filter.issues.map((i) => t(`issue.${i}`)).join(' / ') : t('filter.issues_any')}
              </span>
            </Pill>
          )}
          {filter.bestOnly && (
            <Pill label={t('collections.builtin_best_per_group')} onRemove={() => setFilter({ bestOnly: false })}>
              <span>{t('collections.builtin_best_per_group')}</span>
            </Pill>
          )}
          {filter.edited && (
            <Pill label={t('collections.builtin_edited')} onRemove={() => setFilter({ edited: false })}>
              <span>{t('collections.builtin_edited')}</span>
            </Pill>
          )}
          {filter.device.length > 0 && (
            <Pill label={t('device.filter')} onRemove={() => setFilter({ device: [] })}>
              <span>
                {t('device.filter')}: {filter.device.map((d) => (d === 'none' ? t('device.none') : devNames.get(d) ?? `#${d}`)).join(' | ')}
              </span>
            </Pill>
          )}
          {filter.lens.length > 0 && (
            <Pill label={t('filter.lens')} onRemove={() => setFilter({ lens: [] })}>
              <span className="max-w-[320px] truncate">
                {t('filter.lens')}: {filter.lens.join(' | ')}
              </span>
            </Pill>
          )}
          {filter.tags.length > 0 && (
            <Pill label={t('filter.tags')} onRemove={() => setFilter({ tags: [] })}>
              <span className="max-w-[320px] truncate">
                {t('filter.tags')}: {filter.tags.join(filter.tagMode === 'all' ? ' + ' : ' | ')}
              </span>
            </Pill>
          )}
          {(filter.takenFrom !== null || filter.takenTo !== null) && (
            <Pill label={t('filter.date')} onRemove={() => setFilter({ takenFrom: null, takenTo: null })}>
              <span className="tnum">
                {t('filter.date')}: {filter.takenFrom ?? '…'} – {filter.takenTo ?? '…'}
              </span>
            </Pill>
          )}
          {filter.sharp && (
            <Pill label={t('filter.sharp')} onRemove={() => setFilter({ sharp: null })}>
              <span>
                {t('filter.sharp')}: {t(`filter.sharp_${filter.sharp}`)}
              </span>
            </Pill>
          )}
          {filter.faceIds.length > 0 && (
            <Pill tone="ai" label={t('filter.faces')} onRemove={() => setFilter({ faceIds: [], faceSimilarity: null })}>
              <span>{t('filter.facesN', { n: filter.faceIds.length })}</span>
              <FaceSimilaritySlider value={filter.faceSimilarity ?? DEFAULT_FACE_SIMILARITY} onCommit={(v) => setFilter({ faceSimilarity: v })} />
            </Pill>
          )}
          {filter.sceneType && (
            <Pill label={t('filter.scene')} onRemove={() => setFilter({ sceneType: null })}>
              <span>
                {t('filter.scene')}: {t(`scene_type.${filter.sceneType}`)}
              </span>
            </Pill>
          )}
        </div>
      )}
    </div>
  )
}
