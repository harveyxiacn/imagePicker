import { Check, Pencil, RefreshCw, RotateCcw } from 'lucide-react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useDeviceOffsets, useDeviceOffsetsMutation, useNotGuest } from '@/api/queries'
import type { DeviceOffset } from '@/api/types'
import {
  acceptBody,
  acceptableDevices,
  canAccept,
  checkOffsetInput,
  confidenceLevel,
  confidencePercent,
  formatOffset,
  hasCorrection,
  needsAlignment,
} from '@/lib/timeAlign'
import { DeviceKindIcon } from './DeviceFilterPopover'

/**
 * Clock offsets of the devices of a session (docs/09 P1-2): the estimate against the other devices,
 * accept it, type an offset or reset. Corrections only move the capture time used for burst grouping
 * and events; files and EXIF stay as they are. Shown for sessions with two or more devices.
 */
export function DeviceTimePanel({ sessionId, open }: { sessionId: number; open: boolean }) {
  const { t, i18n } = useTranslation()
  const owner = useNotGuest()
  const q = useDeviceOffsets(sessionId, open)
  const m = useDeviceOffsetsMutation(sessionId)
  const [editing, setEditing] = useState<number | null>(null)
  const [text, setText] = useState('')

  if (q.isLoading) return <p className="mt-2 border-t border-line pt-2 text-xs text-faint">{t('timeAlign.estimating')}</p>
  const data = q.data
  // an older server (or a guest-less mock) has no such endpoint: leave the popover as it was
  if (!data || !needsAlignment(data.devices)) return null
  const devices = data.devices
  const busy = m.isPending
  const checked = checkOffsetInput(text)
  const fmtPct = (c: number) => `${confidencePercent(c).toLocaleString(i18n.language)}%`

  const startEdit = (d: DeviceOffset) => {
    setEditing(d.device_id)
    setText(d.offset_ms !== 0 ? formatOffset(d.offset_ms) : (d.estimate && d.estimate.source !== 'reference' ? formatOffset(d.estimate.offset_ms) : ''))
  }
  const submit = (d: DeviceOffset) => {
    if (checked.check !== 'ok' || checked.ms === null) return
    m.mutate({ action: 'set', device_id: d.device_id, offset_ms: checked.ms }, { onSuccess: () => setEditing(null) })
  }
  const toAccept = acceptableDevices(devices)

  return (
    <section className="mt-2 border-t border-line pt-2" aria-label={t('timeAlign.title')} data-testid="time-align-panel">
      <div className="flex items-center justify-between gap-2">
        <h3 className="text-xs font-medium text-muted">{t('timeAlign.title')}</h3>
        <button className="btn btn-ghost btn-icon" title={t('timeAlign.reestimate')} aria-label={t('timeAlign.reestimate')} disabled={busy} onClick={() => m.mutate({ refresh: true })}>
          <RefreshCw size={13} />
        </button>
      </div>
      <p className="mt-1 text-xs text-faint">{t('timeAlign.hint')}</p>
      <ul className="mt-1.5 flex max-h-[220px] flex-col gap-1.5 overflow-y-auto">
        {devices.map((d) => {
          const e = d.estimate
          return (
            <li key={d.device_id} className="rounded-control border border-line px-2 py-1.5" data-testid={`time-align-device-${d.device_id}`}>
              <div className="flex items-center gap-2">
                <DeviceKindIcon kind={d.kind} className="shrink-0 text-muted" />
                <span className="min-w-0 flex-1 truncate" title={d.name}>
                  {d.name}
                </span>
                <span className="tnum text-xs" data-testid="time-align-applied">
                  {d.offset_ms !== 0 ? formatOffset(d.offset_ms) : t('timeAlign.none')}
                </span>
              </div>
              {d.offset_ms !== 0 && <div className="mt-0.5 text-xs text-faint">{t(`timeAlign.source_${d.source}`)}</div>}
              <div className="mt-0.5 text-xs text-muted" data-testid="time-align-estimate">
                {!e && t('timeAlign.noEstimate')}
                {e && e.source === 'reference' && t('timeAlign.reference')}
                {e && e.source !== 'reference' && (
                  <>
                    {t('timeAlign.estimate')} <span className="tnum text-fg">{formatOffset(e.offset_ms)}</span> {'· '}
                    <span className={confidenceLevel(e.confidence) === 'low' ? 'text-warning' : ''}>
                      {t(`timeAlign.conf_${confidenceLevel(e.confidence)}`)} {fmtPct(e.confidence)}
                    </span>{' '}
                    {'· '}
                    {t(`timeAlign.est_${e.source}`, { n: e.evidence })}
                  </>
                )}
              </div>
              {editing === d.device_id ? (
                <div className="mt-1.5 flex flex-col gap-1">
                  <div className="flex items-center gap-1.5">
                    <input
                      className="field min-w-0 flex-1"
                      value={text}
                      autoFocus
                      aria-label={t('timeAlign.inputLabel')}
                      placeholder="-1h 02m 05s"
                      onChange={(ev) => setText(ev.target.value)}
                      onKeyDown={(ev) => {
                        if (ev.key === 'Enter') submit(d)
                        if (ev.key === 'Escape') {
                          ev.stopPropagation()
                          setEditing(null)
                        }
                      }}
                      data-testid="time-align-input"
                    />
                    <button className="btn btn-primary" disabled={busy || checked.check !== 'ok'} onClick={() => submit(d)} data-testid="time-align-set">
                      {t('timeAlign.set')}
                    </button>
                    <button className="btn btn-ghost" onClick={() => setEditing(null)}>
                      {t('timeAlign.cancel')}
                    </button>
                  </div>
                  <span className={`text-xs ${text && checked.check !== 'ok' ? 'text-warning' : 'text-faint'}`} role={text && checked.check !== 'ok' ? 'alert' : undefined}>
                    {checked.check === 'invalid' && text ? t('timeAlign.invalid') : checked.check === 'too_large' ? t('timeAlign.tooLarge') : t('timeAlign.inputHint')}
                  </span>
                </div>
              ) : (
                owner && (
                  <div className="mt-1 flex flex-wrap justify-end gap-1">
                    {canAccept(d) && (
                      <button className="btn btn-ghost" disabled={busy} onClick={() => m.mutate(acceptBody(devices, d.device_id))} data-testid="time-align-accept">
                        <Check size={13} />
                        {t('timeAlign.accept')}
                      </button>
                    )}
                    <button className="btn btn-ghost" disabled={busy} onClick={() => startEdit(d)} data-testid="time-align-edit">
                      <Pencil size={13} />
                      {t('timeAlign.edit')}
                    </button>
                    {d.offset_ms !== 0 && (
                      <button className="btn btn-ghost" disabled={busy} onClick={() => m.mutate({ action: 'reset', device_id: d.device_id })} data-testid="time-align-reset">
                        <RotateCcw size={13} />
                        {t('timeAlign.reset')}
                      </button>
                    )}
                  </div>
                )
              )}
            </li>
          )
        })}
      </ul>
      {owner && (toAccept.length > 0 || hasCorrection(devices)) && (
        <div className="mt-1.5 flex justify-end gap-2">
          {hasCorrection(devices) && (
            <button className="btn btn-ghost" disabled={busy} onClick={() => m.mutate({ action: 'reset' })} data-testid="time-align-reset-all">
              {t('timeAlign.resetAll')}
            </button>
          )}
          {toAccept.length > 0 && (
            <button className="btn btn-primary" disabled={busy} onClick={() => m.mutate(acceptBody(devices))} data-testid="time-align-accept-all">
              {t('timeAlign.acceptAll', { n: toAccept.length })}
            </button>
          )}
        </div>
      )}
    </section>
  )
}
