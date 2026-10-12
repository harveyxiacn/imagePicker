/**
 * Which image the loupe draws, and where, so that 100% is the photo's own pixels.
 *
 * "100%" means one image pixel per screen (device) pixel, like 1:1 in other photo tools: the zoom
 * maths in `zoom.ts` work in CSS pixels, so the image is handed to them at its CSS size at 100%
 * (`cssImageSize`), and its rectangle is snapped to the page's device pixels (`snapRect`) so the
 * browser draws the full-resolution image without resampling.
 *
 * Sources (docs/05 §API): previews of 2048 / 4096 px long edge (`GET /api/preview/{id}?s=`), and
 * the full-resolution image (`GET /api/full/{id}`), fetched only once the view is zoomed in past
 * what the 4096 preview holds. At fit (and while flipping through photos) only the preloaded 2048
 * preview is used, so next / previous stays as fast as before. ZoomPane skips the 4096 preview when
 * the view jumps straight to 100% (Space, double-click, Z).
 */
import type { Rect, Size } from './zoom'

export type LoupeSource = 2048 | 4096 | 'full'

/** How far a preview may be enlarged before the next larger image is fetched (below 100%). */
export const UPSCALE_SLACK = 1.15

/** Zoom (relative to fit) up to which the view counts as "fit": the 2048 preview is enough. */
const FIT_EPSILON = 1.001

/**
 * The smallest image that shows the view sharply.
 * @param zoom view zoom relative to fit (`ViewState.zoom`)
 * @param deviceLong long edge of the image on screen, in device pixels
 * @param nativeLong long edge of the photo in pixels (orientation applied); null = unknown
 */
export function loupeSource(zoom: number, deviceLong: number, nativeLong: number | null): LoupeSource {
  if (zoom <= FIT_EPSILON) return 2048
  const native = nativeLong && nativeLong > 0 ? nativeLong : Infinity
  // At 100% and above the photo's own pixels are needed: a preview only qualifies when it holds
  // all of them. Below that, a slightly enlarged preview is fine.
  const fits = (px: number) => px >= native || (deviceLong < native && deviceLong <= px * UPSCALE_SLACK)
  if (fits(Math.min(2048, native))) return 2048
  if (fits(Math.min(4096, native))) return 4096
  return 'full'
}

/** Image size in CSS pixels when shown at 100% (one image pixel per device pixel). */
export function cssImageSize(img: Size, dpr: number): Size {
  const r = dpr > 0 && Number.isFinite(dpr) ? dpr : 1
  return { w: img.w / r, h: img.h / r }
}

/**
 * Rounds a rectangle (CSS px, relative to its container) to whole device pixels of the page, so a
 * 100% image is not resampled. `origin` = the container's position on the page (CSS px), which
 * itself may sit between device pixels.
 */
export function snapRect(r: Rect, dpr: number, origin: { x: number; y: number } = { x: 0, y: 0 }): Rect {
  const k = dpr > 0 && Number.isFinite(dpr) ? dpr : 1
  const at = (v: number, o: number) => (Math.round((o + v) * k) - o * k) / k
  const len = (v: number) => Math.round(v * k) / k
  return { left: at(r.left, origin.x), top: at(r.top, origin.y), width: len(r.width), height: len(r.height) }
}
