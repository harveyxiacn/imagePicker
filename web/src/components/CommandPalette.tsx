import * as Dialog from '@radix-ui/react-dialog'
import { Search } from 'lucide-react'
import { useEffect, useMemo, useState, type KeyboardEvent as ReactKeyboardEvent } from 'react'
import { useTranslation } from 'react-i18next'
import { matchPath, useLocation, useNavigate } from 'react-router-dom'
import { eventToSpec } from '@/lib/keymap'
import { useBindings } from '@/lib/shortcuts'
import { buildItems, paletteSpecs, rankItems, type PaletteItem } from '@/lib/palette'
import { getPaletteHost } from '@/lib/paletteHost'
import { useUi } from '@/stores/ui'

/**
 * Ctrl+K command palette: fuzzy search over the actions of the central keymap (what the current screen can run)
 * plus navigation targets. The key itself is looked up in the keymap, so rebinding it there is enough.
 */
export function CommandPalette() {
  const open = useUi((s) => s.paletteOpen)
  const setOpen = useUi((s) => s.setPaletteOpen)

  useEffect(() => {
    const specs = paletteSpecs()
    const onKey = (e: KeyboardEvent) => {
      if (!specs.includes(eventToSpec(e))) return
      // another dialog (export, help...) keeps the keyboard; the palette only toggles itself
      const other = document.querySelector('[role="dialog"]:not([data-palette])')
      if (other) return
      e.preventDefault()
      const ui = useUi.getState()
      ui.setPaletteOpen(!ui.paletteOpen)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  return (
    <Dialog.Root open={open} onOpenChange={setOpen}>
      <Dialog.Portal>
        <Dialog.Overlay className="anim-fade fixed inset-0 z-(--z-scrim) bg-black/60" />
        <Dialog.Content
          data-palette=""
          aria-describedby={undefined}
          className="anim-pop fixed top-[14vh] left-1/2 z-(--z-modal) flex palette-max-h w-[calc(100vw-32px)] max-w-xl -translate-x-1/2 flex-col overflow-hidden rounded-card border border-line bg-panel shadow-[var(--shadow)]"
          data-testid="command-palette"
        >
          <PaletteBody onClose={() => setOpen(false)} />
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  )
}

function PaletteBody({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const { pathname } = useLocation()
  const [query, setQuery] = useState('')
  const [cursor, setCursor] = useState(0)

  const bindings = useBindings()
  const host = useMemo(() => getPaletteHost(), [])
  const sessionId = useMemo(() => {
    const m = matchPath({ path: '/s/:sessionId', end: false }, pathname)
    const n = Number(m?.params.sessionId)
    return Number.isFinite(n) && m ? n : null
  }, [pathname])
  const items = useMemo(
    () =>
      buildItems({
        scope: host?.scope() ?? 'grid',
        t: (k) => t(k),
        available: (id) => !!host?.has(id),
        sessionId,
        bindings,
      }),
    [host, sessionId, t, bindings],
  )
  const results = useMemo(() => rankItems(items, query), [items, query])
  const active = Math.min(cursor, Math.max(0, results.length - 1))
  const optionId = (i: number) => `palette-opt-${i}`

  useEffect(() => {
    document.getElementById(optionId(active))?.scrollIntoView?.({ block: 'nearest' })
  }, [active, results])

  const run = (item: PaletteItem) => {
    onClose()
    if (item.kind === 'nav' && item.path) navigate(item.path)
    else if (host) setTimeout(() => host.run(item.id), 0)
  }

  const onKeyDown = (e: ReactKeyboardEvent) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setCursor(Math.min(results.length - 1, active + 1))
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      setCursor(Math.max(0, active - 1))
    } else if (e.key === 'Home' && e.ctrlKey) {
      setCursor(0)
    } else if (e.key === 'Enter' && !e.nativeEvent.isComposing) {
      e.preventDefault()
      const item = results[active]
      if (item) run(item)
    }
  }

  return (
    <>
      <Dialog.Title className="sr-only">{t('palette.title')}</Dialog.Title>
      <div className="flex items-center gap-2 border-b border-line px-3">
        <Search size={15} className="shrink-0 text-muted" aria-hidden />
        <input
          autoFocus
          role="combobox"
          aria-expanded
          aria-controls="palette-list"
          aria-activedescendant={results.length ? optionId(active) : undefined}
          aria-autocomplete="list"
          aria-label={t('palette.title')}
          className="h-11 w-full bg-transparent text-base text-fg placeholder:text-muted focus-visible:outline-offset-[-2px]"
          placeholder={t('palette.placeholder')}
          value={query}
          onChange={(e) => {
            setQuery(e.target.value)
            setCursor(0)
          }}
          onKeyDown={onKeyDown}
          data-testid="palette-input"
        />
      </div>
      <ul id="palette-list" role="listbox" aria-label={t('palette.title')} className="min-h-0 flex-1 overflow-y-auto p-1" data-testid="palette-list">
        {results.map((it, i) => (
          <li
            key={it.id}
            id={optionId(i)}
            role="option"
            aria-selected={i === active}
            className={`flex cursor-pointer items-center justify-between gap-3 rounded-control px-3 py-1.5 ${i === active ? 'bg-accent text-accent-fg' : 'hover:bg-elevated'}`}
            onMouseMove={() => i !== active && setCursor(i)}
            onClick={() => run(it)}
            data-testid="palette-item"
          >
            <span className="min-w-0 truncate">
              {it.label}
              <span className={`ml-2 text-xs ${i === active ? 'opacity-80' : 'text-muted'}`}>{it.group}</span>
            </span>
            {it.keys.length > 0 && (
              <span className="flex shrink-0 items-center gap-1">
                {it.keys.slice(0, 2).map((combo, ci) => (
                  <span key={ci} className="flex items-center gap-0.5">
                    {combo.map((k, j) => (
                      <kbd key={j}>{k}</kbd>
                    ))}
                  </span>
                ))}
              </span>
            )}
          </li>
        ))}
        {results.length === 0 && (
          <li role="presentation" className="px-3 py-6 text-center text-muted" data-testid="palette-empty">
            {t('palette.empty')}
          </li>
        )}
      </ul>
      <div className="flex items-center justify-between border-t border-line px-3 py-1.5 text-xs text-muted" aria-live="polite">
        <span>{t('palette.hint')}</span>
        <span>{t('palette.results', { n: results.length })}</span>
      </div>
    </>
  )
}
