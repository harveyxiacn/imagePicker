import { useEffect, useRef, useState } from 'react'
import { renderPreview } from '@/api/client'
import type { EditStack, PreviewBody, PreviewResult } from '@/api/types'
import { PreviewScheduler, previewLongEdge } from '@/lib/previewScheduler'

export interface FrameInfo {
  photoId: number
  url: string
  w: number
  h: number
  renderMs: number | null
  backend: PreviewResult['backend']
  longEdge: number
  /** false for small drag frames */
  final: boolean
}

interface Loaded extends PreviewResult {
  url: string
  w: number
  h: number
}

export type FrameBody = { stack?: EditStack; original?: boolean }

interface Args {
  photoId: number | null
  /** what to render; null = nothing */
  body: FrameBody | null
  /** identity of `body` (stack hash + mode) */
  bodyKey: string
  dragging: boolean
  container: { w: number; h: number }
  enabled?: boolean
}

/**
 * Progressive preview frames for the edit page (see lib/previewScheduler): small frames while dragging,
 * a screen-resolution frame on release. The previous frame stays up until the next one is decoded.
 */
export function usePreviewFrame({ photoId, body, bodyKey, dragging, container, enabled = true }: Args): { frame: FrameInfo | null; error: string | null } {
  const [state, setState] = useState<{ frame: FrameInfo | null; error: string | null }>({ frame: null, error: null })
  const sched = useRef<PreviewScheduler<Loaded> | null>(null)
  const urlRef = useRef<string | null>(null)
  const latest = useRef({ body })
  useEffect(() => {
    latest.current = { body }
  })

  useEffect(() => {
    if (photoId === null) return
    const s = new PreviewScheduler<Loaded>({
      fetcher: async (req, signal) => {
        const r = await renderPreview(req.payload as PreviewBody, signal)
        const url = URL.createObjectURL(r.blob)
        try {
          const im = new Image()
          im.src = url
          await im.decode()
          if (signal.aborted) throw new DOMException('aborted', 'AbortError')
          return { ...r, url, w: im.naturalWidth, h: im.naturalHeight }
        } catch (e) {
          URL.revokeObjectURL(url)
          throw e
        }
      },
      onFrame: ({ request, data }) => {
        const old = urlRef.current
        urlRef.current = data.url
        setState({
          error: null,
          frame: { photoId, url: data.url, w: data.w, h: data.h, renderMs: data.renderMs, backend: data.backend, longEdge: request.longEdge, final: request.final },
        })
        if (old) setTimeout(() => URL.revokeObjectURL(old), 1500)
      },
      onError: (err) => setState((st) => ({ ...st, error: err instanceof Error ? err.message : String(err) })),
    })
    sched.current = s
    return () => {
      s.dispose()
      sched.current = null
      if (urlRef.current) URL.revokeObjectURL(urlRef.current)
      urlRef.current = null
    }
  }, [photoId])

  useEffect(() => {
    const b = latest.current.body
    if (!enabled || photoId === null || !b) return
    const longEdge = previewLongEdge(!dragging, container, typeof window === 'undefined' ? 1 : window.devicePixelRatio || 1)
    sched.current?.request({
      key: `${photoId}|${bodyKey}`,
      longEdge,
      final: !dragging,
      payload: { photo_id: photoId, ...b, long_edge: longEdge } satisfies PreviewBody,
    })
  }, [photoId, bodyKey, dragging, container, enabled])

  return { frame: state.frame && state.frame.photoId === photoId ? state.frame : null, error: state.error }
}
