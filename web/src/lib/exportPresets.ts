/** Export presets (also used by the AI assistant's `export` tool): long edge (null = original) + JPEG quality. */
export const EXPORT_PRESETS = {
  original: { edge: null, quality: 95 },
  wechat: { edge: 1920, quality: 82 },
  xiaohongshu: { edge: 1440, quality: 88 },
  instagram: { edge: 1080, quality: 90 },
} as const
export type ExportPresetId = keyof typeof EXPORT_PRESETS
