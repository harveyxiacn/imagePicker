/** Which photos have prepared portrait geometry (`beauty.prepare` done). Shared by the mock renderer and the M4 handlers. */
const ready = new Set<number>()

export const isBeautyReady = (photoId: number): boolean => ready.has(photoId)
export const markBeautyReady = (photoId: number): void => {
  ready.add(photoId)
}
