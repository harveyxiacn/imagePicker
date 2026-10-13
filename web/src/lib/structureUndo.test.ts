import { beforeEach, describe, expect, it } from 'vitest'
import { useHistory } from './history'
import { mergeGroupsEntry, mergePeopleEntry, movedFacesEntry, personPatchEntry, splitGroupEntry, type StructureApi } from './structureUndo'

/** Records every call as `name(args)`; ids handed out by split / create are predictable. */
function fakeApi(opts: { nextBurst?: number; nextPerson?: number } = {}) {
  const calls: string[] = []
  let burst = opts.nextBurst ?? 100
  let person = opts.nextPerson ?? 500
  const api: StructureApi = {
    splitGroup: async (b, p) => {
      calls.push(`split(${b},${p})`)
      return { burst_ids: [b, burst++] }
    },
    mergeGroups: async (ids) => {
      calls.push(`merge(${ids.join(',')})`)
      return { burst_id: Math.min(...ids) }
    },
    patchPerson: async (id, body) => void calls.push(`patch(${id},${JSON.stringify(body)})`),
    mergePeople: async (ids, into) => void calls.push(`mergePeople(${ids.join(',')}->${into})`),
    createPerson: async (faces, name) => {
      calls.push(`create([${faces.join(',')}],${JSON.stringify(name ?? null)})`)
      return { person: { id: person++ } }
    },
    setFacePerson: async (f, p) => void calls.push(`face(${f}->${p})`),
  }
  return { api, calls }
}

beforeEach(() => useHistory.getState().clear())

describe('group split / merge undo', () => {
  it('undoing a split merges the halves, redo splits at the same photo again', async () => {
    const { api, calls } = fakeApi()
    const focus: (number | undefined)[] = []
    const e = splitGroupEntry({ label: 'split', api, burstIds: [10, 11], atPhotoId: 42, after: (f) => focus.push(f) })
    await e.custom!.undo()
    await e.custom!.redo()
    // the re-split yields a new second burst (100); a second undo must merge that one
    await e.custom!.undo()
    expect(calls).toEqual(['merge(10,11)', 'split(10,42)', 'merge(10,100)'])
    expect(focus).toEqual([10, 10, 10])
  })

  it('undoing a merge splits where the later group began; redo merges the new halves', async () => {
    const { api, calls } = fakeApi()
    const e = mergeGroupsEntry({ label: 'merge', api, mergedId: 7, splitAtPhotoId: 90 })
    await e.custom!.undo()
    await e.custom!.redo()
    await e.custom!.undo()
    expect(calls).toEqual(['split(7,90)', 'merge(7,100)', 'split(7,90)'])
  })

  it('travels through the shared history stack', async () => {
    const { api, calls } = fakeApi()
    useHistory.getState().push(splitGroupEntry({ label: 'split', api, burstIds: [1, 2], atPhotoId: 5 }))
    const e = useHistory.getState().undo()!
    expect(e.label).toBe('split')
    expect(e.changes).toEqual([])
    await e.custom!.undo()
    expect(useHistory.getState().redoStack).toHaveLength(1)
    expect(calls).toEqual(['merge(1,2)'])
  })
})

describe('person edits undo', () => {
  it('rename and hide restore only the changed fields', async () => {
    const { api, calls } = fakeApi()
    const rename = personPatchEntry({ label: 'rename', api, personId: 3, before: { name: null }, next: { name: 'Ann' } })
    await rename.custom!.undo()
    await rename.custom!.redo()
    const hide = personPatchEntry({ label: 'hide', api, personId: 3, before: { hidden: false }, next: { hidden: true } })
    await hide.custom!.undo()
    expect(calls).toEqual(['patch(3,{"name":null})', 'patch(3,{"name":"Ann"})', 'patch(3,{"hidden":false})'])
  })

  it('undoing a merge re-creates the merged people from their faces and restores names', async () => {
    const { api, calls } = fakeApi()
    const e = mergePeopleEntry({
      label: 'merge people',
      api,
      into: 1,
      intoNameBefore: null,
      merged: [
        { id: 2, name: 'Bob', hidden: false, faceIds: [20, 21] },
        { id: 3, name: null, hidden: true, faceIds: [30] },
        { id: 4, name: 'Gone', hidden: false, faceIds: [] },
      ],
    })
    await e.custom!.undo()
    expect(calls).toEqual([
      'create([20,21],"Bob")',
      'create([30],null)',
      'patch(501,{"hidden":true})',
      'patch(1,{"name":null})',
    ])
    calls.length = 0
    await e.custom!.redo() // merges the re-created people (new ids) into the survivor again
    expect(calls).toEqual(['mergePeople(1,500,501->1)'])
  })

  it('moved faces go back to their person; redo repeats the move', async () => {
    const { api, calls } = fakeApi()
    let again = 0
    const e = movedFacesEntry({
      label: 'move',
      api,
      faces: [
        { id: 8, from: 2 },
        { id: 9, from: 3 },
      ],
      again: async () => void again++,
    })
    await e.custom!.undo()
    await e.custom!.redo()
    expect(calls).toEqual(['face(8->2)', 'face(9->3)'])
    expect(again).toBe(1)
  })
})
