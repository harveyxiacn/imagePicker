/** In-memory store of generated patch assets (RGBA PNG blobs) of the mock backend; shared by the handlers and the mock renderer. */
interface Asset {
  photoId: number
  blob: Blob
  bitmap: Promise<ImageBitmap> | null
}
const assets = new Map<string, Asset>()

export function putAsset(photoId: number, id: string, blob: Blob): void {
  assets.set(id, { photoId, blob, bitmap: null })
}
export const assetBlob = (photoId: number, id: string): Blob | undefined => {
  const a = assets.get(id)
  return a && a.photoId === photoId ? a.blob : undefined
}
/** Decoded asset (cached); undefined when the id is unknown. */
export function assetBitmap(id: string): Promise<ImageBitmap> | undefined {
  const a = assets.get(id)
  if (!a) return undefined
  a.bitmap ??= createImageBitmap(a.blob)
  return a.bitmap
}
