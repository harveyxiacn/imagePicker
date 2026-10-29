import { CalendarDays, ChevronRight, Loader2, X, ZoomIn, ZoomOut } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import type { KeyboardEvent } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'
import { thumbUrlById } from '@/api/client'
import { useLibraryTimeline } from '@/api/queries'
import type { TimelineBucket, TimelineEventRef } from '@/api/types'
import { toggleEvent } from '@/lib/timeline'
import {
  activeBucketKey,
  barRatio,
  breadcrumbs,
  bucketLabel,
  bucketShortLabel,
  moveFocus,
  ROOT_SCOPE,
  timelineQueryFor,
  toggleBucketRange,
  zoomInScope,
  zoomKey,
  zoomOutScope,
  type Scope,
} from '@/lib/timelineScrub'
import { useUi } from '@/stores/ui'
import { errorText } from '@/lib/errors'

/**
 * Timeline scrubber (PRD F1.9): the photos of the library by capture date, zoomable year -> month ->
 * day. A tile click sets the grid's capture-date filter to the tile; events of a month / day are
 * listed under the tiles. Keyboard: arrows move, Enter filters, + / - zoom, Esc is left to the page.
 */
export function TimelinePanel({ sessionId }: { sessionId: number }) {
  const { t, i18n } = useTranslation()
  const navigate = useNavigate()
  const filter = useUi((s) => s.filter)
  const [scope, setScope] = useState<Scope>(ROOT_SCOPE)
  const [allSessions, setAllSessions] = useState(true)
  const [focus, setFocus] = useState(0)
  const tiles = useRef<Array<HTMLButtonElement | null>>([])
  const pendingFocus = useRef(false)

  const request = useMemo(() => timelineQueryFor(filter, scope, allSessions ? null : sessionId), [filter, scope, allSessions, sessionId])
  const q = useLibraryTimeline(request)
  const data = q.data
  const buckets = useMemo(() => data?.buckets ?? [], [data])
  const active = activeBucketKey(buckets, filter)
  const max = useMemo(() => buckets.reduce((m, b) => Math.max(m, b.count), 0), [buckets])
  const level = data?.granularity ?? scope.granularity

  // a zoom or a new answer may leave the roving focus past the end
  const current = Math.min(focus, Math.max(0, buckets.length - 1))
  useEffect(() => {
    if (pendingFocus.current && buckets.length) {
      pendingFocus.current = false
      tiles.current[Math.min(focus, buckets.length - 1)]?.focus()
    }
  }, [buckets, focus])

  const goto = (s: Scope, keepFocus = false) => {
    pendingFocus.current = keepFocus
    setFocus(0)
    setScope(s)
  }
  const filterTo = (b: TimelineBucket) => useUi.getState().setFilter(toggleBucketRange(b, filter))
  const zoomIn = (b: TimelineBucket, byKeyboard = false) => {
    const next = zoomInScope(b, level)
    if (next) goto(next, byKeyboard)
  }
  const zoomOut = (byKeyboard = false) => {
    if (scope.granularity !== 'year') goto(zoomOutScope(scope), byKeyboard)
  }

  const onKey = (e: KeyboardEvent, i: number, b: TimelineBucket) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return
    const to = moveFocus(e.key, i, buckets.length)
    if (to !== null) {
      e.preventDefault()
      setFocus(to)
      tiles.current[to]?.focus()
      return
    }
    const z = zoomKey(e.key)
    if (z === 'in') {
      e.preventDefault()
      zoomIn(b, true)
    } else if (z === 'out') {
      e.preventDefault()
      zoomOut(true)
    }
  }

  const openEvent = (ev: TimelineEventRef) => {
    const ui = useUi.getState()
    if (ev.session_id === sessionId) {
      ui.setFilter({ eventId: toggleEvent(ui.filter.eventId, ev.id) })
    } else {
      ui.openWithFilter({ eventId: ev.id })
      navigate(`/s/${ev.session_id}`)
    }
  }

  const crumbs = breadcrumbs(scope)
  const crumbText = (c: (typeof crumbs)[number]) => (c.key === null ? t('timelineView.all') : bucketLabel(c.key, c.granularity === 'all' ? 'year' : c.granularity, i18n.language))
  const eventId = filter.eventId

  return (
    <section className="shrink-0 border-b border-line bg-panel px-3 py-2" aria-label={t('timelineView.title')} data-testid="timeline-panel">
      <div className="flex flex-wrap items-center gap-2">
        <CalendarDays size={14} className="text-muted" aria-hidden />
        <nav aria-label={t('timelineView.title')} className="flex min-w-0 flex-1 flex-wrap items-center gap-0.5 text-sm">
          {crumbs.map((c, i) => (
            <span key={`${c.granularity}-${c.key}`} className="flex items-center gap-0.5">
              {i > 0 && <ChevronRight size={12} className="text-faint" aria-hidden />}
              {c.scope ? (
                <button className="btn hit btn-ghost !h-6 !px-1.5" onClick={() => goto(c.scope!)} data-testid="timeline-crumb">
                  {crumbText(c)}
                </button>
              ) : (
                <span className="px-1.5 font-medium" aria-current="location">
                  {crumbText(c)}
                </span>
              )}
            </span>
          ))}
        </nav>
        <button
          className="btn hit !h-6"
          aria-pressed={allSessions}
          title={t('timelineView.scopeHint')}
          onClick={() => {
            setAllSessions(!allSessions)
            setFocus(0)
          }}
          data-testid="timeline-scope"
        >
          {allSessions ? t('timelineView.scopeAll') : t('timelineView.scopeSession')}
        </button>
        <button className="btn btn-ghost btn-icon !h-6 !w-6" aria-label={t('timelineView.zoomOut')} title={t('timelineView.zoomOut')} disabled={scope.granularity === 'year'} onClick={() => zoomOut()} data-testid="timeline-zoom-out">
          <ZoomOut size={13} />
        </button>
        <button className="btn btn-ghost btn-icon !h-6 !w-6" aria-label={t('timelineView.close')} title={t('timelineView.close')} onClick={() => useUi.getState().setTimelineOpen(false)} data-testid="timeline-close">
          <X size={13} />
        </button>
      </div>

      {q.isPending ? (
        <div className="flex items-center gap-2 py-3 text-sm text-muted">
          <Loader2 className="animate-spin" size={14} />
          {t('library.loading')}
        </div>
      ) : q.isError ? (
        <div className="flex items-center gap-3 py-3 text-sm text-danger">
          {errorText(q.error)}
          <button className="btn hit !h-6" onClick={() => void q.refetch()}>
            {t('common.retry')}
          </button>
        </div>
      ) : buckets.length === 0 ? (
        <div className="py-3 text-sm text-muted" data-testid="timeline-empty">
          {t('timelineView.empty')}
        </div>
      ) : (
        <div
          role="group"
          aria-label={t('timelineView.desc')}
          aria-describedby="timeline-keys"
          className="mt-2 flex gap-1 overflow-x-auto pb-1"
          data-testid="timeline-buckets"
          data-level={level}
        >
          <span id="timeline-keys" className="sr-only">
            {t('timelineView.keys')}
          </span>
          {buckets.map((b, i) => {
            const on = active === b.key
            const label = bucketLabel(b.key, level, i18n.language)
            const short = bucketShortLabel(b.key, level, i18n.language)
            // the button is named by its content: visible short label + count, then the full label for screen readers
            const name = `${label.includes(short) ? label : `${short} ${label}`}, ${t('timelineView.photos', { n: b.count.toLocaleString(i18n.language) })}`
            const next = zoomInScope(b, level)
            return (
              <div key={b.key} className="relative shrink-0">
                <button
                  ref={(el) => {
                    tiles.current[i] = el
                  }}
                  className="relative flex h-[88px] w-24 flex-col justify-end gap-0.5 overflow-hidden rounded-control border border-line bg-elevated p-1 text-left transition-colors hover:border-accent"
                  style={on ? { borderColor: 'var(--accent)' } : undefined}
                  tabIndex={i === current ? 0 : -1}
                  aria-pressed={on}
                  onFocus={() => setFocus(i)}
                  onClick={() => filterTo(b)}
                  onDoubleClick={() => zoomIn(b)}
                  onKeyDown={(e) => onKey(e, i, b)}
                  data-testid="timeline-bucket"
                  data-bucket={b.key}
                >
                  {b.photo_ids[0] !== undefined && (
                    <img src={thumbUrlById(b.photo_ids[0])} alt="" loading="lazy" draggable={false} className="pointer-events-none absolute inset-x-0 top-0 h-[46px] w-full object-cover opacity-80" />
                  )}
                  <span className="relative block truncate text-xs font-medium">{short}</span>{' '}
                  <span className="relative flex items-center gap-1">
                    <span className="block h-1.5 rounded-full bg-accent" style={{ width: `${Math.round(barRatio(b.count, max) * 100)}%` }} aria-hidden />
                    <span className="tnum text-[11px] text-faint">{b.count.toLocaleString(i18n.language)}</span>
                  </span>
                  <span className="sr-only">{`, ${name}`}</span>
                </button>
                {next && (
                  <button
                    className="btn btn-ghost btn-icon absolute top-0.5 right-0.5 !h-5 !w-5 bg-panel"
                    tabIndex={-1}
                    aria-label={t('timelineView.zoomIn', { label })}
                    title={t('timelineView.zoomIn', { label })}
                    onClick={() => zoomIn(b)}
                    data-testid="timeline-zoom-in"
                  >
                    <ZoomIn size={11} />
                  </button>
                )}
              </div>
            )
          })}
        </div>
      )}

      {data && data.undated.count > 0 && (
        <p className="mt-1 text-xs text-faint" data-testid="timeline-undated">
          {t('timelineView.undated', { n: data.undated.count.toLocaleString(i18n.language) })}
        </p>
      )}

      {data && data.events.length > 0 && (
        <div className="mt-1 flex flex-wrap items-center gap-1" role="group" aria-label={t('timelineView.events')} data-testid="timeline-events">
          <span className="text-[11px] font-semibold tracking-wide text-faint uppercase">{t('timelineView.events')}</span>
          {data.events.map((ev) => (
            <button
              key={ev.id}
              className="btn hit !h-6 max-w-[16rem]"
              aria-pressed={ev.session_id === sessionId && eventId === ev.id}
              aria-label={t('timelineView.eventOpen', { name: ev.name })}
              onClick={() => openEvent(ev)}
              data-testid="timeline-event"
              data-event={ev.id}
            >
              <span className="truncate">{ev.name}</span>
              <span className="tnum text-xs text-faint">{ev.photo_count}</span>
            </button>
          ))}
        </div>
      )}
    </section>
  )
}
