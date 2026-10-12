//! Which photos a plan step works on (`docs/api-contract-m6.md` A.4).
//!
//! Every step that changes photos must have a target the plan can justify: the matches of a
//! `filter` step earlier in the same plan, an explicit query with a condition, photos the user
//! selected / opened, or — only when the request itself names no condition — the current view or
//! the whole session. [`bind`] resolves `"current_filter"` against the plan's own `filter` steps
//! and classifies every target; [`llm_problem`] refuses model plans whose targets are not
//! justified.

use serde_json::{json, Value};

use super::rules::{Call, Locale};
use super::tools;
use super::PlanContext;

#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    /// `filter`, `describe`, `suggest_edits`: no photo is changed.
    NoPhotos,
    /// An explicit query that narrows the photos (a condition or a `limit`); a `filter` step of
    /// the same plan becomes one.
    Query(String),
    /// Photo ids; `from_context` = every id is selected in the UI or is the open photo.
    Ids { from_context: bool },
    /// The grid's current filter, sent with the request (a condition the user set up).
    View(String),
    /// Every photo of the session: no condition anywhere.
    Session,
}

impl Target {
    /// No condition of its own: the whole current view or the whole session.
    pub fn unscoped(&self) -> bool {
        matches!(self, Self::View(_) | Self::Session)
    }
}

/// Tools that change photos (everything but `filter` and the read-only VLM tools).
pub fn changes_photos(tool: &str) -> bool {
    !matches!(tool, "filter" | "describe" | "suggest_edits")
}

fn query_target(q: &str) -> Target {
    if tools::query_has_conditions(q) {
        Target::Query(q.trim().trim_start_matches('?').to_string())
    } else {
        Target::Session
    }
}

/// Binds `"current_filter"` to the plan's own `filter` steps and classifies every call's target.
///
/// After a `filter` step, `"current_filter"` means that filter's matches (the grid shows them once
/// the plan runs), never the view the request was sent from: the selection is rewritten to an
/// explicit `{"query": ..}` so that execution resolves exactly the photos the plan counted. A
/// keep-top call without `selection` after a `filter` step is limited to it the same way.
pub fn bind(calls: &mut [Call], ctx: &PlanContext) -> Vec<Target> {
    let mut plan_filter: Option<String> = None;
    let mut out = Vec::with_capacity(calls.len());
    for c in calls.iter_mut() {
        if c.tool == "filter" {
            plan_filter = Some(tools::filter_query(&c.args));
            out.push(Target::NoPhotos);
            continue;
        }
        if !changes_photos(&c.tool) {
            out.push(Target::NoPhotos);
            continue;
        }
        let keep_top = matches!(c.tool.as_str(), "group_keep_top" | "scene_keep_top");
        let sel = c.args.get("selection").cloned();
        let current = matches!(&sel, Some(Value::String(s)) if s == "current_filter");
        if let Some(q) = &plan_filter {
            if current || (keep_top && sel.is_none()) {
                if let Value::Object(m) = &mut c.args {
                    m.insert("selection".into(), json!({ "query": q }));
                }
                out.push(query_target(q));
                continue;
            }
        }
        out.push(match sel {
            // keep-top over every burst / scene of the session
            None => Target::Session,
            Some(Value::String(_)) => match ctx.filter.as_deref().map(str::trim) {
                Some(f) if !f.is_empty() && tools::query_has_conditions(f) => {
                    Target::View(f.to_string())
                }
                _ => Target::Session,
            },
            Some(Value::Object(o)) => match o.get("query") {
                Some(q) => query_target(q.as_str().unwrap_or_default()),
                None => {
                    let ids: Vec<i64> = o
                        .get("ids")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(Value::as_i64).collect())
                        .unwrap_or_default();
                    Target::Ids {
                        from_context: ids.iter().all(|id| {
                            ctx.selection.contains(id) || ctx.current_photo_id == Some(*id)
                        }),
                    }
                }
            },
            Some(_) => Target::Session,
        });
    }
    out
}

/// Whether the rules engine's reading of the same request also acts on the whole view / session
/// (the request names no condition): the only case in which a model step may do so.
/// `rules_unsure`: the rules did not understand the request or only approximated it (hints such
/// as an unknown place), so they cannot vouch for anything.
pub fn rules_unscoped(rules_calls: &[Call], rules_unsure: bool, ctx: &PlanContext) -> bool {
    if rules_unsure {
        return false;
    }
    let mut calls = rules_calls.to_vec();
    let acting: Vec<Target> = bind(&mut calls, ctx)
        .into_iter()
        .filter(|t| *t != Target::NoPhotos)
        .collect();
    !acting.is_empty() && acting.iter().all(Target::unscoped)
}

/// Why a model plan may not run as proposed; `None` when every target is justified. `calls` and
/// `targets` come from [`bind`]; `rules_unscoped` from [`rules_unscoped`].
pub fn llm_problem(
    calls: &[Call],
    targets: &[Target],
    rules_unscoped: bool,
    l: Locale,
) -> Option<String> {
    let zh = l == Locale::Zh;
    for (c, t) in calls.iter().zip(targets) {
        match t {
            Target::Ids {
                from_context: false,
            } => {
                return Some(if zh {
                    format!("模型的 {} 步骤指定了既未选中也未打开的照片。", c.tool)
                } else {
                    format!(
                        "the model's {} step names photos that are neither selected nor open",
                        c.tool
                    )
                })
            }
            t if t.unscoped() && !rules_unscoped => {
                let what = match (t, zh) {
                    (Target::View(_), true) => "当前视图的全部照片",
                    (Target::View(_), false) => "the whole current view",
                    (_, true) => "本会话的全部照片",
                    (_, false) => "every photo in the session",
                };
                return Some(if zh {
                    format!(
                        "模型的 {} 步骤没有自己的筛选条件，会作用于{what}，而请求里提到了条件（或无法确认要作用于全部）。",
                        c.tool
                    )
                } else {
                    format!(
                        "the model's {} step has no condition of its own and would act on {what}, \
                         but the request names a condition (or does not clearly mean all photos)",
                        c.tool
                    )
                });
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(tool: &str, args: Value) -> Call {
        Call {
            tool: tool.into(),
            args,
        }
    }

    fn ctx(filter: &str, selection: &[i64], current: Option<i64>) -> PlanContext {
        PlanContext {
            filter: Some(filter.into()),
            selection: selection.to_vec(),
            current_photo_id: current,
            locale: None,
        }
    }

    #[test]
    fn current_filter_after_a_filter_step_is_that_filter() {
        let mut calls = vec![
            call("filter", json!({"issues_any": ["blurry"]})),
            call(
                "set_flag",
                json!({"selection": "current_filter", "flag": -1}),
            ),
        ];
        // the request came from an unfiltered grid: the step must still be the blurry photos
        let t = bind(&mut calls, &ctx("", &[], None));
        assert_eq!(t[0], Target::NoPhotos);
        assert_eq!(t[1], Target::Query("issues_any=blurry".into()));
        assert_eq!(
            calls[1].args["selection"],
            json!({"query": "issues_any=blurry"})
        );
        // ... and from a filtered grid, too: the plan's filter replaces the view
        let mut calls = vec![
            call("filter", json!({"issues_any": ["closed_eyes"]})),
            call(
                "set_flag",
                json!({"selection": "current_filter", "flag": -1}),
            ),
        ];
        let t = bind(&mut calls, &ctx("flag=picked", &[], None));
        assert_eq!(t[1], Target::Query("issues_any=closed_eyes".into()));
        // keep-top without a selection is limited to the plan's filter
        let mut calls = vec![
            call("filter", json!({"scene_type": "portrait"})),
            call("group_keep_top", json!({"n": 1, "reject_rest": true})),
        ];
        let t = bind(&mut calls, &ctx("", &[], None));
        assert_eq!(t[1], Target::Query("scene_type=portrait".into()));
        assert_eq!(
            calls[1].args["selection"],
            json!({"query": "scene_type=portrait"})
        );
        // a filter that clears everything does not narrow anything
        let mut calls = vec![
            call("filter", json!({})),
            call(
                "set_flag",
                json!({"selection": "current_filter", "flag": -1}),
            ),
        ];
        assert_eq!(bind(&mut calls, &ctx("", &[], None))[1], Target::Session);
    }

    #[test]
    fn targets_without_a_plan_filter() {
        let c = ctx("rating_gte=4", &[3, 4], Some(9));
        let mut calls = vec![
            call(
                "set_flag",
                json!({"selection": "current_filter", "flag": -1}),
            ),
            call(
                "set_rating",
                json!({"selection": {"query": ""}, "rating": 2}),
            ),
            call(
                "set_rating",
                json!({"selection": {"query": "sort=ai"}, "rating": 2}),
            ),
            call(
                "set_rating",
                json!({"selection": {"query": "sort=ai&limit=3"}, "rating": 5}),
            ),
            call(
                "set_flag",
                json!({"selection": {"query": "issues_none=0"}, "flag": -1}),
            ),
            call("set_flag", json!({"selection": {"ids": [3, 9]}, "flag": 1})),
            call(
                "set_flag",
                json!({"selection": {"ids": [3, 77]}, "flag": 1}),
            ),
            call("scene_keep_top", json!({"n": 1, "reject_rest": false})),
            call("describe", json!({"photo_id": 9})),
        ];
        let t = bind(&mut calls, &c);
        assert_eq!(t[0], Target::View("rating_gte=4".into()));
        assert_eq!(t[1], Target::Session, "an empty query is everything");
        assert_eq!(t[2], Target::Session, "sorting narrows nothing");
        assert_eq!(t[3], Target::Query("sort=ai&limit=3".into()));
        assert_eq!(t[4], Target::Session, "a false flag is no condition");
        assert_eq!(t[5], Target::Ids { from_context: true });
        assert_eq!(
            t[6],
            Target::Ids {
                from_context: false
            }
        );
        assert_eq!(t[7], Target::Session);
        assert_eq!(t[8], Target::NoPhotos);
        // without a grid filter `current_filter` is the whole session
        let mut calls = vec![call(
            "set_flag",
            json!({"selection": "current_filter", "flag": -1}),
        )];
        assert_eq!(bind(&mut calls, &ctx("", &[], None))[0], Target::Session);
        assert_eq!(
            bind(&mut calls, &ctx("sort=rating", &[], None))[0],
            Target::Session
        );
    }

    #[test]
    fn model_targets_need_a_condition_unless_the_request_has_none() {
        let c = ctx("", &[], None);
        // "把闭眼的照片都淘汰": the model left out the filter
        let mut calls = vec![call(
            "set_flag",
            json!({"selection": "current_filter", "flag": -1}),
        )];
        let t = bind(&mut calls, &c);
        let rules = vec![
            call("filter", json!({"issues_any": ["closed_eyes"]})),
            call(
                "set_flag",
                json!({"selection": {"query": "issues_any=closed_eyes"}, "flag": -1}),
            ),
        ];
        let agree = rules_unscoped(&rules, false, &c);
        assert!(!agree);
        assert!(llm_problem(&calls, &t, agree, Locale::Zh).is_some());
        // "reject everything": the rules read it as the whole session too
        let rules_all = vec![call(
            "set_flag",
            json!({"selection": {"query": ""}, "flag": -1}),
        )];
        assert!(rules_unscoped(&rules_all, false, &c));
        assert!(llm_problem(&calls, &t, true, Locale::En).is_none());
        // an unsupported rules reading never vouches for the model
        assert!(!rules_unscoped(&rules_all, true, &c));
        assert!(!rules_unscoped(&[], false, &c));
        // the model's filter + current_filter is fine on its own
        let mut calls = vec![
            call("filter", json!({"issues_any": ["blurry"]})),
            call(
                "set_flag",
                json!({"selection": "current_filter", "flag": -1}),
            ),
        ];
        let t = bind(&mut calls, &c);
        assert!(llm_problem(&calls, &t, false, Locale::En).is_none());
        // invented ids are refused
        let mut calls = vec![call(
            "set_flag",
            json!({"selection": {"ids": [5]}, "flag": -1}),
        )];
        let t = bind(&mut calls, &ctx("", &[1, 2], Some(3)));
        let why = llm_problem(&calls, &t, true, Locale::En).unwrap();
        assert!(why.contains("neither selected nor open"), "{why}");
    }
}
