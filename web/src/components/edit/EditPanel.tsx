import type { Photo } from '@/api/types'
import { AiPanel, BasicPanel, SharpenPanel } from './BasicPanels'
import { GradingPanel, HslPanel } from './ColorPanels'
import { CropPanel } from './CropPanel'
import { CurvesPanel } from './CurvesPanel'
import { LocalPanel } from './LocalPanel'
import { PortraitPanel } from './PortraitPanel'
import { PresetsPanel } from './PresetsPanel'

/** Right column of the edit page (doc 04 section 3.5). */
export function EditPanel({ photo }: { photo: Photo }) {
  return (
    <div className="flex flex-col" data-testid="edit-panel">
      <AiPanel />
      <BasicPanel />
      <CurvesPanel />
      <HslPanel />
      <GradingPanel />
      <LocalPanel photo={photo} />
      <PortraitPanel key={photo.id} photo={photo} />
      <CropPanel photo={photo} />
      <PresetsPanel photoId={photo.id} />
      <SharpenPanel />
    </div>
  )
}
