import { usePhotoAnalysis } from '@/api/queries'
import { expressionTone } from '@/lib/ai'
import { useFaceMenu } from './FaceMenu'

const TONE: Record<string, string> = {
  good: 'var(--success)',
  ok: 'var(--warning)',
  bad: 'var(--danger)',
  none: 'var(--faint)',
}

/** Face boxes over the image (Shift+F). Right-click a box: filter by / exclude / name this person. */
export function FaceBoxes({ photoId }: { photoId: number }) {
  const analysis = usePhotoAnalysis(photoId)
  const menu = useFaceMenu()
  const faces = analysis.data?.photo_id === photoId ? analysis.data.faces : []
  return (
    <>
      {faces.map((f) => {
        const [x, y, w, h] = f.bbox
        const color = TONE[expressionTone(f)]
        return (
          <div
            key={f.id}
            className="pointer-events-auto absolute"
            style={{ left: `${x * 100}%`, top: `${y * 100}%`, width: `${w * 100}%`, height: `${h * 100}%`, border: `2px solid ${color}`, borderRadius: 4, opacity: f.is_subject ? 1 : 0.6 }}
            onContextMenu={(e) => menu.open(e, f)}
            data-testid="face-box"
          >
            {(f.person_name || !f.is_subject) && (
              <span className="absolute -top-5 left-0 max-w-full truncate rounded px-1 text-[11px] leading-4 text-black" style={{ background: color }}>
                {f.person_name ?? '·'}
              </span>
            )}
          </div>
        )
      })}
      {menu.node}
    </>
  )
}
