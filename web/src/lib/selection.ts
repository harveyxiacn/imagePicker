export interface SelectionState {
  ids: ReadonlySet<number>
  /** Anchor photo for shift-range selection. */
  anchorId: number | null
}

export const EMPTY_SELECTION: SelectionState = { ids: new Set(), anchorId: null }

export interface ClickMods {
  shift?: boolean
  ctrl?: boolean
}

/**
 * Pure selection reducer for a click on `id`.
 * - plain click: select only `id`
 * - ctrl/cmd: toggle `id`, anchor moves to it
 * - shift: select range anchor..id (replacing, or extending when ctrl is also held)
 */
export function clickSelect(
  state: SelectionState,
  orderedIds: readonly number[],
  id: number,
  mods: ClickMods = {},
): SelectionState {
  if (mods.shift && state.anchorId !== null) {
    const a = orderedIds.indexOf(state.anchorId)
    const b = orderedIds.indexOf(id)
    if (a >= 0 && b >= 0) {
      const [lo, hi] = a <= b ? [a, b] : [b, a]
      const next = new Set(mods.ctrl ? state.ids : [])
      for (let i = lo; i <= hi; i++) next.add(orderedIds[i])
      return { ids: next, anchorId: state.anchorId }
    }
  }
  if (mods.ctrl) {
    const next = new Set(state.ids)
    if (next.has(id)) next.delete(id)
    else next.add(id)
    return { ids: next, anchorId: id }
  }
  return { ids: new Set([id]), anchorId: id }
}

export function selectAll(orderedIds: readonly number[], anchorId: number | null = null): SelectionState {
  return { ids: new Set(orderedIds), anchorId: anchorId ?? orderedIds[0] ?? null }
}

/** Drop ids no longer present in the list (after a filter change / refetch). */
export function pruneSelection(state: SelectionState, orderedIds: readonly number[]): SelectionState {
  if (state.ids.size === 0) return state
  const present = new Set(orderedIds)
  let changed = false
  const next = new Set<number>()
  for (const id of state.ids) {
    if (present.has(id)) next.add(id)
    else changed = true
  }
  const anchorId = state.anchorId !== null && present.has(state.anchorId) ? state.anchorId : null
  if (!changed && anchorId === state.anchorId) return state
  return { ids: next, anchorId }
}
