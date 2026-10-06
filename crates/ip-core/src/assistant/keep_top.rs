//! `group_keep_top` / `scene_keep_top`: keep the best `n` photos of every burst group / scene.
//!
//! Kept photos are picked (`flag = 1`); with `reject_rest` the others are rejected
//! (`flag = -1`). Photos that are already rejected are left alone and never compete. Inside a
//! scene the choice spreads over bursts first (the best frame of every burst, best first) before
//! a second frame of the same burst is taken, so "2 per scene" does not give two near-identical
//! shots.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::Serialize;

use crate::error::{CoreError, Result};
use crate::model::PatchRequest;
use crate::Core;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepScope {
    Group,
    Scene,
}

/// What a keep-top run decided.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct KeepTop {
    /// Photos to keep (pick).
    pub kept: Vec<i64>,
    /// The other candidates of the same groups (rejected with `reject_rest`).
    pub rest: Vec<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cand {
    pub id: i64,
    pub score: Option<f64>,
    pub rating: Option<i64>,
}

fn better(a: &Cand, b: &Cand) -> std::cmp::Ordering {
    let key = |c: &Cand| (c.score.unwrap_or(-1.0), c.rating.unwrap_or(0) as f64);
    let (ka, kb) = (key(a), key(b));
    kb.0.partial_cmp(&ka.0)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then(kb.1.partial_cmp(&ka.1).unwrap_or(std::cmp::Ordering::Equal))
        .then(a.id.cmp(&b.id))
}

/// Best first.
pub fn rank(mut v: Vec<Cand>) -> Vec<Cand> {
    v.sort_by(better);
    v
}

/// Keeps the best `n` of one burst.
pub fn keep_in_burst(photos: Vec<Cand>, n: usize) -> KeepTop {
    let ranked = rank(photos);
    KeepTop {
        kept: ranked.iter().take(n).map(|c| c.id).collect(),
        rest: ranked.iter().skip(n).map(|c| c.id).collect(),
    }
}

/// Keeps the best `n` photos of a scene made of `bursts`: round-robin over the bursts' ranks.
pub fn keep_in_scene(bursts: Vec<Vec<Cand>>, n: usize) -> KeepTop {
    let ranked: Vec<Vec<Cand>> = bursts.into_iter().map(rank).filter(|b| !b.is_empty()).collect();
    let depth = ranked.iter().map(Vec::len).max().unwrap_or(0);
    let mut order: Vec<i64> = Vec::new();
    for d in 0..depth {
        let mut level: Vec<Cand> = ranked.iter().filter_map(|b| b.get(d).copied()).collect();
        level.sort_by(better);
        order.extend(level.into_iter().map(|c| c.id));
    }
    KeepTop {
        kept: order.iter().take(n).copied().collect(),
        rest: order.iter().skip(n).copied().collect(),
    }
}

impl Core {
    /// What `group_keep_top` / `scene_keep_top` would do (nothing is changed).
    /// `selection` limits the photos that take part.
    pub async fn keep_top_plan(
        &self,
        session_id: i64,
        scope: KeepScope,
        selection: Option<&[i64]>,
        n: usize,
    ) -> Result<KeepTop> {
        if n == 0 {
            return Err(CoreError::bad_request("n must be at least 1"));
        }
        let groups = self.groups(session_id).await?;
        let ids: Vec<i64> = groups
            .scenes
            .iter()
            .flat_map(|s| s.bursts.iter().flat_map(|b| b.photo_ids.iter().copied()))
            .collect();
        let info: HashMap<i64, (Option<f64>, Option<i64>, i64)> = self
            .db
            .call(move |c| {
                let mut out = HashMap::new();
                for chunk in ids.chunks(500) {
                    let ph = vec!["?"; chunk.len()].join(",");
                    let mut st = c.prepare(&format!(
                        "SELECT id, ai_score, user_rating, COALESCE(flag,0) FROM photo WHERE id IN ({ph})"
                    ))?;
                    let rows = st
                        .query_map(rusqlite::params_from_iter(chunk.iter()), |r| {
                            Ok((r.get::<_, i64>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?)))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    out.extend(rows);
                }
                Ok(out)
            })
            .await?;
        let allowed: Option<HashSet<i64>> = selection.map(|s| s.iter().copied().collect());
        let cands = |photo_ids: &[i64]| -> Vec<Cand> {
            photo_ids
                .iter()
                .filter(|id| allowed.as_ref().is_none_or(|a| a.contains(id)))
                .filter_map(|id| {
                    let (score, rating, flag) = info.get(id)?;
                    (*flag != -1).then_some(Cand {
                        id: *id,
                        score: *score,
                        rating: *rating,
                    })
                })
                .collect()
        };
        let mut out = KeepTop::default();
        for scene in &groups.scenes {
            match scope {
                KeepScope::Group => {
                    for b in &scene.bursts {
                        let k = keep_in_burst(cands(&b.photo_ids), n);
                        out.kept.extend(k.kept);
                        out.rest.extend(k.rest);
                    }
                }
                KeepScope::Scene => {
                    let bursts: Vec<Vec<Cand>> =
                        scene.bursts.iter().map(|b| cands(&b.photo_ids)).collect();
                    let k = keep_in_scene(bursts, n);
                    out.kept.extend(k.kept);
                    out.rest.extend(k.rest);
                }
            }
        }
        Ok(out)
    }

    async fn apply_keep_top(
        self: &Arc<Self>,
        plan: &KeepTop,
        reject_rest: bool,
    ) -> Result<()> {
        if !plan.kept.is_empty() {
            self.patch_photos(PatchRequest {
                ids: plan.kept.clone(),
                flag: Some(1),
                ..Default::default()
            })
            .await?;
        }
        if reject_rest && !plan.rest.is_empty() {
            self.patch_photos(PatchRequest {
                ids: plan.rest.clone(),
                flag: Some(-1),
                ..Default::default()
            })
            .await?;
        }
        Ok(())
    }

    /// Picks the best `n` of every burst group (and with `reject_rest` rejects the others).
    pub async fn group_keep_top(
        self: &Arc<Self>,
        session_id: i64,
        selection: Option<&[i64]>,
        n: usize,
        reject_rest: bool,
    ) -> Result<KeepTop> {
        let plan = self
            .keep_top_plan(session_id, KeepScope::Group, selection, n)
            .await?;
        self.apply_keep_top(&plan, reject_rest).await?;
        Ok(plan)
    }

    /// Picks the best `n` of every scene (and with `reject_rest` rejects the others).
    pub async fn scene_keep_top(
        self: &Arc<Self>,
        session_id: i64,
        selection: Option<&[i64]>,
        n: usize,
        reject_rest: bool,
    ) -> Result<KeepTop> {
        let plan = self
            .keep_top_plan(session_id, KeepScope::Scene, selection, n)
            .await?;
        self.apply_keep_top(&plan, reject_rest).await?;
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: i64, score: f64) -> Cand {
        Cand {
            id,
            score: Some(score),
            rating: None,
        }
    }

    #[test]
    fn burst_keeps_the_best_n() {
        let k = keep_in_burst(vec![c(1, 0.2), c(2, 0.9), c(3, 0.5), c(4, 0.7)], 2);
        assert_eq!(k.kept, [2, 4]);
        assert_eq!(k.rest, [3, 1]);
        let k = keep_in_burst(vec![c(1, 0.2)], 3);
        assert_eq!(k.kept, [1]);
        assert!(k.rest.is_empty());
        assert_eq!(keep_in_burst(vec![], 2), KeepTop::default());
    }

    #[test]
    fn ties_and_missing_scores_are_deterministic() {
        let k = keep_in_burst(
            vec![
                Cand { id: 5, score: None, rating: Some(4) },
                Cand { id: 3, score: None, rating: None },
                Cand { id: 2, score: Some(0.5), rating: None },
                Cand { id: 1, score: Some(0.5), rating: Some(2) },
            ],
            2,
        );
        // equal scores: the rated one first; unscored photos come last (rating breaks ties)
        assert_eq!(k.kept, [1, 2]);
        assert_eq!(k.rest, [5, 3]);
    }

    #[test]
    fn scene_spreads_over_bursts_before_taking_second_frames() {
        // burst A: 0.95, 0.94 (near-identical frames); burst B: 0.6; burst C: 0.8
        let k = keep_in_scene(
            vec![
                vec![c(1, 0.95), c(2, 0.94)],
                vec![c(3, 0.6)],
                vec![c(4, 0.8)],
            ],
            2,
        );
        assert_eq!(k.kept, [1, 4], "best frames of different bursts, not 1 and 2");
        assert_eq!(k.rest, [3, 2]);
        // with n = 3 the weakest burst's only frame still beats a second frame
        let k = keep_in_scene(
            vec![
                vec![c(1, 0.95), c(2, 0.94)],
                vec![c(3, 0.6)],
                vec![c(4, 0.8)],
            ],
            3,
        );
        assert_eq!(k.kept, [1, 4, 3]);
        assert_eq!(k.rest, [2]);
        assert_eq!(keep_in_scene(vec![vec![], vec![]], 2), KeepTop::default());
    }
}
