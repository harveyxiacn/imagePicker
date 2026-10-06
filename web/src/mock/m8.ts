/**
 * Mock backend for M8 (docs/api-contract-m8.md): fake platform album list + media permission, remote AI pairing
 * (host side: pair/start, devices; phone side: connect / status / disconnect). One mock server plays both roles.
 *
 * URL switches: `?mock_media=denied|partial|granted|none` (permission state of the fake media plugin; `none` hides it).
 * Pairing: the code shown by "pair/start" is accepted by `connect`; `123456` always works; any url containing
 * "unreachable" fails with 502 host_unreachable; an expired code gives 410 pair_expired.
 */
import { delay, http, HttpResponse } from 'msw'
import type { RemoteDevice, RemoteStatus } from '@/api/types'
import { fakeQr } from './m6'

const err = (status: number, code: string, message: string) => HttpResponse.json({ error: { code, message } }, { status })
const lat = () => delay(20 + Math.random() * 40)
const nowS = () => Math.floor(Date.now() / 1000)

// ---------------------------------------------------------------- albums (platform plugin `list_albums`)

function cover(hue: number, label: string): string {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 160 160"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="hsl(${hue} 55% 42%)"/><stop offset="1" stop-color="hsl(${(hue + 50) % 360} 60% 24%)"/></linearGradient></defs><rect width="160" height="160" fill="url(#g)"/><circle cx="116" cy="46" r="16" fill="hsl(${hue} 90% 80%)" opacity=".85"/><path d="M0 160V112l40-30 36 26 30-20 54 36v36z" fill="hsl(${hue} 40% 14%)" opacity=".7"/><text x="10" y="150" font-size="14" fill="#fff" opacity=".7" font-family="sans-serif">${label}</text></svg>`
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`
}

const DAY = 86_400_000
const ALBUMS = [
  { id: 'camera', name: 'Camera', path: '/storage/emulated/0/DCIM/Camera', count: 1284, hue: 28, ago: 0 },
  { id: 'kyoto', name: 'Kyoto2026', path: '/storage/emulated/0/DCIM/Kyoto2026', count: 642, hue: 350, ago: 6 },
  { id: 'wechat', name: 'WeiXin', path: '/storage/emulated/0/Pictures/WeiXin', count: 318, hue: 140, ago: 2 },
  { id: 'screenshots', name: 'Screenshots', path: '/storage/emulated/0/Pictures/Screenshots', count: 207, hue: 210, ago: 1 },
  { id: 'download', name: 'Download', path: '/storage/emulated/0/Download', count: 58, hue: 265, ago: 11 },
  { id: 'birthday', name: 'Birthday', path: '/storage/emulated/0/DCIM/Birthday', count: 134, hue: 48, ago: 30 },
].map((a) => ({
  id: a.id,
  name: a.name,
  path: a.path,
  count: a.count,
  cover_path: `${a.path}/cover.jpg`,
  cover_url: cover(a.hue, a.name),
  latest_ms: Date.now() - a.ago * DAY,
}))

// ---------------------------------------------------------------- remote AI

interface HostState {
  pair: { code: string; expiresAt: number } | null
  devices: RemoteDevice[]
}
const host: HostState = {
  pair: null,
  devices: [
    { device_id: 'dev-ipad', name: 'iPad Pro (客厅)', created_at: nowS() - 9 * 86400, last_seen: nowS() - 3600 },
    { device_id: 'dev-pixel-old', name: 'Pixel 6', created_at: nowS() - 40 * 86400, last_seen: nowS() - 20 * 86400 },
  ],
}

const phone: RemoteStatus = { paired: false, connected: false, url: null, host_name: null, host_tier: null, last_error: null, last_seen: null }

const randomCode = () => String(100000 + Math.floor(Math.random() * 900000))

export const m8Handlers = [
  http.get('/api/mock/media/permission', async ({ request }) => {
    await lat()
    const state = new URL(request.url).searchParams.get('state') ?? 'granted'
    return HttpResponse.json({ granted: state !== 'denied', partial: state === 'partial' })
  }),
  http.get('/api/mock/media/albums', async ({ request }) => {
    await lat()
    const state = new URL(request.url).searchParams.get('state') ?? 'granted'
    if (state === 'denied') return err(403, 'permission_denied', 'READ_MEDIA_IMAGES not granted')
    // Android 14 partial access only exposes the photos the user picked
    return HttpResponse.json({ albums: state === 'partial' ? ALBUMS.slice(0, 2).map((a) => ({ ...a, count: Math.round(a.count / 20) })) : ALBUMS })
  }),

  // ---- host side
  http.post('/api/remote/pair/start', async () => {
    await lat()
    host.pair = { code: randomCode(), expiresAt: Date.now() + 5 * 60_000 }
    const url = 'http://192.168.1.23:7878'
    return HttpResponse.json({
      code: host.pair.code,
      expires_at: Math.floor(host.pair.expiresAt / 1000),
      url,
      qr_svg: fakeQr(`imagepicker://pair?url=${url}&code=${host.pair.code}`),
    })
  }),
  http.get('/api/remote/devices', async () => {
    await lat()
    return HttpResponse.json({ devices: host.devices })
  }),
  http.delete('/api/remote/devices/:id', async ({ params }) => {
    await lat()
    const n = host.devices.length
    host.devices = host.devices.filter((d) => d.device_id !== params.id)
    return host.devices.length === n ? err(404, 'not_found', 'device not found') : new HttpResponse(null, { status: 204 })
  }),

  // ---- phone side
  http.get('/api/remote/status', async () => {
    await lat()
    return HttpResponse.json(phone)
  }),
  http.post('/api/remote/connect', async ({ request }) => {
    await delay(500)
    const body = (await request.json()) as { url?: string; code?: string; device_name?: string }
    if (!body.url || !body.code) return err(400, 'bad_request', 'url and code required')
    if (body.url.includes('unreachable')) {
      phone.last_error = 'host_unreachable'
      return err(502, 'host_unreachable', 'cannot reach the host')
    }
    const valid = host.pair && host.pair.code === body.code
    if (host.pair && host.pair.code === body.code && Date.now() > host.pair.expiresAt) return err(410, 'pair_expired', 'pairing code expired')
    if (!valid && body.code !== '123456') return err(400, 'invalid_code', 'wrong pairing code')
    host.pair = null
    host.devices = [{ device_id: `dev-${Date.now()}`, name: body.device_name || 'Pixel 7', created_at: nowS(), last_seen: nowS() }, ...host.devices]
    Object.assign(phone, { paired: true, connected: true, url: body.url, host_name: 'Studio-PC', host_tier: 'T2', last_error: null, last_seen: nowS() })
    return HttpResponse.json(phone)
  }),
  http.delete('/api/remote/connect', async () => {
    await lat()
    Object.assign(phone, { paired: false, connected: false, url: null, host_name: null, host_tier: null, last_error: null, last_seen: null })
    return new HttpResponse(null, { status: 204 })
  }),
]

/** Test hook: `?mock_remote=paired` starts with a connected phone, `?mock_remote=offline` with an unreachable host. */
export function seedM8Mock(): void {
  const mode = new URLSearchParams(window.location.search).get('mock_remote')
  if (mode === 'paired') Object.assign(phone, { paired: true, connected: true, url: 'http://192.168.1.23:7878', host_name: 'Studio-PC', host_tier: 'T2', last_seen: nowS() })
  if (mode === 'offline')
    Object.assign(phone, { paired: true, connected: false, url: 'http://192.168.1.23:7878', host_name: 'Studio-PC', host_tier: 'T2', last_error: 'host_unreachable', last_seen: nowS() - 7200 })
}
