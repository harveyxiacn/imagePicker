import { useEffect, useState } from 'react'
import { MOBILE_BREAKPOINT } from './layout'

const query = `(max-width: ${MOBILE_BREAKPOINT - 1}px)`

function matches(): boolean {
  return typeof window !== 'undefined' && !!window.matchMedia && window.matchMedia(query).matches
}

/** True below the phone breakpoint (768 px). Re-evaluates on resize / rotation. */
export function useIsMobile(): boolean {
  const [m, setM] = useState(matches)
  useEffect(() => {
    if (!window.matchMedia) return
    const mq = window.matchMedia(query)
    const on = () => setM(mq.matches)
    on()
    mq.addEventListener('change', on)
    return () => mq.removeEventListener('change', on)
  }, [])
  return m
}

/** True when the primary pointer is a finger (phones, tablets): used to hide hover-only affordances. */
export function useIsTouch(): boolean {
  const [t] = useState(() => typeof window !== 'undefined' && !!window.matchMedia && window.matchMedia('(pointer: coarse)').matches)
  return t
}
