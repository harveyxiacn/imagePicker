//! The assistant's tool registry (`docs/api-contract-m6.md` A.1): JSON schemas for the LLM and a
//! strict validator that every tool call (from the rules engine or from a model) must pass
//! before it can become a plan step.

use serde_json::{json, Map, Value};

use crate::model::{FlagFilter, PersonMode, PersonState, PhotoQuery, SortKey, COLOR_LABELS};
use crate::Issue;

pub type VResult<T> = std::result::Result<T, String>;

pub const MAX_STEPS: usize = 8;
const MAX_IDS: usize = 20_000;

pub const TOOL_NAMES: [&str; 14] = [
    "filter",
    "set_rating",
    "set_flag",
    "accept_ai",
    "group_keep_top",
    "scene_keep_top",
    "apply_preset",
    "auto_adjust",
    "apply_profiles",
    "besttake_auto",
    "remove_bystanders",
    "export",
    "describe",
    "suggest_edits",
];

pub const EXPORT_PRESETS: [&str; 4] = ["original", "wechat", "xiaohongshu", "instagram"];

pub fn is_tool(name: &str) -> bool {
    TOOL_NAMES.contains(&name)
}

/// Keys of the `filter` tool: the `/api/photos` query parameters plus `collection`.
pub const FILTER_KEYS: [&str; 18] = [
    "rating_gte",
    "flag",
    "color_label",
    "sort",
    "ai_rating_gte",
    "issues_none",
    "issues_any",
    "burst_best_only",
    "burst_id",
    "scene_type",
    "persons",
    "person_mode",
    "exclude_persons",
    "person_state",
    "include_background",
    "faces_min",
    "faces_max",
    "has_edits",
];

fn selection_schema() -> Value {
    json!({
        "description": "Which photos: {\"ids\":[...]}, {\"query\":\"<GET /api/photos query string>\"} or \"current_filter\"",
        "oneOf": [
            {"type":"object","properties":{"ids":{"type":"array","items":{"type":"integer"}}},"required":["ids"]},
            {"type":"object","properties":{"query":{"type":"string"}},"required":["query"]},
            {"type":"string","enum":["current_filter"]}
        ]
    })
}

fn filter_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "rating_gte": {"type":"integer","minimum":0,"maximum":5},
            "flag": {"type":"string","enum":["picked","rejected","unflagged","not_rejected"]},
            "color_label": {"type":"string","enum": COLOR_LABELS},
            "sort": {"type":"string","enum":["taken_at","-taken_at","name","rating","ai"]},
            "ai_rating_gte": {"type":"number","minimum":0,"maximum":5},
            "issues_none": {"type":"boolean"},
            "issues_any": {"type":"array","items":{"type":"string","enum":["closed_eyes","blurry","overexposed","underexposed","noisy","tilted"]}},
            "burst_best_only": {"type":"boolean"},
            "burst_id": {"type":"integer"},
            "scene_type": {"type":"string","description":"portrait|group|landscape|food|architecture|night|pet|other"},
            "persons": {"type":"array","items":{"type":"integer"},"description":"person ids"},
            "person_mode": {"type":"string","enum":["all","any"]},
            "exclude_persons": {"type":"array","items":{"type":"integer"}},
            "person_state": {"type":"array","items":{"type":"string","enum":["eyes_open","smiling","looking","subject"]}},
            "include_background": {"type":"boolean"},
            "faces_min": {"type":"integer","minimum":0},
            "faces_max": {"type":"integer","minimum":0},
            "has_edits": {"type":"boolean"}
        }
    })
}

/// A tool as offered to the model.
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
}

pub fn registry() -> Vec<ToolSpec> {
    let sel = selection_schema();
    let obj = |props: Value, required: &[&str]| json!({"type":"object","additionalProperties":false,"properties":props,"required":required});
    vec![
        ToolSpec {
            name: "filter",
            description: "Only change the grid filter (no data is modified). Empty arguments clear the filter.",
            parameters: filter_schema(),
        },
        ToolSpec {
            name: "set_rating",
            description: "Set the star rating (0..5, null clears) of the selected photos.",
            parameters: obj(json!({"selection":sel,"rating":{"type":["integer","null"],"minimum":0,"maximum":5}}), &["selection","rating"]),
        },
        ToolSpec {
            name: "set_flag",
            description: "Flag the selected photos: 1 pick, -1 reject, 0 clear.",
            parameters: obj(json!({"selection":sel,"flag":{"type":"integer","enum":[-1,0,1]}}), &["selection","flag"]),
        },
        ToolSpec {
            name: "accept_ai",
            description: "Accept the AI rating of the selected photos as the user rating.",
            parameters: obj(json!({"selection":sel}), &["selection"]),
        },
        ToolSpec {
            name: "group_keep_top",
            description: "In every burst group keep (pick) the best n photos; optionally reject the rest.",
            parameters: obj(json!({"selection":sel,"n":{"type":"integer","minimum":1},"reject_rest":{"type":"boolean"}}), &["n"]),
        },
        ToolSpec {
            name: "scene_keep_top",
            description: "In every scene keep (pick) the best n photos; optionally reject the rest.",
            parameters: obj(json!({"selection":sel,"n":{"type":"integer","minimum":1},"reject_rest":{"type":"boolean"}}), &["n"]),
        },
        ToolSpec {
            name: "apply_preset",
            description: "Apply a preset (look) to the selected photos.",
            parameters: obj(json!({"selection":sel,"preset_id":{"type":"string"}}), &["selection","preset_id"]),
        },
        ToolSpec {
            name: "auto_adjust",
            description: "AI one-click adjustment, saved per photo.",
            parameters: obj(json!({"selection":sel,"mode":{"type":"string","enum":["auto","portrait","landscape"]}}), &["selection"]),
        },
        ToolSpec {
            name: "apply_profiles",
            description: "Apply the saved per-person beauty profiles.",
            parameters: obj(json!({"selection":sel}), &["selection"]),
        },
        ToolSpec {
            name: "besttake_auto",
            description: "Best take: give everyone their best expression in the selected burst groups.",
            parameters: obj(json!({"selection":sel}), &["selection"]),
        },
        ToolSpec {
            name: "remove_bystanders",
            description: "Remove passers-by (non-subject faces) from the selected photos.",
            parameters: obj(json!({"selection":sel}), &["selection"]),
        },
        ToolSpec {
            name: "export",
            description: "Export the selected photos (preset: original, wechat, xiaohongshu, instagram).",
            parameters: obj(json!({"selection":sel,"preset":{"type":"string","enum":EXPORT_PRESETS},"dest":{"type":"string"}}), &["selection"]),
        },
        ToolSpec {
            name: "describe",
            description: "Describe one photo with the vision-language model.",
            parameters: obj(json!({"photo_id":{"type":"integer"}}), &["photo_id"]),
        },
        ToolSpec {
            name: "suggest_edits",
            description: "Suggest edits (an Adjust) for one photo with the vision-language model.",
            parameters: obj(json!({"photo_id":{"type":"integer"}}), &["photo_id"]),
        },
    ]
}

/// Tool definitions in the shape sent to `llm.plan` (`name`, `description`, `parameters`).
pub fn llm_tools() -> Vec<Value> {
    registry()
        .into_iter()
        .map(|t| json!({"name": t.name, "description": t.description, "parameters": t.parameters}))
        .collect()
}

// ------------------------------------------------------------------ value helpers

fn as_int(v: &Value, what: &str) -> VResult<i64> {
    match v {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
            .ok_or_else(|| format!("{what} must be an integer")),
        Value::String(s) => s
            .trim()
            .parse::<i64>()
            .map_err(|_| format!("{what} must be an integer")),
        _ => Err(format!("{what} must be an integer")),
    }
}

fn as_num(v: &Value, what: &str) -> VResult<f64> {
    let f = match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    };
    f.filter(|f| f.is_finite())
        .ok_or_else(|| format!("{what} must be a number"))
}

fn as_bool(v: &Value, what: &str) -> VResult<bool> {
    match v {
        Value::Bool(b) => Ok(*b),
        Value::Number(n) if n.as_i64() == Some(1) => Ok(true),
        Value::Number(n) if n.as_i64() == Some(0) => Ok(false),
        Value::String(s) => match s.trim() {
            "1" | "true" => Ok(true),
            "0" | "false" => Ok(false),
            _ => Err(format!("{what} must be a boolean")),
        },
        _ => Err(format!("{what} must be a boolean")),
    }
}

fn as_str<'a>(v: &'a Value, what: &str) -> VResult<&'a str> {
    v.as_str().ok_or_else(|| format!("{what} must be a string"))
}

/// An array of items or a comma separated string.
fn as_list(v: &Value, what: &str) -> VResult<Vec<Value>> {
    match v {
        Value::Array(a) => Ok(a.clone()),
        Value::String(s) => Ok(s
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| Value::String(s.to_string()))
            .collect()),
        _ => Err(format!("{what} must be a list")),
    }
}

fn one_of(v: &Value, what: &str, allowed: &[&str]) -> VResult<String> {
    let s = as_str(v, what)?.trim();
    if allowed.contains(&s) {
        Ok(s.to_string())
    } else {
        Err(format!("{what} must be one of {}", allowed.join("|")))
    }
}

fn int_in(v: &Value, what: &str, lo: i64, hi: i64) -> VResult<i64> {
    let n = as_int(v, what)?;
    if (lo..=hi).contains(&n) {
        Ok(n)
    } else {
        Err(format!("{what} must be within {lo}..{hi}"))
    }
}

fn id_list(v: &Value, what: &str, max: usize) -> VResult<Vec<i64>> {
    let l = as_list(v, what)?;
    if l.len() > max {
        return Err(format!("{what} has more than {max} entries"));
    }
    l.iter().map(|x| as_int(x, what)).collect()
}

// ------------------------------------------------------------------ filter

/// Validates and canonicalises `filter` arguments (types normalised, `null`s dropped).
pub fn norm_filter(args: &Value) -> VResult<Value> {
    let empty = Map::new();
    let obj = match args {
        Value::Null => &empty,
        Value::Object(o) => o,
        _ => return Err("filter arguments must be an object".into()),
    };
    let mut out = Map::new();
    for (k, v) in obj {
        if v.is_null() {
            continue;
        }
        let nv: Value = match k.as_str() {
            "rating_gte" => json!(int_in(v, k, 0, 5)?),
            "flag" => json!(one_of(
                v,
                k,
                &["picked", "rejected", "unflagged", "not_rejected"]
            )?),
            "color_label" => json!(one_of(v, k, &COLOR_LABELS)?),
            "sort" => json!(one_of(
                v,
                k,
                &["taken_at", "-taken_at", "name", "rating", "ai"]
            )?),
            "ai_rating_gte" => {
                let f = as_num(v, k)?;
                if !(0.0..=5.0).contains(&f) {
                    return Err("ai_rating_gte must be within 0..5".into());
                }
                json!(f)
            }
            "issues_none" | "burst_best_only" | "include_background" | "has_edits" => {
                json!(as_bool(v, k)?)
            }
            "issues_any" => {
                let keys: Vec<&str> = Issue::ALL.iter().map(|i| i.key()).collect();
                let mut l = Vec::new();
                for x in as_list(v, k)? {
                    let s = one_of(&x, k, &keys)?;
                    if !l.contains(&s) {
                        l.push(s);
                    }
                }
                json!(l)
            }
            "burst_id" => json!(int_in(v, k, 0, i64::MAX)?),
            "scene_type" => {
                let s = as_str(v, k)?.trim().to_ascii_lowercase();
                if s.is_empty()
                    || s.len() > 32
                    || !s.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                {
                    return Err("scene_type must be a short lower-case word".into());
                }
                json!(s)
            }
            "persons" | "exclude_persons" => json!(id_list(v, k, 50)?),
            "person_mode" => json!(one_of(v, k, &["all", "any"])?),
            "person_state" => {
                let mut l = Vec::new();
                for x in as_list(v, k)? {
                    let s = one_of(&x, k, &["eyes_open", "smiling", "looking", "subject"])?;
                    if !l.contains(&s) {
                        l.push(s);
                    }
                }
                json!(l)
            }
            "faces_min" | "faces_max" => json!(int_in(v, k, 0, 1000)?),
            "collection" => {
                let s = as_str(v, k)?.trim();
                if s.is_empty() || s.len() > 64 {
                    return Err("collection must be a short id".into());
                }
                json!(s)
            }
            other => return Err(format!("unknown filter key {other:?}")),
        };
        out.insert(k.clone(), nv);
    }
    Ok(Value::Object(out))
}

fn pct(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b',') {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

fn unpct(s: &str) -> VResult<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                let h = s
                    .get(i + 1..i + 3)
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                    .ok_or("bad percent escape")?;
                out.push(h);
                i += 3;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| "query is not UTF-8".to_string())
}

/// `/api/photos` query string (no `session_id`) of canonical filter arguments.
pub fn filter_query(args: &Value) -> String {
    let Some(o) = args.as_object() else {
        return String::new();
    };
    let mut parts = Vec::new();
    for key in FILTER_KEYS {
        let Some(v) = o.get(key) else { continue };
        let val = match v {
            Value::Bool(b) => (if *b { "1" } else { "0" }).to_string(),
            Value::Number(n) => n.to_string(),
            Value::String(s) => pct(s),
            Value::Array(a) => a
                .iter()
                .map(|x| match x {
                    Value::String(s) => s.clone(),
                    o => o.to_string(),
                })
                .collect::<Vec<_>>()
                .join(","),
            _ => continue,
        };
        parts.push(format!("{key}={val}"));
    }
    parts.join("&")
}

/// Parses a selection `query` (the `/api/photos` parameters, optionally with `limit`) into a
/// [`PhotoQuery`] of `session_id`.
pub fn parse_photo_query(session_id: i64, q: &str) -> VResult<PhotoQuery> {
    let q = q.trim().trim_start_matches('?');
    let mut args = Map::new();
    let mut limit = None;
    for pair in q.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let (k, v) = (unpct(k)?, unpct(v)?);
        if v.is_empty() {
            continue;
        }
        match k.as_str() {
            "limit" => limit = Some(int_in(&Value::String(v), "limit", 1, 5000)?),
            "session_id" | "cursor" => {
                return Err(format!("{k} is not allowed in a selection query"))
            }
            _ if FILTER_KEYS.contains(&k.as_str()) => {
                args.insert(k, Value::String(v));
            }
            _ => return Err(format!("unknown query parameter {k:?}")),
        }
    }
    let n = norm_filter(&Value::Object(args))?;
    Ok(photo_query_of(session_id, &n, limit))
}

/// [`PhotoQuery`] of canonical filter arguments.
pub fn photo_query_of(session_id: i64, f: &Value, limit: Option<i64>) -> PhotoQuery {
    let get = |k: &str| f.get(k);
    let ints = |k: &str| -> Vec<i64> {
        get(k)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default()
    };
    let strs = |k: &str| -> Vec<String> {
        get(k)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let boolean = |k: &str| get(k).and_then(Value::as_bool).unwrap_or(false);
    PhotoQuery {
        session_id,
        devices: Vec::new(),
        device_none: false,
        rating_gte: get("rating_gte").and_then(Value::as_i64),
        flag: get("flag")
            .and_then(Value::as_str)
            .and_then(FlagFilter::parse)
            .unwrap_or_default(),
        color_label: get("color_label")
            .and_then(Value::as_str)
            .map(str::to_string),
        sort: get("sort")
            .and_then(Value::as_str)
            .and_then(SortKey::parse)
            .unwrap_or_default(),
        cursor: None,
        limit,
        ai_rating_gte: get("ai_rating_gte").and_then(Value::as_f64),
        issues_none: boolean("issues_none"),
        issues_any: strs("issues_any")
            .iter()
            .filter_map(|s| Issue::parse(s))
            .collect(),
        burst_best_only: boolean("burst_best_only"),
        burst_id: get("burst_id").and_then(Value::as_i64),
        scene_type: get("scene_type")
            .and_then(Value::as_str)
            .map(str::to_string),
        persons: ints("persons"),
        person_mode: match get("person_mode").and_then(Value::as_str) {
            Some("any") => PersonMode::Any,
            _ => PersonMode::All,
        },
        exclude_persons: ints("exclude_persons"),
        person_state: strs("person_state")
            .iter()
            .filter_map(|s| PersonState::parse(s))
            .collect(),
        include_background: boolean("include_background"),
        faces_min: get("faces_min").and_then(Value::as_i64),
        faces_max: get("faces_max").and_then(Value::as_i64),
        has_edits: get("has_edits").and_then(Value::as_bool),
    }
}

// ------------------------------------------------------------------ selection + calls

/// Canonical selection: `{"ids":[..]}`, `{"query":".."}` or `"current_filter"`.
pub fn norm_selection(v: &Value) -> VResult<Value> {
    match v {
        Value::String(s) if s == "current_filter" => Ok(v.clone()),
        Value::Object(o) => {
            if o.len() != 1 {
                return Err("selection must have exactly one of ids / query".into());
            }
            if let Some(ids) = o.get("ids") {
                let ids = id_list(ids, "selection.ids", MAX_IDS)?;
                return Ok(json!({"ids": ids}));
            }
            if let Some(q) = o.get("query") {
                let q = as_str(q, "selection.query")?;
                if q.len() > 2000 {
                    return Err("selection.query is too long".into());
                }
                parse_photo_query(0, q).map_err(|e| format!("selection.query: {e}"))?;
                return Ok(json!({"query": q.trim().trim_start_matches('?')}));
            }
            Err("selection must have ids or query".into())
        }
        _ => {
            Err("selection must be {\"ids\":[..]}, {\"query\":\"..\"} or \"current_filter\"".into())
        }
    }
}

fn only_keys(o: &Map<String, Value>, allowed: &[&str]) -> VResult<()> {
    for k in o.keys() {
        if !allowed.contains(&k.as_str()) {
            return Err(format!("unexpected argument {k:?}"));
        }
    }
    Ok(())
}

fn req<'a>(o: &'a Map<String, Value>, k: &str) -> VResult<&'a Value> {
    o.get(k).ok_or_else(|| format!("missing argument {k:?}"))
}

/// Validates one tool call and returns its canonical arguments.
pub fn validate_call(tool: &str, args: &Value) -> VResult<Value> {
    if !is_tool(tool) {
        return Err(format!("unknown tool {tool:?}"));
    }
    if tool == "filter" {
        return norm_filter(args);
    }
    let empty = Map::new();
    let o = match args {
        Value::Object(o) => o,
        Value::Null => &empty,
        _ => return Err("arguments must be an object".into()),
    };
    let mut out = Map::new();
    let with_selection = |out: &mut Map<String, Value>, required: bool| -> VResult<()> {
        match o.get("selection") {
            Some(s) if !s.is_null() => {
                out.insert("selection".into(), norm_selection(s)?);
                Ok(())
            }
            _ if required => Err("missing argument \"selection\"".into()),
            _ => Ok(()),
        }
    };
    match tool {
        "set_rating" => {
            only_keys(o, &["selection", "rating"])?;
            with_selection(&mut out, true)?;
            let r = req(o, "rating")?;
            out.insert(
                "rating".into(),
                if r.is_null() {
                    Value::Null
                } else {
                    json!(int_in(r, "rating", 0, 5)?)
                },
            );
        }
        "set_flag" => {
            only_keys(o, &["selection", "flag"])?;
            with_selection(&mut out, true)?;
            out.insert(
                "flag".into(),
                json!(int_in(req(o, "flag")?, "flag", -1, 1)?),
            );
        }
        "accept_ai" | "apply_profiles" | "besttake_auto" | "remove_bystanders" => {
            only_keys(o, &["selection"])?;
            with_selection(&mut out, true)?;
        }
        "group_keep_top" | "scene_keep_top" => {
            only_keys(o, &["selection", "n", "reject_rest"])?;
            with_selection(&mut out, false)?;
            out.insert("n".into(), json!(int_in(req(o, "n")?, "n", 1, 100)?));
            let rr = match o.get("reject_rest") {
                Some(v) if !v.is_null() => as_bool(v, "reject_rest")?,
                _ => false,
            };
            out.insert("reject_rest".into(), json!(rr));
        }
        "apply_preset" => {
            only_keys(o, &["selection", "preset_id"])?;
            with_selection(&mut out, true)?;
            let p = as_str(req(o, "preset_id")?, "preset_id")?.trim();
            if p.is_empty() || p.len() > 64 {
                return Err("preset_id must be a short id".into());
            }
            out.insert("preset_id".into(), json!(p));
        }
        "auto_adjust" => {
            only_keys(o, &["selection", "mode"])?;
            with_selection(&mut out, true)?;
            let m = match o.get("mode") {
                Some(v) if !v.is_null() => one_of(v, "mode", &["auto", "portrait", "landscape"])?,
                _ => "auto".to_string(),
            };
            out.insert("mode".into(), json!(m));
        }
        "export" => {
            only_keys(o, &["selection", "preset", "dest"])?;
            with_selection(&mut out, true)?;
            if let Some(p) = o.get("preset").filter(|v| !v.is_null()) {
                out.insert(
                    "preset".into(),
                    json!(one_of(p, "preset", &EXPORT_PRESETS)?),
                );
            }
            if let Some(d) = o.get("dest").filter(|v| !v.is_null()) {
                let d = as_str(d, "dest")?.trim();
                if d.is_empty() || d.len() > 1024 || d.contains('\0') {
                    return Err("dest must be a path".into());
                }
                out.insert("dest".into(), json!(d));
            }
        }
        "describe" | "suggest_edits" => {
            only_keys(o, &["photo_id"])?;
            let id = int_in(req(o, "photo_id")?, "photo_id", 1, i64::MAX)?;
            out.insert("photo_id".into(), json!(id));
        }
        _ => unreachable!("checked by is_tool"),
    }
    Ok(Value::Object(out))
}

/// Tools whose effect is on edit stacks / files rather than ratings; they are undone through
/// the saved edit stacks.
pub fn edits_tool(tool: &str) -> bool {
    matches!(
        tool,
        "apply_preset" | "auto_adjust" | "apply_profiles" | "besttake_auto" | "remove_bystanders"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_schema_and_validates_a_minimal_call() {
        let reg = registry();
        assert_eq!(reg.len(), TOOL_NAMES.len());
        for t in &reg {
            assert!(is_tool(t.name));
            assert_eq!(t.parameters["type"], "object", "{}", t.name);
        }
        assert_eq!(llm_tools().len(), 14);
    }

    #[test]
    fn filter_args_are_canonicalised() {
        let v = validate_call(
            "filter",
            &json!({"persons":"3,4","rating_gte":"4","issues_none":"1","flag":"picked","scene_type":" Landscape ","x":null}),
        )
        .unwrap();
        assert_eq!(v["persons"], json!([3, 4]));
        assert_eq!(v["rating_gte"], 4);
        assert_eq!(v["issues_none"], true);
        assert_eq!(v["scene_type"], "landscape");
        let q = filter_query(&v);
        assert!(
            q.contains("persons=3,4") && q.contains("rating_gte=4") && q.contains("flag=picked")
        );
        // round trip through the query parser
        let pq = parse_photo_query(7, &q).unwrap();
        assert_eq!(pq.session_id, 7);
        assert_eq!(pq.persons, [3, 4]);
        assert_eq!(pq.rating_gte, Some(4));
        assert_eq!(pq.flag, FlagFilter::Picked);
        assert!(pq.issues_none);
    }

    #[test]
    fn validator_rejects_bad_calls() {
        let bad: Vec<(&str, Value)> = vec![
            ("rm_rf", json!({})),
            ("filter", json!({"rating_gte": 9})),
            ("filter", json!({"flag": "maybe"})),
            ("filter", json!({"nope": 1})),
            ("filter", json!({"persons": [1, "x"]})),
            ("filter", json!("rating")),
            ("filter", json!({"ai_rating_gte": 6})),
            ("filter", json!({"issues_any": ["sleepy"]})),
            ("set_rating", json!({"selection":"current_filter"})),
            (
                "set_rating",
                json!({"selection":"current_filter","rating":6}),
            ),
            ("set_rating", json!({"selection":"everything","rating":3})),
            (
                "set_rating",
                json!({"selection":{"ids":[1],"query":""},"rating":3}),
            ),
            ("set_rating", json!({"selection":{"ids":["a"]},"rating":3})),
            (
                "set_rating",
                json!({"selection":{"query":"drop=table"},"rating":3}),
            ),
            (
                "set_rating",
                json!({"selection":"current_filter","rating":3,"extra":1}),
            ),
            ("set_flag", json!({"selection":"current_filter","flag":2})),
            ("group_keep_top", json!({"n":0})),
            ("group_keep_top", json!({"n":1000})),
            ("scene_keep_top", json!({"reject_rest":true})),
            ("apply_preset", json!({"selection":"current_filter"})),
            (
                "apply_preset",
                json!({"selection":"current_filter","preset_id":""}),
            ),
            (
                "auto_adjust",
                json!({"selection":"current_filter","mode":"cartoon"}),
            ),
            (
                "export",
                json!({"selection":"current_filter","preset":"tiktok"}),
            ),
            ("export", json!({"selection":"current_filter","dest":""})),
            ("describe", json!({})),
            ("describe", json!({"photo_id":0})),
            ("suggest_edits", json!({"photo_id":"x"})),
            ("accept_ai", json!({"selection": 5})),
        ];
        for (tool, args) in bad {
            assert!(
                validate_call(tool, &args).is_err(),
                "{tool} {args} should be rejected"
            );
        }
    }

    #[test]
    fn validator_accepts_and_normalises_good_calls() {
        let v = validate_call(
            "group_keep_top",
            &json!({"n":"2","selection":{"ids":[3,3,4]}}),
        )
        .unwrap();
        assert_eq!(v["n"], 2);
        assert_eq!(v["reject_rest"], false);
        assert_eq!(v["selection"]["ids"], json!([3, 3, 4]));
        let v = validate_call(
            "set_rating",
            &json!({"selection":"current_filter","rating":null}),
        )
        .unwrap();
        assert!(v["rating"].is_null());
        let v = validate_call(
            "set_flag",
            &json!({"selection":{"query":"rating_gte=4&limit=3&sort=ai"},"flag":-1}),
        )
        .unwrap();
        assert_eq!(v["selection"]["query"], "rating_gte=4&limit=3&sort=ai");
        let v = validate_call("auto_adjust", &json!({"selection":"current_filter"})).unwrap();
        assert_eq!(v["mode"], "auto");
        let v = validate_call(
            "export",
            &json!({"selection":"current_filter","preset":"wechat"}),
        )
        .unwrap();
        assert_eq!(v["preset"], "wechat");
        assert!(validate_call("describe", &json!({"photo_id": 5})).is_ok());
    }

    #[test]
    fn query_parser_handles_encoding_and_limits() {
        let q = parse_photo_query(1, "scene_type=landscape&limit=5&sort=ai").unwrap();
        assert_eq!(q.limit, Some(5));
        assert_eq!(q.sort, SortKey::Ai);
        assert!(parse_photo_query(1, "limit=0").is_err());
        assert!(parse_photo_query(1, "limit=99999").is_err());
        assert!(parse_photo_query(1, "session_id=2").is_err());
        assert!(parse_photo_query(1, "persons=%zz").is_err());
        let q = parse_photo_query(1, "persons=1%2C2&person_mode=any").unwrap();
        assert_eq!(q.persons, [1, 2]);
        assert_eq!(q.person_mode, PersonMode::Any);
        assert!(parse_photo_query(1, "").is_ok());
    }
}
