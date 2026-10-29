import { ChevronLeft, HeartHandshake } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { Link, useNavigate } from 'react-router-dom'
import { HeaderControls } from '@/components/HeaderControls'
import { TasteSection } from '@/components/settings/TasteSection'

/** "My style" (F2.10, docs/03 §3.3): what the personal style model learned from your picks. */
export function Taste() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  return (
    <div className="flex h-full flex-col" data-testid="taste-page">
      <header className="flex h-12 shrink-0 items-center gap-3 border-b border-line px-3">
        <button className="btn btn-ghost" onClick={() => (window.history.length > 1 ? navigate(-1) : navigate('/'))} aria-label={t('common.back')}>
          <ChevronLeft size={16} />
          <span className="hidden sm:inline">{t('common.back')}</span>
        </button>
        <h1 className="flex items-center gap-2 text-base font-semibold">
          <HeartHandshake size={16} />
          {t('taste.title')}
        </h1>
        <div className="ml-auto flex items-center gap-2">
          <HeaderControls />
        </div>
      </header>
      <main className="min-h-0 flex-1 overflow-y-auto p-4">
        <div className="mx-auto flex max-w-3xl flex-col gap-4">
          <TasteSection level={2} />
          <Link to="/" className="btn btn-ghost self-start">
            {t('taste.backHome')}
          </Link>
        </div>
      </main>
    </div>
  )
}
