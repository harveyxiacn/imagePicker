import { useEffect, useRef, useState, type RefObject } from 'react'
import { isTypingTarget } from './keymap'

/** Observe an element's content box size. */
export function useElementSize<T extends HTMLElement>(): [RefObject<T | null>, { w: number; h: number }] {
  const ref = useRef<T | null>(null)
  const [size, setSize] = useState({ w: 0, h: 0 })
  useEffect(() => {
    const el = ref.current
    if (!el) return
    const ro = new ResizeObserver(([entry]) => {
      const r = entry.contentRect
      setSize((s) => (s.w === r.width && s.h === r.height ? s : { w: r.width, h: r.height }))
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  return [ref, size]
}

/** True while `key` is held (hold-to-zoom). Ignores typing targets, resets on blur. */
export function useKeyHeld(key: string, enabled = true): boolean {
  const [held, setHeld] = useState(false)
  useEffect(() => {
    if (!enabled) return
    const down = (e: KeyboardEvent) => {
      if (e.key.toLowerCase() !== key || e.ctrlKey || e.metaKey || e.altKey || isTypingTarget(e.target)) return
      setHeld(true)
    }
    const up = (e: KeyboardEvent) => {
      if (e.key.toLowerCase() === key) setHeld(false)
    }
    const reset = () => setHeld(false)
    window.addEventListener('keydown', down)
    window.addEventListener('keyup', up)
    window.addEventListener('blur', reset)
    return () => {
      window.removeEventListener('keydown', down)
      window.removeEventListener('keyup', up)
      window.removeEventListener('blur', reset)
      setHeld(false)
    }
  }, [key, enabled])
  return enabled && held
}

export function useDebouncedEffect(fn: () => void, deps: unknown[], ms: number) {
  const fnRef = useRef(fn)
  useEffect(() => {
    fnRef.current = fn
  })
  useEffect(() => {
    const id = setTimeout(() => fnRef.current(), ms)
    return () => clearTimeout(id)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, ms])
}
