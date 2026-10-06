import { describe, expect, it, vi } from 'vitest'
import { createMediaApi, loadAlbums } from './android'

describe('android media api', () => {
  it('calls plugin commands with contract argument names', async () => {
    const invoke = vi.fn(async (cmd: string) => {
      if (cmd.endsWith('list_albums')) return { albums: [{ id: '1', name: 'Camera', path: '/p', count: 2, cover_path: '/p/a.jpg', latest_ms: 5 }] }
      if (cmd.endsWith('trash')) return { trashed: 2 }
      if (cmd.endsWith('share')) return { shared: 1 }
      return null
    })
    const api = createMediaApi(invoke)
    expect((await api.listAlbums())[0].name).toBe('Camera')
    expect(await api.trash(['/a', '/b'])).toBe(2)
    expect(await api.share(['/a'])).toBe(1)
    await api.keepAwake(true)
    await api.startForeground('t', 'x', 40)
    expect(invoke).toHaveBeenCalledWith('plugin:imagepicker-media|trash', { paths: ['/a', '/b'] })
    expect(invoke).toHaveBeenCalledWith('plugin:imagepicker-media|keep_awake', { on: true })
    expect(invoke).toHaveBeenCalledWith('plugin:imagepicker-media|start_foreground', { title: 't', text: 'x', progress: 40 })
  })

  it('loadAlbums reports denied / partial / ok', async () => {
    const mk = (granted: boolean, partial: boolean) =>
      createMediaApi(async (cmd) => (cmd.endsWith('request_permission') ? { granted, partial } : { albums: [] }))
    expect((await loadAlbums(mk(false, false))).status).toBe('denied')
    expect((await loadAlbums(mk(true, true))).status).toBe('partial')
    expect((await loadAlbums(mk(true, false))).status).toBe('ok')
  })
})
