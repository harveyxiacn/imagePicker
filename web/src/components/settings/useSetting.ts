import { usePatchSettings, useSettings } from '@/api/queries'
import type { Settings } from '@/api/types'
import { patchAt, type SettingPath } from '@/lib/settingsForm'

/** Settings + a setter that PATCHes one control (path -> minimal body) with optimistic UI. */
export function useSetting(): { s: Settings | undefined; set: (path: SettingPath, value: unknown) => void; isError: boolean; error: unknown } {
  const q = useSettings()
  const patch = usePatchSettings()
  return { s: q.data, set: (path, value) => patch.mutate(patchAt(path, value)), isError: q.isError, error: q.error }
}
