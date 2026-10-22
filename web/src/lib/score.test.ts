import { describe, expect, it } from 'vitest'
import { aiScore100, formatAiScore, formatAiScoreOf100 } from './score'

describe('ai score display', () => {
  it('maps 0-1 to a rounded 0-100 integer', () => {
    expect(aiScore100(0.824)).toBe(82)
    expect(aiScore100(0.825)).toBe(83)
    expect(aiScore100(0)).toBe(0)
    expect(aiScore100(1)).toBe(100)
  })
  it('clamps out-of-range values', () => {
    expect(aiScore100(-0.2)).toBe(0)
    expect(aiScore100(1.4)).toBe(100)
  })
  it('returns null / dash for missing scores', () => {
    expect(aiScore100(null)).toBeNull()
    expect(aiScore100(undefined)).toBeNull()
    expect(aiScore100(Number.NaN)).toBeNull()
    expect(formatAiScore(null)).toBe('–')
    expect(formatAiScoreOf100(null)).toBe('–')
  })
  it('formats', () => {
    expect(formatAiScore(0.68)).toBe('68')
    expect(formatAiScoreOf100(0.68)).toBe('68/100')
  })
})
