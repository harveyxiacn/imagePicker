import { describe, expect, it } from 'vitest'
import type { BestPeopleResponse, Person } from '@/api/types'
import { buildBestSections, folderNames, sectionsToFolders } from './bestN'

const person = (id: number, name: string | null, count = 10): Person => ({ id, name, cover_face_id: id * 10, photo_count: count, hidden: false })

const people = [person(1, '小明'), person(2, '小红'), person(3, null), person(4, '小明')]

const resp: BestPeopleResponse = {
  people: [
    {
      person_id: 2,
      photos: [
        { photo_id: 21, score: 0.4 },
        { photo_id: 22, score: 0.9 },
        { photo_id: 23, score: 0.6 },
        { photo_id: 22, score: 0.9 },
      ],
    },
    { person_id: 1, photos: [{ photo_id: 11, score: 0.5 }] },
    { person_id: 3, photos: [] },
  ],
}

describe('buildBestSections', () => {
  it('follows the requested order, sorts best-first, dedupes and caps to N', () => {
    const s = buildBestSections(people, resp, [2, 1], 2)
    expect(s.map((x) => x.person.id)).toEqual([2, 1])
    expect(s[0].photos).toEqual([
      { photoId: 22, score: 0.9 },
      { photoId: 23, score: 0.6 },
    ])
    expect(s[1].photos).toEqual([{ photoId: 11, score: 0.5 }])
  })

  it('drops people without photos or unknown people, and handles a missing response', () => {
    expect(buildBestSections(people, resp, [3, 99, 2], 5).map((x) => x.person.id)).toEqual([2])
    expect(buildBestSections(people, undefined, [1, 2], 3)).toEqual([])
  })

  it('always shows at least one photo', () => {
    expect(buildBestSections(people, resp, [2], 0)[0].photos).toHaveLength(1)
  })
})

describe('export folders by person', () => {
  it('names folders after people, falls back to person_<id>, de-duplicates and sanitises', () => {
    const names = folderNames([person(1, '小明'), person(2, null), person(4, '小明'), person(5, 'A/B:C')])
    expect(names.get(1)).toBe('小明')
    expect(names.get(2)).toBe('person_2')
    expect(names.get(4)).toBe('小明_2')
    expect(names.get(5)).toBe('A_B_C')
  })

  it('builds the `folders` export body from the sections', () => {
    const s = buildBestSections(people, resp, [2, 1], 2)
    expect(sectionsToFolders(s)).toEqual({ 小红: [22, 23], 小明: [11] })
  })
})
