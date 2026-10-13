/**
 * Undo / redo entries for structural edits that are not rating-style PATCHes: burst-group split / merge and
 * person edits (rename, hide, merge, moving faces). Every builder returns a `HistoryEntry` whose `custom` hooks call
 * the inverse API requests; they take the API as a parameter so the logic is unit-testable without a server.
 *
 * Ids are re-learned from each response: splitting again or re-creating a person yields new ids, and the closures
 * follow them so a long undo / redo chain keeps working.
 */
import type { QueryClient } from '@tanstack/react-query'
import { qk } from './cache'
import type { HistoryEntry } from './history'

/** Everything that shows people / faces refetches after a person edit (or its undo). */
export function refreshPeople(qc: QueryClient): void {
  void qc.invalidateQueries({ queryKey: qk.peopleAll })
  void qc.invalidateQueries({ queryKey: ['analysis', 'photo'] })
  void qc.invalidateQueries({ queryKey: ['burstFaces'] })
  void qc.invalidateQueries({ queryKey: qk.photoPeopleAll })
}

export interface StructureApi {
  splitGroup: (burstId: number, atPhotoId: number) => Promise<{ burst_ids: number[] }>
  mergeGroups: (burstIds: number[]) => Promise<{ burst_id: number }>
  patchPerson: (id: number, body: { name?: string | null; hidden?: boolean }) => Promise<unknown>
  mergePeople: (ids: number[], into: number) => Promise<unknown>
  createPerson: (faceIds: number[], name?: string | null) => Promise<{ person: { id: number } }>
  setFacePerson: (faceId: number, personId: number | null) => Promise<unknown>
}

interface Common {
  label: string
  api: StructureApi
  /** refresh caches / UI after every undo or redo; receives the burst that should stay in focus (if any) */
  after?: (focusBurstId?: number) => void
}

/** Group split at `atPhotoId` produced `burstIds` = [kept, new]. Undo merges them back; redo splits again. */
export function splitGroupEntry({ label, api, after, burstIds, atPhotoId }: Common & { burstIds: number[]; atPhotoId: number }): HistoryEntry {
  let [first, second] = burstIds
  return {
    label,
    changes: [],
    custom: {
      undo: async () => {
        const r = await api.mergeGroups([first, second])
        first = r.burst_id
        after?.(first)
      },
      redo: async () => {
        const r = await api.splitGroup(first, atPhotoId)
        ;[first, second] = r.burst_ids
        after?.(first)
      },
    },
  }
}

/**
 * Two groups merged into `mergedId`. `splitAtPhotoId` is the first photo (shot order) of whichever group was later in
 * time; the server keeps the earlier group's id, so undo splits right there and redo merges the two halves again.
 */
export function mergeGroupsEntry({ label, api, after, mergedId, splitAtPhotoId }: Common & { mergedId: number; splitAtPhotoId: number }): HistoryEntry {
  let first = mergedId
  let second = -1
  return {
    label,
    changes: [],
    custom: {
      undo: async () => {
        const r = await api.splitGroup(first, splitAtPhotoId)
        ;[first, second] = r.burst_ids
        after?.(first)
      },
      redo: async () => {
        const r = await api.mergeGroups([first, second])
        first = r.burst_id
        after?.(first)
      },
    },
  }
}

/** Rename / hide / unhide: `before` and `after` hold only the fields that changed. */
export function personPatchEntry({
  label,
  api,
  after,
  personId,
  before,
  next,
}: Common & { personId: number; before: { name?: string | null; hidden?: boolean }; next: { name?: string | null; hidden?: boolean } }): HistoryEntry {
  return {
    label,
    changes: [],
    custom: {
      undo: async () => {
        await api.patchPerson(personId, before)
        after?.()
      },
      redo: async () => {
        await api.patchPerson(personId, next)
        after?.()
      },
    },
  }
}

export interface MergedPersonSnapshot {
  id: number
  name: string | null
  hidden: boolean
  /** faces the person had before the merge */
  faceIds: number[]
}

/**
 * People merged into `into`. The server deletes the merged people, so undo re-creates each from its faces (new id,
 * same name / hidden flag) and restores the survivor's previous name; redo merges the re-created people again.
 * Beauty profiles of deleted people are not restored.
 */
export function mergePeopleEntry({
  label,
  api,
  after,
  into,
  intoNameBefore,
  merged,
}: Common & { into: number; intoNameBefore: string | null; merged: MergedPersonSnapshot[] }): HistoryEntry {
  let ids = merged.map((m) => m.id)
  return {
    label,
    changes: [],
    custom: {
      undo: async () => {
        const next: number[] = []
        for (const m of merged) {
          if (m.faceIds.length === 0) continue
          const { person } = await api.createPerson(m.faceIds, m.name)
          if (m.hidden) await api.patchPerson(person.id, { hidden: true })
          next.push(person.id)
        }
        await api.patchPerson(into, { name: intoNameBefore })
        ids = next
        after?.()
      },
      redo: async () => {
        if (ids.length) await api.mergePeople([into, ...ids], into)
        after?.()
      },
    },
  }
}

/**
 * Faces moved out of their persons ("not this person", "this is a new person"). Undo puts every face back where it was;
 * `again` performs the original move for redo.
 */
export function movedFacesEntry({
  label,
  api,
  after,
  faces,
  again,
}: Common & { faces: { id: number; from: number }[]; again: () => Promise<void> }): HistoryEntry {
  return {
    label,
    changes: [],
    custom: {
      undo: async () => {
        for (const f of faces) await api.setFacePerson(f.id, f.from)
        after?.()
      },
      redo: async () => {
        await again()
        after?.()
      },
    },
  }
}
