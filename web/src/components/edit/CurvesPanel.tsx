import { useQueryClient } from '@tanstack/react-query'
import { RotateCcw } from 'lucide-react'
import { useRef, useState, type PointerEvent as RPointerEvent } from 'react'
import { useTranslation } from 'react-i18next'
import type { CurveChannel, Point } from '@/api/types'
import { addCurvePoint, curvePath, hitCurvePoint, IDENTITY_CURVE, moveCurvePoint, removeCurvePoint } from '@/lib/curves'
import { getAdjust, setCurve, updateGlobal } from '@/lib/edit'
import { changeStack, commitLive, setLive } from '@/lib/editActions'
import { useEdit } from '@/stores/edit'
import { Section } from './Section'

const CHANNELS: { id: CurveChannel; color: string; label: string }[] = [
  { id: 'rgb', color: '#e8e8e8', label: 'RGB' },
  { id: 'r', color: '#e5534b', label: 'R' },
  { id: 'g', color: '#3fb950', label: 'G' },
  { id: 'b', color: '#4c8dff', label: 'B' },
]

interface EditorProps {
  points: Point[]
  color: string
  onChange: (p: Point[]) => void
  onCommit: () => void
}

const HIT = 0.05
const PAD = 5

/** Tone-curve editor: drag points, click to add, double-click / right-click / Delete to remove. */
export function CurveEditor({ points, color, onChange, onCommit }: EditorProps) {
  const ref = useRef<SVGSVGElement>(null)
  const drag = useRef<number | null>(null)
  const [selected, setSelected] = useState<number | null>(null)
  const pts = points.length ? points : IDENTITY_CURVE

  const unit = (e: { clientX: number; clientY: number }) => {
    const r = ref.current!.getBoundingClientRect()
    // the viewBox has a PAD margin so end points are not clipped
    const vb = 100 + 2 * PAD
    return { x: ((e.clientX - r.left) / r.width) * (vb / 100) - PAD / 100, y: 1 - (((e.clientY - r.top) / r.height) * (vb / 100) - PAD / 100) }
  }
  const onDown = (e: RPointerEvent<SVGSVGElement>) => {
    if (e.button !== 0) return
    const { x, y } = unit(e)
    let idx = hitCurvePoint(pts, x, y, HIT)
    if (idx < 0) {
      const res = addCurvePoint(pts, x, y)
      idx = res.index
      onChange(res.points)
    }
    drag.current = idx
    setSelected(idx)
    e.currentTarget.setPointerCapture(e.pointerId)
    useEdit.getState().setDragging(true)
  }
  const onMove = (e: RPointerEvent<SVGSVGElement>) => {
    if (drag.current === null) return
    const { x, y } = unit(e)
    onChange(moveCurvePoint(pts, drag.current, x, y))
  }
  const onUp = () => {
    if (drag.current === null) return
    drag.current = null
    useEdit.getState().setDragging(false)
    onCommit()
  }
  const remove = (idx: number) => {
    if (idx <= 0 || idx >= pts.length - 1) return
    onChange(removeCurvePoint(pts, idx))
    setSelected(null)
    onCommit()
  }

  return (
    <svg
      ref={ref}
      viewBox={`${-PAD} ${-PAD} ${100 + 2 * PAD} ${100 + 2 * PAD}`}
      className="aspect-square w-full touch-none overflow-visible rounded-control border border-line bg-bg"
      tabIndex={0}
      role="application"
      aria-label="curve"
      data-testid="curve-editor"
      onPointerDown={onDown}
      onPointerMove={onMove}
      onPointerUp={onUp}
      onPointerCancel={onUp}
      onDoubleClick={(e) => {
        const { x, y } = unit(e)
        remove(hitCurvePoint(pts, x, y, HIT))
      }}
      onContextMenu={(e) => {
        e.preventDefault()
        const { x, y } = unit(e)
        remove(hitCurvePoint(pts, x, y, HIT))
      }}
      onKeyDown={(e) => {
        if ((e.key === 'Delete' || e.key === 'Backspace') && selected !== null) {
          e.preventDefault()
          remove(selected)
        }
      }}
    >
      {[25, 50, 75].map((g) => (
        <g key={g} stroke="var(--line)" strokeWidth="0.4">
          <line x1={g} y1="0" x2={g} y2="100" />
          <line x1="0" y1={g} x2="100" y2={g} />
        </g>
      ))}
      <line x1="0" y1="100" x2="100" y2="0" stroke="var(--faint)" strokeWidth="0.5" strokeDasharray="2 2" />
      <path d={curvePath(pts, 100, 100)} fill="none" stroke={color} strokeWidth="1.4" strokeLinejoin="round" data-testid="curve-path" />
      {pts.map(([x, y], i) => (
        <circle
          key={i}
          cx={x * 100}
          cy={(1 - y) * 100}
          r={i === selected ? 2.6 : 2.2}
          fill={i === selected ? 'var(--accent)' : 'var(--bg)'}
          stroke={i === selected ? 'var(--accent)' : color}
          strokeWidth="1"
          data-testid="curve-point"
        />
      ))}
    </svg>
  )
}

export function CurvesPanel() {
  const { t } = useTranslation()
  const qc = useQueryClient()
  const stack = useEdit((s) => s.stack)
  const [ch, setCh] = useState<CurveChannel>('rgb')
  const adjust = getAdjust(stack)
  const meta = CHANNELS.find((c) => c.id === ch)!
  const points = adjust.curve?.[ch] ?? []
  const dirty = !!adjust.curve && Object.values(adjust.curve).some((p) => p && p.length)

  return (
    <Section
      id="curves"
      title={t('edit.curves')}
      defaultCollapsed
      right={
        <button
          className="btn btn-ghost btn-icon !h-6 !w-6"
          disabled={!dirty}
          title={t('edit.resetSection')}
          aria-label={t('edit.resetSection')}
          onClick={() => changeStack(qc, updateGlobal(useEdit.getState().stack, (a) => ({ ...a, curve: undefined })), t('history.curves'))}
        >
          <RotateCcw size={12} />
        </button>
      }
    >
      <div className="mb-2 flex gap-1" role="tablist">
        {CHANNELS.map((c) => (
          <button key={c.id} role="tab" aria-selected={ch === c.id} className="chip" aria-pressed={ch === c.id} onClick={() => setCh(c.id)}>
            <span className="h-2 w-2 rounded-full" style={{ background: c.color }} />
            {c.label}
          </button>
        ))}
      </div>
      <CurveEditor
        points={points}
        color={meta.color}
        onChange={(p) => setLive(updateGlobal(useEdit.getState().stack, (a) => setCurve(a, ch, p)))}
        onCommit={() => commitLive(qc, t('history.curves'), `curve:${ch}`)}
      />
      <p className="mt-1.5 text-[11px] text-faint">{t('edit.curvesHint')}</p>
    </Section>
  )
}
