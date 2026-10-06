import { Lightbulb } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { faceCropUrl } from '@/api/client'
import { useBurstFaces } from '@/api/queries'
import type { Face, Photo } from '@/api/types'
import { commonBestFrames, expressionTone, type CellTone } from '@/lib/ai'
import { useFaceMenu } from './FaceMenu'

const TONE_COLOR: Record<CellTone, string> = {
  good: 'var(--success)',
  ok: 'var(--warning)',
  bad: 'var(--danger)',
  none: 'transparent',
}

interface Props {
  burstId: number
  /** Shot order (matches the filmstrip numbering). */
  photos: Photo[]
  aId: number | null
  bId: number | null
  /** Click a cell: select that frame as B (Shift = pin as A). */
  onPick: (photoId: number, e?: React.MouseEvent) => void
  /** Open the Best Take editor for this group (doc 04 3.3 "✨ 生成全员最佳表情合成"). */
  onCompose?: () => void
}

/** Person x frame expression matrix (doc 04 3.3): green = eyes open + smile, yellow = so-so, red = closed/blurry. */
export function ExpressionMatrix({ burstId, photos, aId, bId, onPick, onCompose }: Props) {
  const { t } = useTranslation()
  const q = useBurstFaces(burstId)
  const menu = useFaceMenu()
  const data = q.data
  const num = new Map(photos.map((p, i) => [p.id, i + 1]))

  if (q.isPending) return <div className="p-3 text-muted">{t('library.loading')}</div>
  if (q.isError) return <div className="p-3 text-danger">{(q.error as Error).message}</div>
  if (!data || data.tracks.length === 0) return <div className="p-3 text-muted" data-testid="matrix-empty">{t('matrix.noFaces')}</div>

  const common = commonBestFrames(data.tracks)
  const bestForAll = common.length > 0
  const colOutline = (id: number) => (id === aId ? 'var(--accent)' : id === bId ? 'var(--fg)' : 'transparent')

  return (
    <div className="flex flex-col gap-2 p-2" data-testid="expression-matrix">
      <div className="overflow-x-auto">
        <table className="border-separate border-spacing-1 text-xs">
          <thead>
            <tr>
              <th className="min-w-24 text-left font-normal text-muted">{t('matrix.person')}</th>
              {photos.map((p) => (
                <th
                  key={p.id}
                  className="tnum min-w-10 cursor-pointer rounded font-normal text-muted hover:text-fg"
                  style={{ outline: `2px solid ${colOutline(p.id)}`, outlineOffset: -1 }}
                  onClick={(e) => onPick(p.id, e)}
                  title={p.file_name}
                >
                  #{num.get(p.id)}
                  {p.id === aId ? ' A' : p.id === bId ? ' B' : ''}
                </th>
              ))}
              <th className="pl-2 text-left font-normal text-muted">{t('matrix.best')}</th>
            </tr>
          </thead>
          <tbody>
            {data.tracks.map((tr) => {
              const firstFace = Object.values(tr.cells).find((c): c is Face => !!c)
              return (
                <tr key={tr.track_id} data-testid="matrix-row">
                  <td className="pr-2">
                    <div className="flex items-center gap-2">
                      {firstFace && <img src={faceCropUrl(firstFace.id, 128)} alt="" className="h-6 w-6 rounded-full object-cover" draggable={false} />}
                      <span className="max-w-20 truncate text-sm">{tr.person_name ?? (tr.person_id !== null ? `${t('person.unnamed')} ${tr.person_id}` : t('face.unassigned'))}</span>
                    </div>
                  </td>
                  {photos.map((p) => {
                    const face = tr.cells[String(p.id)] ?? null
                    const tone = expressionTone(face)
                    const isBest = tr.best_photo_ids.includes(p.id)
                    return (
                      <td key={p.id} className="p-0 text-center">
                        {face ? (
                          <button
                            type="button"
                            className="relative block h-9 w-9 overflow-hidden rounded-control bg-bg"
                            style={{ boxShadow: `inset 0 0 0 2px ${TONE_COLOR[tone]}`, outline: `2px solid ${colOutline(p.id)}`, outlineOffset: 1 }}
                            data-tone={tone}
                            data-testid="matrix-cell"
                            title={`#${num.get(p.id)} · ${t('face.eyes')} ${face.eyes_open?.toFixed(2) ?? '–'} · ${t('face.smile')} ${face.smile?.toFixed(2) ?? '–'}`}
                            onClick={(e) => onPick(p.id, e)}
                            onContextMenu={(e) => menu.open(e, face)}
                          >
                            <img src={faceCropUrl(face.id, 128)} alt="" className="h-full w-full object-cover" draggable={false} />
                            <span className="absolute inset-0 rounded-control" style={{ boxShadow: `inset 0 0 0 2px ${TONE_COLOR[tone]}` }} />
                            {isBest && <span className="absolute right-0 bottom-0 rounded-tl bg-black/70 px-0.5 text-[9px] leading-3 text-ai">★</span>}
                          </button>
                        ) : (
                          <span className="block h-9 w-9 rounded-control border border-dashed border-line text-faint" style={{ lineHeight: '34px' }}>
                            –
                          </span>
                        )}
                      </td>
                    )
                  })}
                  <td className="pl-2 whitespace-nowrap">
                    {tr.best_photo_ids.map((id) => (
                      <button key={id} className="tnum mr-1 rounded bg-panel px-1.5 py-0.5 text-ai hover:bg-elevated" onClick={(e) => onPick(id, e)} data-testid="matrix-best">
                        #{num.get(id) ?? '?'}
                      </button>
                    ))}
                  </td>
                </tr>
              )
            })}
          </tbody>
        </table>
      </div>
      <div className="flex flex-wrap items-center gap-3 text-xs" data-testid="matrix-hint">
        <span className="flex items-center gap-1.5 text-muted">
          <Lightbulb size={13} className="text-accent" />
          {bestForAll ? t('matrix.commonBest', { frames: common.map((id) => `#${num.get(id)}`).join(' ') }) : t('matrix.noCommonBest')}
        </span>
        <span className="flex items-center gap-2 text-faint">
          <Legend color="var(--success)" label={t('matrix.good')} />
          <Legend color="var(--warning)" label={t('matrix.ok')} />
          <Legend color="var(--danger)" label={t('matrix.bad')} />
        </span>
        <button
          className={`btn !h-6 text-xs ${bestForAll ? 'text-ai' : 'btn-primary'}`}
          disabled={!onCompose || photos.length < 2}
          onClick={onCompose}
          title={t('matrix.composeHint')}
          data-testid="matrix-compose"
        >
          ✨ {t('matrix.compose')}
        </button>
      </div>
      {menu.node}
    </div>
  )
}

function Legend({ color, label }: { color: string; label: string }) {
  return (
    <span className="flex items-center gap-1">
      <span className="inline-block h-2.5 w-2.5 rounded-sm" style={{ background: color }} />
      {label}
    </span>
  )
}
