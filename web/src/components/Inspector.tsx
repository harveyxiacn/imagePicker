import { ChevronDown, ChevronRight, ImageOff } from 'lucide-react'
import { useState, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import { previewUrl, thumbUrl } from '@/api/client'
import type { ColorLabel, Flag, Photo } from '@/api/types'
import { formatBytes, formatDate, formatDims, formatShutter } from '@/lib/format'
import { AiRatingSlot, ColorDots, FlagButtons, StarRating } from './controls'

interface Props {
  photo: Photo | undefined
  /** Number of photos the controls will act on (selection size). */
  targetCount: number
  onRate: (n: number | null) => void
  onFlag: (f: Flag) => void
  onColor: (c: ColorLabel) => void
}

function Row({ k, v }: { k: string; v: ReactNode }) {
  return (
    <tr className="border-t border-line/60 first:border-t-0">
      <th className="w-[38%] py-1 pr-2 text-left align-top font-normal text-muted">{k}</th>
      <td className="tnum py-1 break-all">{v}</td>
    </tr>
  )
}

export function Inspector({ photo, targetCount, onRate, onFlag, onColor }: Props) {
  const { t, i18n } = useTranslation()
  const [exifOpen, setExifOpen] = useState(true)
  const [broken, setBroken] = useState<number | null>(null)

  if (!photo) {
    return <div className="p-4 text-muted">{t('inspector.none')}</div>
  }

  const exposure = [
    photo.aperture !== null ? `f/${photo.aperture}` : null,
    photo.shutter_s !== null ? formatShutter(photo.shutter_s) : null,
    photo.iso !== null ? `ISO ${photo.iso}` : null,
  ]
    .filter(Boolean)
    .join(' · ')

  return (
    <div className="flex flex-col gap-4 p-3">
      <div className="flex aspect-[4/3] items-center justify-center overflow-hidden rounded-card bg-bg">
        {broken === photo.id ? (
          <ImageOff className="text-faint" />
        ) : (
          <img
            key={photo.id}
            src={photo.thumb_ready ? thumbUrl(photo, 512) : previewUrl(photo.id, 1024)}
            alt={photo.file_name}
            className="max-h-full max-w-full object-contain"
            draggable={false}
            onError={() => setBroken(photo.id)}
          />
        )}
      </div>

      <div>
        <div className="truncate font-medium" title={photo.file_name}>
          {photo.file_name}
        </div>
        {targetCount > 1 && <div className="mt-0.5 text-xs text-accent">{t('inspector.applyingTo', { n: targetCount })}</div>}
      </div>

      <section className="flex flex-col gap-2.5">
        <div className="flex items-center justify-between gap-2">
          <span className="text-muted">{t('inspector.rating')}</span>
          <StarRating value={photo.user_rating} onChange={onRate} />
        </div>
        <div className="flex items-center justify-between gap-2">
          <span className="text-muted">{t('inspector.aiRating')}</span>
          <AiRatingSlot value={photo.ai_rating} />
        </div>
        <div className="flex items-center justify-between gap-2">
          <span className="text-muted">{t('inspector.flag')}</span>
          <FlagButtons value={photo.flag} onChange={onFlag} />
        </div>
        <div className="flex items-center justify-between gap-2">
          <span className="text-muted">{t('inspector.color')}</span>
          <ColorDots value={photo.color_label} onChange={onColor} />
        </div>
      </section>

      <section>
        <button
          className="mb-1 flex w-full items-center gap-1 text-left font-medium"
          onClick={() => setExifOpen((o) => !o)}
          aria-expanded={exifOpen}
        >
          {exifOpen ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
          {t('inspector.exif')}
        </button>
        {exifOpen && (
          <table className="w-full text-xs">
            <tbody>
              <Row k={t('exif.path')} v={photo.path} />
              <Row k={t('exif.format')} v={photo.format.toUpperCase()} />
              <Row k={t('exif.size')} v={formatBytes(photo.file_size)} />
              <Row k={t('exif.dimensions')} v={formatDims(photo.width, photo.height)} />
              <Row k={t('exif.taken')} v={formatDate(photo.taken_at, i18n.language, photo.taken_at_offset_min)} />
              <Row k={t('exif.camera')} v={photo.camera ?? '—'} />
              <Row k={t('exif.lens')} v={photo.lens ?? '—'} />
              <Row k={t('exif.focal')} v={photo.focal_mm !== null ? `${photo.focal_mm} mm` : '—'} />
              <Row k={t('exif.exposure')} v={exposure || '—'} />
            </tbody>
          </table>
        )}
      </section>
    </div>
  )
}
