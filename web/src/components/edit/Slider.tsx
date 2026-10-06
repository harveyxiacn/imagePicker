import { memo, useEffect, useRef, useState } from 'react'
import { useEdit } from '@/stores/edit'

interface Props {
  label: string
  value: number
  min: number
  max: number
  step: number
  decimals?: number
  /** neutral value restored by double-click */
  def?: number
  /** live update while dragging / typing / wheel (no history) */
  onLive: (v: number) => void
  /** end of a gesture: pointer up, key up, wheel idle, number committed */
  onCommit: () => void
  /** CSS gradient for the track (temp / tint / HSL); default track shows a fill from `def` to the value */
  track?: string
  /** highlight (AI apply diff) */
  flash?: boolean
  testId?: string
  /** suffix such as `°` or `%` */
  unit?: string
  disabled?: boolean
}

const WHEEL_IDLE_MS = 380

function fmt(v: number, decimals: number, signed: boolean): string {
  const s = v.toFixed(decimals)
  return signed && v > 0 ? `+${s}` : s
}

/** Lightroom-style slider: double-click resets, wheel / arrows fine-tune, numeric input, Alt-drag clipping stub. */
export const Slider = memo(function Slider({ label, value, min, max, step, decimals = 0, def = 0, onLive, onCommit, track, flash, testId, unit, disabled }: Props) {
  const [draft, setDraft] = useState<string | null>(null)
  const wheelTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const cancelled = useRef(false)
  const latest = useRef({ value, min, max, step, onLive, onCommit })
  useEffect(() => {
    latest.current = { value, min, max, step, onLive, onCommit }
  })

  // Wheel over the track nudges by one step (Shift = x10); a burst of ticks commits once.
  useEffect(() => {
    const el = inputRef.current
    if (!el) return
    const onWheel = (e: WheelEvent) => {
      e.preventDefault()
      const l = latest.current
      const dir = e.deltaY < 0 ? 1 : -1
      const next = Math.min(l.max, Math.max(l.min, Math.round((l.value + dir * l.step * (e.shiftKey ? 10 : 1)) / l.step) * l.step))
      l.onLive(Number(next.toFixed(6)))
      if (wheelTimer.current) clearTimeout(wheelTimer.current)
      wheelTimer.current = setTimeout(() => latest.current.onCommit(), WHEEL_IDLE_MS)
    }
    el.addEventListener('wheel', onWheel, { passive: false })
    return () => {
      el.removeEventListener('wheel', onWheel)
      if (wheelTimer.current) clearTimeout(wheelTimer.current)
    }
  }, [])

  const pct = (v: number) => ((v - min) / (max - min)) * 100
  const a = pct(Math.min(value, def))
  const b = pct(Math.max(value, def))
  const fill = `linear-gradient(to right, var(--line) ${a}%, var(--accent) ${a}%, var(--accent) ${b}%, var(--line) ${b}%)`
  const changed = Math.abs(value - def) > 1e-9
  const signed = min < 0

  const reset = () => {
    onLive(def)
    onCommit()
  }
  const commitDraft = () => {
    if (draft === null) return
    if (cancelled.current) {
      cancelled.current = false
      setDraft(null)
      return
    }
    const n = Number(draft)
    setDraft(null)
    if (Number.isFinite(n) && draft.trim() !== '') {
      onLive(Math.min(max, Math.max(min, n)))
      onCommit()
    }
  }

  return (
    <div className={`flex items-center gap-2 py-[3px] ${flash ? 'anim-flash' : ''} ${disabled ? 'pointer-events-none opacity-40' : ''}`} data-testid={testId} aria-disabled={disabled || undefined}>
      <button
        type="button"
        tabIndex={-1}
        className={`w-[72px] shrink-0 truncate text-left text-[12px] ${changed ? 'text-fg' : 'text-muted'}`}
        onDoubleClick={reset}
        title={label}
      >
        {label}
      </button>
      <input
        ref={inputRef}
        type="range"
        className="lr-slider min-w-0 flex-1"
        min={min}
        max={max}
        step={step}
        value={value}
        data-changed={changed}
        disabled={disabled}
        aria-label={label}
        style={{ ['--track' as string]: track ?? fill }}
        onChange={(e) => onLive(Number(e.target.value))}
        onPointerDown={(e) => {
          useEdit.getState().setDragging(true)
          if (e.altKey) useEdit.getState().setClipping(true)
        }}
        onPointerUp={() => {
          useEdit.getState().setDragging(false)
          useEdit.getState().setClipping(false)
          onCommit()
        }}
        onPointerCancel={() => {
          useEdit.getState().setDragging(false)
          useEdit.getState().setClipping(false)
          onCommit()
        }}
        onKeyUp={(e) => {
          if (e.key.startsWith('Arrow') || e.key === 'Home' || e.key === 'End' || e.key === 'PageUp' || e.key === 'PageDown') onCommit()
        }}
        onDoubleClick={reset}
      />
      <input
        className="lr-num"
        inputMode="decimal"
        disabled={disabled}
        aria-label={`${label} (${unit ?? '#'})`}
        value={draft ?? fmt(value, decimals, signed) + (unit ?? '')}
        onFocus={(e) => {
          setDraft(String(Number(value.toFixed(decimals))))
          e.currentTarget.select()
        }}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commitDraft}
        onKeyDown={(e) => {
          if (e.key === 'Enter') e.currentTarget.blur()
          else if (e.key === 'Escape') {
            cancelled.current = true
            e.currentTarget.blur()
          }
        }}
      />
    </div>
  )
})
