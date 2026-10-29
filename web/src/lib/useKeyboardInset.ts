import { useEffect, useState } from 'react'
import { keyboardInset } from './popover'

/**
 * Pixels the on-screen keyboard covers at the bottom of the page (0 without one). Fixed bottom-anchored UI adds this
 * as its `bottom` offset so a focused input stays visible on browsers that only shrink the visual viewport (iOS Safari).
 * `index.html` also sets `interactive-widget=resizes-content` for those that resize the layout viewport instead.
 */
export function useKeyboardInset(active = true): number {
  const [inset, setInset] = useState(0)
  useEffect(() => {
    const vv = window.visualViewport
    if (!active || !vv) return
    const update = () => setInset(keyboardInset(window.innerHeight, vv))
    update()
    vv.addEventListener('resize', update)
    vv.addEventListener('scroll', update)
    return () => {
      vv.removeEventListener('resize', update)
      vv.removeEventListener('scroll', update)
      setInset(0)
    }
  }, [active])
  return inset
}
