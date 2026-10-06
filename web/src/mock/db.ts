import type { ColorLabel, Flag, ImageFormat, Photo, Session } from '@/api/types'

// ---------- deterministic RNG ----------
export function mulberry32(seed: number) {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = a
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

export function hashString(s: string): number {
  let h = 2166136261
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619)
  return h >>> 0
}

// ---------- store ----------
export interface MockSession {
  session: Session
  photos: Photo[]
}

const sessions = new Map<number, MockSession>()
let nextPhotoId = 1
let nextSessionId = 1

const DIMS: [number, number][] = [
  [6000, 4000],
  [6000, 4000],
  [6000, 4000],
  [4000, 6000],
  [4000, 3000],
  [3000, 4000],
  [6000, 3376],
  [5504, 3672],
]
const CAMERAS = ['SONY ILCE-7M4', 'Canon EOS R6', 'NIKON Z 6II', 'FUJIFILM X-T5', 'Apple iPhone 15 Pro']
const LENSES = ['FE 24-70mm F2.8 GM', 'RF 50mm F1.8', 'NIKKOR Z 85mm f/1.8', 'XF 35mm F1.4', null]
const FORMATS: { f: ImageFormat; ext: string; w: number }[] = [
  { f: 'jpeg', ext: 'JPG', w: 62 },
  { f: 'raw', ext: 'ARW', w: 24 },
  { f: 'heif', ext: 'HEIC', w: 8 },
  { f: 'png', ext: 'PNG', w: 3 },
  { f: 'tiff', ext: 'TIF', w: 3 },
]
const COLORS: Exclude<ColorLabel, null>[] = ['red', 'yellow', 'green', 'blue', 'purple']
const SHUTTERS = [1 / 4000, 1 / 2000, 1 / 1000, 1 / 500, 1 / 250, 1 / 125, 1 / 60, 1 / 30]
const APERTURES = [1.4, 1.8, 2.8, 4, 5.6, 8, 11]
const ISOS = [100, 200, 400, 800, 1600, 3200, 6400]

export function computeCounts(s: MockSession): void {
  let picked = 0
  let rejected = 0
  let rated = 0
  for (const p of s.photos) {
    if (p.flag === 1) picked++
    else if (p.flag === -1) rejected++
    if (p.user_rating !== null) rated++
  }
  s.session.photo_count = s.photos.length
  s.session.picked_count = picked
  s.session.rejected_count = rejected
  s.session.rated_count = rated
  s.session.cover_photo_id = s.photos[Math.floor(s.photos.length / 3)]?.id ?? null
}

export function generatePhotos(
  sessionId: number,
  rootPath: string,
  count: number,
  seed: number,
  startMs: number,
  thumbReadyRatio = 0.88,
): Photo[] {
  const rnd = mulberry32(seed)
  const photos: Photo[] = []
  let t = startMs
  let burstLeft = 0
  const sep = rootPath.includes('\\') ? '\\' : '/'
  const pick = <T,>(arr: T[]) => arr[Math.floor(rnd() * arr.length)]
  for (let i = 0; i < count; i++) {
    // bursts: runs of near-identical frames a few hundred ms apart
    if (burstLeft > 0) {
      burstLeft--
      t += 200 + rnd() * 500
    } else {
      t += 4_000 + rnd() * 120_000
      if (rnd() < 0.18) burstLeft = 2 + Math.floor(rnd() * 6)
    }
    let r = rnd() * 100
    let fmt = FORMATS[0]
    for (const f of FORMATS) {
      if ((r -= f.w) < 0) {
        fmt = f
        break
      }
    }
    const [w, h] = pick(DIMS)
    const id = nextPhotoId++
    const name = `IMG_${String(i + 1).padStart(4, '0')}.${fmt.ext}`
    const rated = rnd() < 0.3
    const fr = rnd()
    const flag: Flag = fr < 0.08 ? 1 : fr < 0.14 ? -1 : 0
    const hasExif = fmt.f !== 'png'
    photos.push({
      id,
      session_id: sessionId,
      path: `${rootPath}${sep}${name}`,
      file_name: name,
      format: fmt.f,
      file_size: Math.round((fmt.f === 'raw' ? 28e6 : fmt.f === 'png' ? 18e6 : 7e6) * (0.6 + rnd() * 0.9)),
      width: w,
      height: h,
      taken_at: Math.round(t),
      camera: hasExif ? CAMERAS[Math.floor(i / 400) % CAMERAS.length] : null,
      lens: hasExif ? pick(LENSES) : null,
      focal_mm: hasExif ? pick([24, 35, 50, 70, 85, 135]) : null,
      aperture: hasExif ? pick(APERTURES) : null,
      shutter_s: hasExif ? pick(SHUTTERS) : null,
      iso: hasExif ? pick(ISOS) : null,
      user_rating: rated ? 1 + Math.floor(rnd() * 5) : null,
      ai_rating: null,
      flag,
      color_label: rnd() < 0.07 ? pick(COLORS) : null,
      burst_id: null,
      thumb_ready: rnd() < thumbReadyRatio,
      thumb_version: (hashString(String(id)) & 0xffffff).toString(16),
    })
  }
  return photos
}

export function addSession(
  id: number,
  title: string,
  rootPath: string,
  photos: Photo[],
  importState: Session['import_state'] = 'ready',
  createdAt = Date.now(),
): MockSession {
  const s: MockSession = {
    session: {
      id,
      title,
      root_path: rootPath,
      created_at: createdAt,
      photo_count: 0,
      picked_count: 0,
      rejected_count: 0,
      rated_count: 0,
      cover_photo_id: null,
      import_state: importState,
    },
    photos,
  }
  computeCounts(s)
  sessions.set(id, s)
  return s
}

export function createSessionId(): number {
  return nextSessionId++
}

export function getSession(id: number): MockSession | undefined {
  return sessions.get(id)
}
export function allSessions(): MockSession[] {
  return [...sessions.values()].sort((a, b) => b.session.created_at - a.session.created_at)
}
export function deleteSession(id: number): boolean {
  return sessions.delete(id)
}
export function findPhoto(id: number): Photo | undefined {
  for (const s of sessions.values()) {
    // photos of a session are generated in id order, so binary search works
    const arr = s.photos
    if (!arr.length || id < arr[0].id || id > arr[arr.length - 1].id) continue
    let lo = 0
    let hi = arr.length - 1
    while (lo <= hi) {
      const mid = (lo + hi) >> 1
      if (arr[mid].id === id) return arr[mid]
      if (arr[mid].id < id) lo = mid + 1
      else hi = mid - 1
    }
  }
  return undefined
}

export function newThumbVersion(id: number): string {
  return ((hashString(`${id}:${Date.now()}`) & 0xffffff) | 0x1000000).toString(16).slice(1)
}

export function seedMockDb(): void {
  if (sessions.size) return
  const day = 86_400_000
  const now = Date.now()
  const mk = (title: string, root: string, n: number, seed: number, ageDays: number, ready = 0.88) => {
    const sid = createSessionId()
    const photos = generatePhotos(sid, root, n, seed, now - (ageDays + 10) * day, ready)
    return addSession(sid, title, root, photos, 'ready', now - ageDays * day)
  }
  mk('京都 2026', 'C:\\Users\\demo\\Pictures\\Kyoto2026', 3000, 11, 1)
  mk('小明生日', 'C:\\Users\\demo\\Pictures\\Birthday', 356, 22, 6, 1)
  mk('国庆聚会', 'C:\\Users\\demo\\Pictures\\Holiday', 812, 33, 12)
  mk('压力测试 · 20,000 张', 'D:\\Archive\\Stress', 20000, 44, 30, 0.97)
}
