import { describe, expect, it } from 'vitest'
import { tasteProgress } from './taste'

describe('tasteProgress', () => {
  it('counts down to the next 50-label retraining', () => {
    expect(tasteProgress(0)).toEqual({ toNext: 50, fraction: 0 })
    expect(tasteProgress(84)).toEqual({ toNext: 16, fraction: 34 / 50 })
    expect(tasteProgress(100)).toEqual({ toNext: 50, fraction: 0 })
    expect(tasteProgress(-3).toNext).toBe(50)
  })
})
