import { Aperture, Camera, CameraOff, Plane, Smartphone, Video, type LucideIcon } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useDevices, useSession } from '@/api/queries'
import type { DeviceKind } from '@/api/types'
import { toggleDevice } from '@/lib/filter'
import { useUi } from '@/stores/ui'

const ICONS: Record<DeviceKind, LucideIcon> = {
  phone: Smartphone,
  camera: Camera,
  drone: Plane,
  action: Video,
  unknown: Aperture,
}

/** Icon for a device kind (phone / camera / drone / action / unknown). */
export function DeviceKindIcon({ kind, size = 13, className }: { kind: DeviceKind | null | undefined; size?: number; className?: string }) {
  const { t } = useTranslation()
  const k = kind ?? 'unknown'
  const Icon = ICONS[k]
  return <Icon size={size} className={className} aria-label={t(`device.kind_${k}`)} role="img">
    <title>{t(`device.kind_${k}`)}</title>
  </Icon>
}

/** Device filter: button + popover with multi-select checkboxes. Hidden for single-device sessions. */
export function DeviceFilterButton({ sessionId }: { sessionId: number }) {
  const { t, i18n } = useTranslation()
  const session = useSession(sessionId)
  const devices = useDevices(sessionId, session.data?.photo_count)
  const selected = useUi((s) => s.filter.device)
  const setFilter = useUi((s) => s.setFilter)
  const [open, setOpen] = useState(false)
  const wrap = useRef<HTMLDivElement>(null)

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (wrap.current && !wrap.current.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    window.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      window.removeEventListener('keydown', onKey)
    }
  }, [open])

  const list = devices.data ?? []
  const withDevice = list.reduce((n, d) => n + d.photo_count, 0)
  const noneCount = Math.max(0, (session.data?.photo_count ?? withDevice) - withDevice)
  // keep the control reachable while a filter is active, otherwise hide for trivial sessions
  if (selected.length === 0 && list.length <= 1 && noneCount === 0) return null
  const fmt = (n: number) => n.toLocaleString(i18n.language)

  return (
    <div className="relative" ref={wrap}>
      <button
        className="btn"
        aria-pressed={selected.length > 0 || open}
        aria-haspopup="dialog"
        aria-expanded={open}
        title={t('device.filter')}
        onClick={() => setOpen(!open)}
        data-testid="device-filter-button"
      >
        <Camera size={14} />
        {t('device.filter')}
        {selected.length > 0 && <span className="tnum text-accent">({selected.length})</span>}
      </button>
      {open && (
        <div
          role="dialog"
          aria-label={t('device.filter')}
          className="anim-pop absolute top-full left-0 z-40 mt-1.5 w-[300px] max-w-[calc(100vw-24px)] rounded-card border border-line bg-elevated p-3 shadow-[var(--shadow)]"
          data-testid="device-filter-popover"
        >
          <ul className="flex max-h-[260px] flex-col gap-0.5 overflow-y-auto">
            {list.map((d) => (
              <li key={d.id}>
                <label className="flex cursor-pointer items-center gap-2 rounded-control px-1.5 py-1 hover:bg-panel">
                  <input type="checkbox" checked={selected.includes(d.id)} onChange={() => setFilter({ device: toggleDevice(selected, d.id) })} />
                  <DeviceKindIcon kind={d.kind} className="shrink-0 text-muted" />
                  <span className="min-w-0 flex-1 truncate" title={d.name}>
                    {d.name}
                  </span>
                  <span className="tnum text-xs text-faint">{fmt(d.photo_count)}</span>
                </label>
              </li>
            ))}
            {noneCount > 0 && (
              <li>
                <label className="flex cursor-pointer items-center gap-2 rounded-control px-1.5 py-1 hover:bg-panel">
                  <input type="checkbox" checked={selected.includes('none')} onChange={() => setFilter({ device: toggleDevice(selected, 'none') })} />
                  <CameraOff size={13} className="shrink-0 text-muted" />
                  <span className="min-w-0 flex-1 truncate">{t('device.none')}</span>
                  <span className="tnum text-xs text-faint">{fmt(noneCount)}</span>
                </label>
              </li>
            )}
          </ul>
          <div className="mt-2 flex justify-end gap-2 border-t border-line pt-2">
            <button className="btn btn-ghost" disabled={selected.length === 0} onClick={() => setFilter({ device: [] })}>
              {t('device.reset')}
            </button>
            <button className="btn btn-primary" onClick={() => setOpen(false)}>
              {t('person.done')}
            </button>
          </div>
        </div>
      )}
    </div>
  )
}
