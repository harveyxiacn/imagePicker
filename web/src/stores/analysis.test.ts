import { beforeEach, describe, expect, it } from 'vitest'
import { useAnalysisUi } from './analysis'

describe('analysis ui store: pausing', () => {
  beforeEach(() => useAnalysisUi.setState({ pausing: {} }))

  it('tracks sessions with a pause that has not taken effect yet', () => {
    const { setPausing } = useAnalysisUi.getState()
    setPausing(1, true)
    setPausing(2, true)
    expect(useAnalysisUi.getState().pausing).toEqual({ 1: true, 2: true })
    setPausing(1, false)
    expect(useAnalysisUi.getState().pausing).toEqual({ 2: true })
  })

  it('does not change the state when nothing changes', () => {
    const { setPausing } = useAnalysisUi.getState()
    const before = useAnalysisUi.getState().pausing
    setPausing(3, false)
    expect(useAnalysisUi.getState().pausing).toBe(before)
    setPausing(3, true)
    const after = useAnalysisUi.getState().pausing
    setPausing(3, true)
    expect(useAnalysisUi.getState().pausing).toBe(after)
  })
})
