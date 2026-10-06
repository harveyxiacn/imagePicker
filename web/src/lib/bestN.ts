import type { BestPeopleResponse, Person } from '@/api/types'

export interface BestSection {
  person: Person
  photos: { photoId: number; score: number }[]
}

/**
 * View model of the "best N per person" page: one section per requested person (in the order requested),
 * photos best-first, capped to `n`; people without photos are dropped.
 */
export function buildBestSections(people: Person[], resp: BestPeopleResponse | undefined, order: number[], n: number): BestSection[] {
  if (!resp) return []
  const byPerson = new Map(people.map((p) => [p.id, p]))
  const rows = new Map(resp.people.map((r) => [r.person_id, r.photos]))
  const out: BestSection[] = []
  for (const id of order) {
    const person = byPerson.get(id)
    const photos = rows.get(id)
    if (!person || !photos?.length) continue
    const seen = new Set<number>()
    const list = [...photos]
      .sort((a, b) => b.score - a.score || a.photo_id - b.photo_id)
      .filter((p) => {
        if (seen.has(p.photo_id)) return false
        seen.add(p.photo_id)
        return true
      })
      .slice(0, Math.max(1, n))
      .map((p) => ({ photoId: p.photo_id, score: p.score }))
    out.push({ person, photos: list })
  }
  return out
}

/** Folder name of a person for "export by person": their name, or `person_<id>`; duplicates get a numeric suffix. */
export function folderNames(people: Person[]): Map<number, string> {
  const used = new Map<string, number>()
  const out = new Map<number, string>()
  for (const p of people) {
    // Illegal characters in Windows folder names are replaced.
    const base = (p.name?.trim() || `person_${p.id}`).replace(/[\\/:*?"<>|]/g, '_')
    const k = used.get(base) ?? 0
    used.set(base, k + 1)
    out.set(p.id, k === 0 ? base : `${base}_${k + 1}`)
  }
  return out
}

/** `folders` body of POST /api/export built from best-N sections. */
export function sectionsToFolders(sections: BestSection[]): Record<string, number[]> {
  const names = folderNames(sections.map((s) => s.person))
  const out: Record<string, number[]> = {}
  for (const s of sections) out[names.get(s.person.id)!] = s.photos.map((p) => p.photoId)
  return out
}
