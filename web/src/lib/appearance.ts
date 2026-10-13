/** Appearance preferences (theme, high contrast, UI font scale) and how they reach the document root. */

export type Theme = 'dark' | 'light' | 'system'
export type Contrast = 'normal' | 'high'

/** UI font scale, in percent of the default size. */
export const FONT_SCALE_MIN = 90
export const FONT_SCALE_MAX = 150
export const FONT_SCALE_STEP = 5
export const FONT_SCALE_DEFAULT = 100

/** Clamp to [90, 150] and snap to the step; anything that is not a finite number falls back to 100. */
export function clampFontScale(v: unknown): number {
  const n = typeof v === 'number' ? v : Number(v)
  if (!Number.isFinite(n)) return FONT_SCALE_DEFAULT
  const snapped = Math.round(n / FONT_SCALE_STEP) * FONT_SCALE_STEP
  return Math.min(FONT_SCALE_MAX, Math.max(FONT_SCALE_MIN, snapped))
}

/** `system` follows the OS colour scheme. */
export function resolveTheme(theme: Theme, prefersLight: boolean): 'dark' | 'light' {
  return theme === 'light' || (theme === 'system' && prefersLight) ? 'light' : 'dark'
}

export interface AppearanceInput {
  theme: Theme
  contrast: Contrast
  fontScale: number
}

/** The slice of `document.documentElement` that `applyAppearance` writes to (kept narrow so it is testable without a DOM). */
export interface RootLike {
  dataset: Record<string, string | undefined>
  style: { setProperty: (name: string, value: string) => void }
}

/** Sets `data-theme`, `data-contrast` and the `--font-scale` CSS variable (a unitless factor, 1 = 100%). */
export function applyAppearance(root: RootLike, input: AppearanceInput, prefersLight: boolean): void {
  root.dataset.theme = resolveTheme(input.theme, prefersLight)
  root.dataset.contrast = input.contrast === 'high' ? 'high' : 'normal'
  root.style.setProperty('--font-scale', String(clampFontScale(input.fontScale) / 100))
}
