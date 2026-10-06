//! Person clustering (docs/03 §4.3): per-burst face tracks (IoU + identity similarity), then
//! cross-burst centroid linkage. User constraints: locked faces keep their person (must-link),
//! faces of the same photo never share a person (cannot-link).

use std::collections::HashSet;

use super::vecs::{cosine, normalize};

/// Same person if the cosine of identity embeddings is at least this (AuraFace: same person
/// 0.43-0.66, different people < 0.2 on real photos).
pub const PERSON_COS: f32 = 0.38;
/// Within a burst a track continues with at least this similarity (or sufficient box overlap).
pub const TRACK_COS: f32 = 0.30;
pub const TRACK_STRONG_COS: f32 = 0.45;
/// A new (unnamed) person needs at least this many distinct photos; rarer faces stay unassigned.
pub const MIN_PHOTOS_NEW_PERSON: usize = 2;

pub fn iou(a: &[f64; 4], b: &[f64; 4]) -> f64 {
    let (ax2, ay2, bx2, by2) = (a[0] + a[2], a[1] + a[3], b[0] + b[2], b[1] + b[3]);
    let iw = (ax2.min(bx2) - a[0].max(b[0])).max(0.0);
    let ih = (ay2.min(by2) - a[1].max(b[1])).max(0.0);
    let inter = iw * ih;
    let union = a[2] * a[3] + b[2] * b[3] - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

#[derive(Debug, Clone)]
pub struct TrackFace {
    /// Position of the photo in the burst's time order.
    pub frame: usize,
    pub bbox: [f64; 4],
    pub emb: Option<Vec<f32>>,
}

struct Track {
    last_bbox: [f64; 4],
    sum: Option<Vec<f32>>,
    last_frame: usize,
}

impl Track {
    fn emb(&self) -> Option<Vec<f32>> {
        self.sum.clone().map(|mut v| {
            normalize(&mut v);
            v
        })
    }
}

/// Associates the faces of one burst into tracks (one person across frames); returns a track
/// index (0-based, in order of first appearance) for every face, in input order.
pub fn assign_tracks(faces: &[TrackFace]) -> Vec<usize> {
    let mut out = vec![usize::MAX; faces.len()];
    let mut tracks: Vec<Track> = Vec::new();
    let max_frame = faces.iter().map(|f| f.frame).max().unwrap_or(0);
    for frame in 0..=max_frame {
        let idx: Vec<usize> = (0..faces.len()).filter(|&i| faces[i].frame == frame).collect();
        if idx.is_empty() {
            continue;
        }
        let mut cands: Vec<(f64, usize, usize)> = Vec::new(); // (score, face, track)
        for &fi in &idx {
            for (ti, t) in tracks.iter().enumerate() {
                let ov = iou(&faces[fi].bbox, &t.last_bbox);
                let cos = match (&faces[fi].emb, t.emb()) {
                    (Some(a), Some(b)) => Some(cosine(a, &b) as f64),
                    _ => None,
                };
                let ok = match cos {
                    Some(c) => c >= TRACK_STRONG_COS as f64 || (ov >= 0.15 && c >= TRACK_COS as f64),
                    None => ov >= 0.4,
                };
                if ok {
                    cands.push((cos.unwrap_or(0.0) + ov, fi, ti));
                }
            }
        }
        cands.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        let mut used_tracks: HashSet<usize> = HashSet::new();
        for (_, fi, ti) in cands {
            if out[fi] != usize::MAX || used_tracks.contains(&ti) {
                continue;
            }
            out[fi] = ti;
            used_tracks.insert(ti);
        }
        for &fi in &idx {
            if out[fi] == usize::MAX {
                out[fi] = tracks.len();
                tracks.push(Track {
                    last_bbox: faces[fi].bbox,
                    sum: None,
                    last_frame: frame,
                });
            }
            let t = &mut tracks[out[fi]];
            t.last_bbox = faces[fi].bbox;
            t.last_frame = frame;
            if let Some(e) = &faces[fi].emb {
                match &mut t.sum {
                    Some(s) if s.len() == e.len() => s.iter_mut().zip(e).for_each(|(a, b)| *a += b),
                    _ => t.sum = Some(e.clone()),
                }
            }
        }
    }
    let _ = tracks.iter().map(|t| t.last_frame).max();
    out
}

#[derive(Debug, Clone)]
pub struct TrackInput {
    /// Mean identity embedding (L2-normalised); `None` = no identity available.
    pub emb: Option<Vec<f32>>,
    pub photos: HashSet<i64>,
    pub n_faces: usize,
    /// A user-assigned person: the track follows it.
    pub locked_person: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Assignment {
    /// An existing person.
    Person(i64),
    /// A new person; tracks with the same cluster id belong together.
    New(usize),
    /// Not assigned (no identity embedding, or too rare to become a person).
    None,
}

struct Cluster {
    person: Option<i64>,
    sum: Vec<f32>,
    weight: f32,
    photos: HashSet<i64>,
    members: Vec<usize>,
}

impl Cluster {
    fn centroid(&self) -> Vec<f32> {
        let mut c = self.sum.clone();
        normalize(&mut c);
        c
    }
    fn add(&mut self, emb: &[f32], w: f32, photos: &HashSet<i64>, member: usize) {
        if self.sum.len() != emb.len() {
            self.sum = vec![0.0; emb.len()];
        }
        self.sum.iter_mut().zip(emb).for_each(|(a, b)| *a += b * w);
        self.weight += w;
        self.photos.extend(photos.iter().copied());
        self.members.push(member);
    }
}

/// `seeds` = (person id, centre) of known people. Returns one [`Assignment`] per track.
pub fn cluster_tracks(
    tracks: &[TrackInput],
    seeds: &[(i64, Vec<f32>)],
    threshold: f32,
) -> Vec<Assignment> {
    let mut clusters: Vec<Cluster> = seeds
        .iter()
        .map(|(pid, c)| {
            let mut sum = c.clone();
            normalize(&mut sum);
            Cluster {
                person: Some(*pid),
                sum,
                weight: 2.0,
                photos: HashSet::new(),
                members: Vec::new(),
            }
        })
        .collect();
    let mut assigned: Vec<Option<usize>> = vec![None; tracks.len()];

    // 1. locked tracks follow their person
    for (ti, t) in tracks.iter().enumerate() {
        let Some(pid) = t.locked_person else { continue };
        let ci = match clusters.iter().position(|c| c.person == Some(pid)) {
            Some(i) => i,
            None => {
                clusters.push(Cluster {
                    person: Some(pid),
                    sum: Vec::new(),
                    weight: 0.0,
                    photos: HashSet::new(),
                    members: Vec::new(),
                });
                clusters.len() - 1
            }
        };
        match &t.emb {
            // locked evidence refines the centre only mildly
            Some(e) => clusters[ci].add(e, t.n_faces as f32 * 0.5, &t.photos, ti),
            None => {
                clusters[ci].photos.extend(t.photos.iter().copied());
                clusters[ci].members.push(ti);
            }
        }
        assigned[ti] = Some(ci);
    }

    // 2. the rest: online centroid assignment, bigger tracks first
    let mut order: Vec<usize> = (0..tracks.len())
        .filter(|&i| assigned[i].is_none() && tracks[i].emb.is_some())
        .collect();
    order.sort_by(|&a, &b| tracks[b].n_faces.cmp(&tracks[a].n_faces).then(a.cmp(&b)));
    for ti in order {
        let t = &tracks[ti];
        let emb = t.emb.as_ref().unwrap();
        let mut best: Option<(f32, usize)> = None;
        for (ci, c) in clusters.iter().enumerate() {
            if c.sum.len() != emb.len() || c.weight <= 0.0 || !c.photos.is_disjoint(&t.photos) {
                continue;
            }
            let s = cosine(emb, &c.centroid());
            if s >= threshold && best.map(|(b, _)| s > b).unwrap_or(true) {
                best = Some((s, ci));
            }
        }
        let ci = match best {
            Some((_, ci)) => ci,
            None => {
                clusters.push(Cluster {
                    person: None,
                    sum: vec![0.0; emb.len()],
                    weight: 0.0,
                    photos: HashSet::new(),
                    members: Vec::new(),
                });
                clusters.len() - 1
            }
        };
        clusters[ci].add(emb, t.n_faces as f32, &t.photos, ti);
        assigned[ti] = Some(ci);
    }

    // 3. merge clusters whose centres are close (a person split by early order effects)
    let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
    for i in 0..clusters.len() {
        for j in i + 1..clusters.len() {
            if clusters[i].weight <= 0.0
                || clusters[j].weight <= 0.0
                || clusters[i].sum.len() != clusters[j].sum.len()
            {
                continue;
            }
            if matches!((clusters[i].person, clusters[j].person), (Some(a), Some(b)) if a != b) {
                continue;
            }
            let s = cosine(&clusters[i].centroid(), &clusters[j].centroid());
            if s >= threshold {
                pairs.push((s, i, j));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let mut parent: Vec<usize> = (0..clusters.len()).collect();
    fn find(p: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while p[r] != r {
            r = p[r];
        }
        let mut c = x;
        while p[c] != r {
            let n = p[c];
            p[c] = r;
            c = n;
        }
        r
    }
    for (_, i, j) in pairs {
        let (a, b) = (find(&mut parent, i), find(&mut parent, j));
        if a == b {
            continue;
        }
        let (ca, cb) = (&clusters[a], &clusters[b]);
        if matches!((ca.person, cb.person), (Some(x), Some(y)) if x != y) {
            continue;
        }
        if !ca.photos.is_disjoint(&cb.photos) {
            continue; // cannot-link: the same photo contains both
        }
        if cosine(&ca.centroid(), &cb.centroid()) < threshold {
            continue;
        }
        // merge b into a (keep a known person if either has one)
        let moved = std::mem::replace(
            &mut clusters[b],
            Cluster {
                person: None,
                sum: Vec::new(),
                weight: 0.0,
                photos: HashSet::new(),
                members: Vec::new(),
            },
        );
        let a_ref = &mut clusters[a];
        if a_ref.person.is_none() {
            a_ref.person = moved.person;
        }
        if a_ref.sum.len() != moved.sum.len() {
            a_ref.sum = vec![0.0; moved.sum.len()];
        }
        a_ref
            .sum
            .iter_mut()
            .zip(&moved.sum)
            .for_each(|(x, y)| *x += y);
        a_ref.weight += moved.weight;
        a_ref.photos.extend(moved.photos);
        a_ref.members.extend(moved.members);
        parent[b] = a;
    }

    // 4. results
    let mut new_ids: Vec<Option<usize>> = vec![None; clusters.len()];
    let mut next_new = 0usize;
    let mut out = Vec::with_capacity(tracks.len());
    for (ti, t) in tracks.iter().enumerate() {
        let Some(c0) = assigned[ti] else {
            out.push(Assignment::None);
            continue;
        };
        let c = find(&mut parent, c0);
        let cl = &clusters[c];
        match cl.person {
            Some(pid) => out.push(Assignment::Person(pid)),
            None => {
                if cl.photos.len() < MIN_PHOTOS_NEW_PERSON && t.locked_person.is_none() {
                    out.push(Assignment::None);
                } else {
                    let id = *new_ids[c].get_or_insert_with(|| {
                        next_new += 1;
                        next_new - 1
                    });
                    out.push(Assignment::New(id));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(angle: f32) -> Vec<f32> {
        let a = angle.to_radians();
        vec![a.cos(), a.sin(), 0.0]
    }

    fn tf(frame: usize, x: f64, emb: Option<Vec<f32>>) -> TrackFace {
        TrackFace {
            frame,
            bbox: [x, 0.3, 0.2, 0.3],
            emb,
        }
    }

    #[test]
    fn iou_basics() {
        let a = [0.0, 0.0, 0.5, 0.5];
        assert!((iou(&a, &a) - 1.0).abs() < 1e-9);
        assert_eq!(iou(&a, &[0.6, 0.6, 0.2, 0.2]), 0.0);
        assert!((iou(&a, &[0.25, 0.0, 0.5, 0.5]) - 1.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn tracks_follow_people_across_frames() {
        // two people swapping sides between frames: embeddings keep them apart
        let faces = vec![
            tf(0, 0.1, Some(v(0.0))),
            tf(0, 0.6, Some(v(90.0))),
            tf(1, 0.6, Some(v(2.0))),  // person A moved right
            tf(1, 0.1, Some(v(88.0))), // person B moved left
            tf(2, 0.55, Some(v(1.0))),
        ];
        let t = assign_tracks(&faces);
        assert_eq!(t[0], t[2]);
        assert_eq!(t[0], t[4]);
        assert_eq!(t[1], t[3]);
        assert_ne!(t[0], t[1]);
    }

    #[test]
    fn tracks_without_identity_use_box_overlap() {
        let faces = vec![
            tf(0, 0.1, None),
            tf(0, 0.6, None),
            tf(1, 0.12, None),
            tf(1, 0.58, None),
        ];
        let t = assign_tracks(&faces);
        assert_eq!(t[0], t[2]);
        assert_eq!(t[1], t[3]);
        assert_ne!(t[0], t[1]);
    }

    #[test]
    fn two_faces_of_one_photo_never_share_a_track() {
        let faces = vec![
            tf(0, 0.1, Some(v(0.0))),
            tf(1, 0.1, Some(v(0.0))),
            tf(1, 0.12, Some(v(1.0))), // identical person twice in a frame (mirror?) -> separate tracks
        ];
        let t = assign_tracks(&faces);
        assert_ne!(t[1], t[2]);
    }

    fn tr(emb: Option<Vec<f32>>, photos: &[i64], locked: Option<i64>) -> TrackInput {
        TrackInput {
            emb,
            photos: photos.iter().copied().collect(),
            n_faces: photos.len(),
            locked_person: locked,
        }
    }

    #[test]
    fn same_people_across_bursts_cluster_together() {
        let tracks = vec![
            tr(Some(v(0.0)), &[1, 2], None),   // A in burst 1
            tr(Some(v(90.0)), &[1, 2], None),  // B in burst 1
            tr(Some(v(10.0)), &[3, 4], None),  // A in burst 2
            tr(Some(v(100.0)), &[3, 4], None), // B in burst 2
            tr(Some(v(200.0)), &[5], None),    // a stranger seen once
            tr(None, &[6, 7], None),           // no identity
        ];
        let a = cluster_tracks(&tracks, &[], PERSON_COS);
        assert_eq!(a[0], a[2]);
        assert_eq!(a[1], a[3]);
        assert_ne!(a[0], a[1]);
        assert!(matches!(a[0], Assignment::New(_)));
        assert_eq!(a[4], Assignment::None); // only one photo -> not a person
        assert_eq!(a[5], Assignment::None);
    }

    #[test]
    fn cannot_link_when_both_are_in_the_same_photo() {
        // two near-identical embeddings in one photo must not become one person
        let tracks = vec![
            tr(Some(v(0.0)), &[1, 2], None),
            tr(Some(v(5.0)), &[1, 2], None),
        ];
        let a = cluster_tracks(&tracks, &[], PERSON_COS);
        assert_ne!(a[0], a[1]);
    }

    #[test]
    fn existing_people_are_reused_and_locks_win() {
        let tracks = vec![
            tr(Some(v(5.0)), &[1], None),             // close to person 10
            tr(Some(v(95.0)), &[2], None),            // close to person 20
            tr(Some(v(5.0)), &[3, 4], Some(20)),      // user says this is person 20 despite looking like 10
            tr(Some(v(250.0)), &[8], None),           // unknown, one photo
        ];
        let seeds = vec![(10, v(0.0)), (20, v(90.0))];
        let a = cluster_tracks(&tracks, &seeds, PERSON_COS);
        assert_eq!(a[0], Assignment::Person(10));
        assert_eq!(a[1], Assignment::Person(20));
        assert_eq!(a[2], Assignment::Person(20));
        assert_eq!(a[3], Assignment::None);
    }

    #[test]
    fn user_merge_constraint_keeps_dissimilar_tracks_together() {
        // after a manual merge both tracks are locked to person 7 although they look different
        let tracks = vec![
            tr(Some(v(0.0)), &[1, 2], Some(7)),
            tr(Some(v(120.0)), &[3, 4], Some(7)),
        ];
        let seeds = vec![(7, v(60.0))];
        let a = cluster_tracks(&tracks, &seeds, PERSON_COS);
        assert_eq!(a, vec![Assignment::Person(7), Assignment::Person(7)]);
    }

    #[test]
    fn two_known_people_never_merge() {
        let tracks: Vec<TrackInput> = vec![];
        let seeds = vec![(1, v(0.0)), (2, v(10.0))]; // very similar centres, different people
        assert!(cluster_tracks(&tracks, &seeds, PERSON_COS).is_empty());
        let tracks = vec![tr(Some(v(5.0)), &[1], None)];
        let a = cluster_tracks(&tracks, &seeds, PERSON_COS);
        assert!(matches!(a[0], Assignment::Person(1) | Assignment::Person(2)));
    }
}
