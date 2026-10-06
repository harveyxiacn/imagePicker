import { useQueryClient } from '@tanstack/react-query'
import { RotateCcw } from 'lucide-react'
import { useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { HSL_BANDS, type HslBand } from '@/api/types'
import { getAdjust, getHsl, setGradingBalance, setGradingWheel, setHsl, updateGlobal, type Wheel } from '@/lib/edit'
import { changeStack, commitLive, setLive } from '@/lib/editActions'
import { useEdit } from '@/stores/edit'
import { Section } from './Section'
import { Slider } from './Slider'

const BAND_HUE: Record<HslBand, number> = { red: 0, orange: 30, yellow: 60, green: 120, aqua: 180, blue: 240, purple: 270, magenta: 320 }

type Tab = 'h' | 's' | 'l'

function bandTrack(band: HslBand, tab: Tab): string {
  const h = BAND_HUE[band]
  if (tab === 'h') return `linear-gradient(to right, hsl(${h - 35} 80% 50%), hsl(${h} 80% 50%), hsl(${h + 35} 80% 50%))`
  if (tab === 's') return `linear-gradient(to right, hsl(${h} 0% 50%), hsl(${h} 90% 50%))`
  return `linear-gradient(to right, #000, hsl(${h} 85% 50%), #fff)`
}

/** HSL: 8 colour bands x H / S / L tabs. */
export function HslPanel() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const stack = useEdit((s) => s.stack)
  const [tab, setTab] = useState<Tab>('h')
  const adjust = getAdjust(stack)
  const dirty = !!adjust.hsl && Object.keys(adjust.hsl).length > 0

  return (
    <Section
      id="hsl"
      title={t('edit.hsl')}
      defaultCollapsed
      right={
        <button
          className="btn btn-ghost btn-icon !h-6 !w-6"
          disabled={!dirty}
          title={t('edit.resetSection')}
          aria-label={t('edit.resetSection')}
          onClick={() => changeStack(qc, updateGlobal(useEdit.getState().stack, (a) => ({ ...a, hsl: undefined })), t('history.hsl'))}
        >
          <RotateCcw size={12} />
        </button>
      }
    >
      <div className="mb-1.5 flex gap-1" role="tablist">
        {(['h', 's', 'l'] as Tab[]).map((k) => (
          <button key={k} role="tab" aria-selected={tab === k} aria-pressed={tab === k} className="chip" onClick={() => setTab(k)} data-testid={`hsl-tab-${k}`}>
            {t(`edit.hsl_${k}`)}
          </button>
        ))}
      </div>
      <div data-testid="hsl-sliders">
        {HSL_BANDS.map((band) => (
          <Slider
            key={band}
            testId={`hsl-${band}-${tab}`}
            label={t(`edit.band_${band}`)}
            value={getHsl(adjust, band)[tab]}
            min={-100}
            max={100}
            step={1}
            track={bandTrack(band, tab)}
            onLive={(v) => setLive(updateGlobal(useEdit.getState().stack, (a) => setHsl(a, band, tab, v)))}
            onCommit={() => commitLive(qc, t('history.hslBand', { band: t(`edit.band_${band}`) }), `hsl:${band}:${tab}`)}
          />
        ))}
      </div>
    </Section>
  )
}

interface WheelProps {
  label: string
  hue: number
  amount: number
  onLive: (hue: number, amount: number) => void
  onCommit: () => void
  testId?: string
}

/** Colour wheel: angle = hue, distance from the centre = amount (0..1). Double-click resets. */
export function ColorWheel({ label, hue, amount, onLive, onCommit, testId }: WheelProps) {
  const ref = useRef<HTMLDivElement>(null)
  const dragging = useRef(false)
  const pick = (e: { clientX: number; clientY: number }) => {
    const r = ref.current!.getBoundingClientRect()
    const dx = e.clientX - (r.left + r.width / 2)
    const dy = e.clientY - (r.top + r.height / 2)
    const h = ((Math.atan2(dx, -dy) * 180) / Math.PI + 360) % 360
    const a = Math.min(1, Math.hypot(dx, dy) / (r.width / 2))
    onLive(h, a)
  }
  const rad = ((hue - 90) * Math.PI) / 180
  const hx = 50 + Math.cos(rad) * amount * 50
  const hy = 50 + Math.sin(rad) * amount * 50
  return (
    <div className="flex flex-col items-center gap-1" data-testid={testId}>
      <div
        ref={ref}
        className="relative aspect-square w-full max-w-[88px] touch-none rounded-full border border-line"
        style={{
          background:
            'radial-gradient(circle, rgb(128 128 128) 0%, rgb(128 128 128 / 0) 70%), conic-gradient(from 0deg, hsl(0 85% 55%), hsl(60 85% 55%), hsl(120 85% 55%), hsl(180 85% 55%), hsl(240 85% 55%), hsl(300 85% 55%), hsl(360 85% 55%))',
          cursor: 'crosshair',
        }}
        onPointerDown={(e) => {
          dragging.current = true
          e.currentTarget.setPointerCapture(e.pointerId)
          useEdit.getState().setDragging(true)
          pick(e)
        }}
        onPointerMove={(e) => dragging.current && pick(e)}
        onPointerUp={() => {
          if (!dragging.current) return
          dragging.current = false
          useEdit.getState().setDragging(false)
          onCommit()
        }}
        onDoubleClick={() => {
          onLive(0, 0)
          onCommit()
        }}
        role="slider"
        aria-label={label}
        aria-valuenow={Math.round(hue)}
      >
        <span
          className="pointer-events-none absolute h-3 w-3 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-white shadow"
          style={{ left: `${hx}%`, top: `${hy}%`, background: amount > 0 ? `hsl(${hue} 90% 55%)` : 'transparent' }}
        />
      </div>
      <div className="text-[11px] text-muted">{label}</div>
      <div className="tnum text-[11px] text-faint">
        {Math.round(hue)}° · {Math.round(amount * 100)}
      </div>
    </div>
  )
}

const WHEELS: Wheel[] = ['shadows', 'midtones', 'highlights']

/** Colour grading: three wheels + balance. */
export function GradingPanel() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const stack = useEdit((s) => s.stack)
  const adjust = getAdjust(stack)
  const g = adjust.grading
  const dirty = !!g

  return (
    <Section
      id="grading"
      title={t('edit.grading')}
      defaultCollapsed
      right={
        <button
          className="btn btn-ghost btn-icon !h-6 !w-6"
          disabled={!dirty}
          title={t('edit.resetSection')}
          aria-label={t('edit.resetSection')}
          onClick={() => changeStack(qc, updateGlobal(useEdit.getState().stack, (a) => ({ ...a, grading: undefined })), t('history.grading'))}
        >
          <RotateCcw size={12} />
        </button>
      }
    >
      <div className="grid grid-cols-3 gap-2" data-testid="grading-wheels">
        {WHEELS.map((w) => (
          <ColorWheel
            key={w}
            testId={`wheel-${w}`}
            label={t(`edit.wheel_${w}`)}
            hue={g?.[w]?.[0] ?? 0}
            amount={g?.[w]?.[1] ?? 0}
            onLive={(h, a) => setLive(updateGlobal(useEdit.getState().stack, (adj) => setGradingWheel(adj, w, h, a)))}
            onCommit={() => commitLive(qc, t('history.gradingWheel', { name: t(`edit.wheel_${w}`) }), `grading:${w}`)}
          />
        ))}
      </div>
      <div className="mt-2">
        <Slider
          testId="grading-balance"
          label={t('edit.balance')}
          value={g?.balance ?? 0}
          min={-100}
          max={100}
          step={1}
          onLive={(v) => setLive(updateGlobal(useEdit.getState().stack, (a) => setGradingBalance(a, v)))}
          onCommit={() => commitLive(qc, t('history.gradingBalance'), 'grading:balance')}
        />
      </div>
    </Section>
  )
}
