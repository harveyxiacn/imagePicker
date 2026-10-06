//! M3 editing: edit stacks, presets, LUTs, previews, auto-adjust, sync and AI masks.
//! Contract: `docs/api-contract-m3.md` (sections B, C and the core side of D).

pub mod beauty;
pub mod luts;
pub mod patch;
pub mod service;
pub mod store;
pub mod sync;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use ip_render::{Adjust, AutoContext, AutoMode, Backend, EditStack, MaskRef, MaskTarget, Op, Warp};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use self::service::{encode_jpeg, parse_target, MaskMode};
use self::sync::SyncKind;
use crate::catalog::{self, now_ms};
use crate::error::{CoreError, Result};
use crate::events::Event;
use crate::model::EditUpdate;
use crate::Core;

pub use self::service::RenderService;

/// Maximum number of ops accepted in one stack.
pub const MAX_OPS: usize = 64;
pub const PREVIEW_DEFAULT_EDGE: u32 = 1600;
pub const PREVIEW_MAX_EDGE: u32 = 8192;
pub const PREVIEW_QUALITY: u8 = 90;
/// Long edge of the proxies `sync --adaptive` compares.
const MATCH_EDGE: u32 = 384;
/// Grid thumbnail regenerated eagerly after an edit.
const GRID: u32 = 256;

/// Cache key of the rendered images of `(photo, stack)`.
pub fn edited_key(fast_key: &str, hash: &str) -> String {
    blake3::hash(format!("{fast_key}:{hash}").as_bytes()).to_hex()[..32].to_string()
}

pub fn stack_hash(raw: &Value) -> String {
    blake3::hash(raw.to_string().as_bytes()).to_hex()[..16].to_string()
}

pub fn empty_stack_json() -> Value {
    json!({"version": 1, "ops": []})
}

// ------------------------------------------------------------------ validation

fn bad(msg: impl Into<String>) -> CoreError {
    CoreError::Unprocessable(msg.into())
}

fn finite(name: &str, v: f32) -> Result<f32> {
    if v.is_finite() {
        Ok(v)
    } else {
        Err(bad(format!("{name} must be a finite number")))
    }
}

fn range(name: &str, v: f32, lo: f32, hi: f32) -> Result<()> {
    finite(name, v)?;
    if v < lo || v > hi {
        return Err(bad(format!("{name} must be within {lo}..{hi}")));
    }
    Ok(())
}

fn check_curve(name: &str, pts: &[[f32; 2]]) -> Result<()> {
    if pts.len() > 32 {
        return Err(bad(format!("curve {name} has more than 32 points")));
    }
    let mut last = f32::MIN;
    for p in pts {
        range(&format!("curve {name} x"), p[0], 0.0, 1.0)?;
        range(&format!("curve {name} y"), p[1], 0.0, 1.0)?;
        if p[0] < last {
            return Err(bad(format!("curve {name} points must be sorted by x")));
        }
        last = p[0];
    }
    Ok(())
}

/// Range check of an [`Adjust`] from outside the edit module (VLM suggestions).
pub(crate) fn check_adjust_public(a: &Adjust) -> Result<()> {
    check_adjust(a)
}

fn check_adjust(a: &Adjust) -> Result<()> {
    range("exposure", a.exposure, -5.0, 5.0)?;
    range("temp", a.temp, -3000.0, 3000.0)?;
    for (n, v) in [
        ("contrast", a.contrast),
        ("highlights", a.highlights),
        ("shadows", a.shadows),
        ("whites", a.whites),
        ("blacks", a.blacks),
        ("tint", a.tint),
        ("vibrance", a.vibrance),
        ("saturation", a.saturation),
        ("clarity", a.clarity),
        ("dehaze", a.dehaze),
    ] {
        range(n, v, -100.0, 100.0)?;
    }
    if let Some(c) = &a.curve {
        check_curve("rgb", &c.rgb)?;
        check_curve("r", &c.r)?;
        check_curve("g", &c.g)?;
        check_curve("b", &c.b)?;
    }
    for h in a.hsl.values() {
        range("hsl h", h.h, -100.0, 100.0)?;
        range("hsl s", h.s, -100.0, 100.0)?;
        range("hsl l", h.l, -100.0, 100.0)?;
    }
    if let Some(g) = &a.grading {
        for (n, w) in [
            ("shadows", g.shadows),
            ("midtones", g.midtones),
            ("highlights", g.highlights),
        ] {
            range(&format!("grading {n} hue"), w[0], 0.0, 360.0)?;
            range(&format!("grading {n} amount"), w[1], 0.0, 1.0)?;
        }
        range("grading balance", g.balance, -100.0, 100.0)?;
    }
    Ok(())
}

fn check_op(op: &Op) -> Result<()> {
    match op {
        Op::Crop(c) => {
            for v in c.rect {
                finite("crop rect", v)?;
            }
            let [x, y, w, h] = c.rect;
            if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
                return Err(bad("crop rect x/y must be within 0..1"));
            }
            if w <= 0.001 || h <= 0.001 || x + w > 1.0001 || y + h > 1.0001 {
                return Err(bad("crop rect must be a non-empty region inside the image"));
            }
            range("crop angle", c.angle, -45.0, 45.0)?;
        }
        Op::Global(a) => check_adjust(a)?,
        Op::Local(l) => {
            range("local amount", l.amount, 0.0, 1.0)?;
            match &l.mask {
                MaskRef::Ai { .. } => {}
                MaskRef::Radial {
                    center,
                    radius,
                    feather,
                } => {
                    for v in center {
                        range("radial center", *v, 0.0, 1.0)?;
                    }
                    for v in radius {
                        range("radial radius", *v, 0.001, 2.0)?;
                    }
                    range("radial feather", *feather, 0.0, 1.0)?;
                }
                MaskRef::Linear { start, end } => {
                    for v in start.iter().chain(end) {
                        range("linear mask point", *v, -0.5, 1.5)?;
                    }
                    if start == end {
                        return Err(bad("linear mask start and end must differ"));
                    }
                }
            }
            check_adjust(&l.adjust)?;
        }
        Op::Lut(l) => {
            if l.file.trim().is_empty() || l.file.len() > 1024 {
                return Err(bad("lut file must be a non-empty id or path"));
            }
            range("lut amount", l.amount, 0.0, 1.0)?;
        }
        Op::OutputSharpen(s) => range("output_sharpen amount", s.amount, 0.0, 100.0)?,
        Op::Beauty(b) => {
            check_person("beauty", b.person_id)?;
            for (n, v) in [
                ("beauty smooth", b.smooth),
                ("beauty whiten", b.whiten),
                ("beauty eye_brighten", b.eye_brighten),
                ("beauty teeth_whiten", b.teeth_whiten),
                ("beauty dark_circles", b.dark_circles),
            ] {
                range(n, v, 0.0, 100.0)?;
            }
        }
        Op::Warp(Warp::Face {
            person_id,
            slim,
            chin,
            eyes,
            nose,
            ..
        }) => {
            check_person("warp", *person_id)?;
            for (n, v) in [
                ("warp slim", *slim),
                ("warp chin", *chin),
                ("warp eyes", *eyes),
                ("warp nose", *nose),
            ] {
                range(n, v, -100.0, 100.0)?;
            }
        }
        Op::Warp(Warp::Body {
            person_id,
            arms,
            legs,
            waist,
            lengthen_legs,
            ..
        }) => {
            check_person("warp", *person_id)?;
            for (n, v) in [
                ("warp arms", *arms),
                ("warp legs", *legs),
                ("warp waist", *waist),
                ("warp lengthen_legs", *lengthen_legs),
            ] {
                range(n, v, 0.0, 100.0)?;
            }
        }
        Op::Patch(p) => {
            if !patch::valid_asset_id(&p.asset) {
                return Err(bad(
                    "patch asset must be 1..96 characters of letters, digits, '_' or '-'",
                ));
            }
            for v in p.rect {
                finite("patch rect", v)?;
            }
            let [x, y, w, h] = p.rect;
            if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
                return Err(bad("patch rect x/y must be within 0..1"));
            }
            if w <= 0.0 || h <= 0.0 || x + w > 1.0001 || y + h > 1.0001 {
                return Err(bad(
                    "patch rect must be a non-empty region inside the image",
                ));
            }
            range("patch feather", p.feather, 0.0, 0.5)?;
            range("patch amount", p.amount, 0.0, 1.0)?;
            check_person("patch", p.person_id)?;
            if p.source_photo_id.is_some_and(|i| i < 1) {
                return Err(bad("patch source_photo_id must be a positive id or null"));
            }
        }
        Op::Warp(Warp::Unknown) | Op::Unknown => {}
    }
    Ok(())
}

fn check_person(what: &str, person_id: Option<i64>) -> Result<()> {
    match person_id {
        Some(p) if p < 1 => Err(bad(format!(
            "{what} person_id must be a positive id or null"
        ))),
        _ => Ok(()),
    }
}

/// True when the op cannot change a pixel (unknown types, portrait ops with nothing dialled in).
/// A stack of only such ops is not an edit (`has_edits` = false).
pub fn op_is_noop(op: &Op) -> bool {
    match op {
        Op::Unknown | Op::Warp(Warp::Unknown) => true,
        Op::Patch(p) => !p.enabled || p.amount <= 0.0,
        Op::Beauty(b) => {
            !b.blemish
                && [
                    b.smooth,
                    b.whiten,
                    b.eye_brighten,
                    b.teeth_whiten,
                    b.dark_circles,
                ]
                .iter()
                .all(|v| *v == 0.0)
        }
        Op::Warp(Warp::Face {
            slim,
            chin,
            eyes,
            nose,
            ..
        }) => [*slim, *chin, *eyes, *nose].iter().all(|v| *v == 0.0),
        Op::Warp(Warp::Body {
            arms,
            legs,
            waist,
            lengthen_legs,
            ..
        }) => [*arms, *legs, *waist, *lengthen_legs]
            .iter()
            .all(|v| *v == 0.0),
        _ => false,
    }
}

/// A validated stack: the typed view, the JSON to store (as received, so ops this build does
/// not know survive) and whether it changes the picture.
#[derive(Debug, Clone)]
pub struct CheckedStack {
    pub stack: EditStack,
    pub raw: Value,
    pub has_edits: bool,
}

/// Parses and range-checks an edit stack. Every failure is a 422 `unprocessable`.
pub fn validate_stack(raw: Value) -> Result<CheckedStack> {
    let mut raw = raw;
    let obj = raw
        .as_object_mut()
        .ok_or_else(|| bad("stack must be an object {\"version\":1,\"ops\":[...]}"))?;
    match obj.get("ops") {
        None | Some(Value::Array(_)) => {}
        Some(_) => return Err(bad("stack.ops must be an array")),
    }
    obj.entry("ops").or_insert_with(|| json!([]));
    obj.entry("version").or_insert_with(|| json!(1));
    let stack: EditStack = serde_json::from_value(Value::Object(obj.clone()))
        .map_err(|e| bad(format!("invalid edit stack: {e}")))?;
    if stack.version != 1 {
        return Err(bad(format!(
            "unsupported stack version {} (this build understands 1)",
            stack.version
        )));
    }
    if stack.ops.len() > MAX_OPS {
        return Err(bad(format!("a stack may hold at most {MAX_OPS} ops")));
    }
    for op in &stack.ops {
        check_op(op)?;
    }
    let has_edits = !stack.ops.iter().all(op_is_noop);
    Ok(CheckedStack {
        stack,
        raw,
        has_edits,
    })
}

/// [`validate_stack`] plus the checks that need the photo: every `patch` asset must exist
/// among the photo's assets (a patch is specific to the photo it was generated for).
pub fn validate_stack_for(
    raw: Value,
    photo_id: i64,
    patches: &patch::PatchStore,
) -> Result<CheckedStack> {
    let checked = validate_stack(raw)?;
    for op in &checked.stack.ops {
        if let Op::Patch(p) = op {
            if !patches.exists(photo_id, &p.asset) {
                return Err(bad(format!(
                    "patch asset {:?} does not exist for photo {photo_id}",
                    p.asset
                )));
            }
        }
    }
    Ok(checked)
}

fn parse_saved(raw: &Value) -> Result<EditStack> {
    serde_json::from_value(raw.clone())
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("stored edit stack is invalid: {e}")))
}

pub(crate) fn ops_of(raw: &Value) -> Vec<Value> {
    raw.get("ops")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn stack_json(ops: Vec<Value>) -> Value {
    json!({"version": 1, "ops": ops})
}

// ------------------------------------------------------------------ wire types

#[derive(Debug, Clone, Serialize)]
pub struct EditDoc {
    pub photo_id: i64,
    pub stack: Value,
    pub updated_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EditPut {
    pub photo_id: i64,
    pub stack: Value,
    pub updated_at: i64,
    pub thumb_version: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PreviewRequest {
    pub photo_id: i64,
    #[serde(default)]
    pub stack: Option<Value>,
    #[serde(default)]
    pub long_edge: Option<u32>,
    #[serde(default)]
    pub original: Option<bool>,
}

#[derive(Debug)]
pub struct Preview {
    pub jpeg: Vec<u8>,
    pub ms: u64,
    pub backend: Backend,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AutoRequest {
    pub mode: AutoMode,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SyncRequest {
    pub from_id: i64,
    pub to_ids: Vec<i64>,
    pub include: Vec<SyncKind>,
    #[serde(default)]
    pub adaptive: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PresetOut {
    pub id: String,
    pub name: String,
    pub builtin: bool,
    pub stack: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PresetCreate {
    pub name: String,
    pub stack: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct LutOut {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LutInfo {
    pub id: String,
    /// i18n key (`lut.<id>`) for built-ins, the file stem for imported LUTs.
    pub name: String,
    pub builtin: bool,
}

fn user_preset_id(n: i64) -> String {
    format!("user_{n}")
}

/// Ops that make sense in a preset: no geometry, no person-specific AI masks.
fn preset_ops(ops: &[Value]) -> Vec<Value> {
    ops.iter()
        .filter(|o| match sync::op_type(o) {
            Some("global") | Some("lut") | Some("output_sharpen") => true,
            Some("local") => {
                let m = o.get("mask");
                let kind = m.and_then(|m| m.get("kind")).and_then(Value::as_str);
                let target = m.and_then(|m| m.get("target")).and_then(Value::as_str);
                !(kind == Some("ai") && target == Some("person"))
            }
            _ => false,
        })
        .cloned()
        .collect()
}

// ------------------------------------------------------------------ Core API

impl Core {
    pub fn render_service(&self) -> &Arc<RenderService> {
        &self.render
    }

    /// Emits `edits.updated` once the grid thumbnail of each photo is rendered (or rendering
    /// failed, in which case the client simply falls back to what `/api/thumb` serves).
    pub(crate) fn spawn_edit_refresh(self: &Arc<Self>, ids: Vec<i64>) {
        if ids.is_empty() {
            return;
        }
        let core = self.clone();
        tokio::spawn(async move { core.refresh_edits(ids).await });
    }

    /// The awaited form of [`Core::spawn_edit_refresh`] (generation tasks use it to order
    /// `edits.updated` before their `*.done` event).
    pub(crate) async fn refresh_edits(self: &Arc<Self>, ids: Vec<i64>) {
        for id in ids {
            let Ok(r) = self.db.call(move |c| catalog::photo_ref(c, id)).await else {
                continue; // deleted meanwhile
            };
            let has_edits = r.edit_hash.is_some();
            if has_edits {
                // failures are logged inside; the event is still sent
                let _ = self.render.edited_image(r.clone(), GRID).await;
            }
            self.events.emit(Event::EditsUpdated {
                items: vec![EditUpdate {
                    id,
                    has_edits,
                    thumb_version: r.thumb_version(),
                }],
            });
        }
    }

    /// Deletes the patch assets of a photo that no retained edit version refers to.
    pub(crate) async fn gc_patches(&self, photo_id: i64) {
        let keep = self
            .db
            .call(move |c| store::all_stacks(c, photo_id))
            .await
            .map(|stacks| {
                stacks
                    .iter()
                    .flat_map(patch::assets_in_stack)
                    .collect::<HashSet<String>>()
            });
        if let Ok(keep) = keep {
            let svc = self.render.clone();
            let _ = tokio::task::spawn_blocking(move || svc.patches.gc(photo_id, &keep)).await;
        }
    }

    pub async fn get_edit(&self, photo_id: i64) -> Result<EditDoc> {
        let cur = self
            .db
            .call(move |c| {
                catalog::photo_ref(c, photo_id)?;
                store::current(c, photo_id)
            })
            .await?;
        Ok(match cur {
            Some(c) => EditDoc {
                photo_id,
                stack: c.stack,
                updated_at: Some(c.updated_at),
            },
            None => EditDoc {
                photo_id,
                stack: empty_stack_json(),
                updated_at: None,
            },
        })
    }

    /// Saves the stack (an empty one resets the photo) and broadcasts `edits.updated` once the
    /// new grid thumbnail exists.
    pub async fn put_edit(self: &Arc<Self>, photo_id: i64, raw: Value) -> Result<EditPut> {
        let checked = validate_stack_for(raw, photo_id, &self.render.patches)?;
        if checked.stack.ops.is_empty() {
            self.delete_edit(photo_id).await?;
            let r = self
                .db
                .call(move |c| catalog::photo_ref(c, photo_id))
                .await?;
            return Ok(EditPut {
                photo_id,
                stack: empty_stack_json(),
                updated_at: now_ms(),
                thumb_version: r.thumb_version(),
            });
        }
        let hash = stack_hash(&checked.raw);
        let before = self
            .db
            .call(move |c| catalog::photo_ref(c, photo_id))
            .await?;
        let (raw, has_edits, h2, now) = (
            checked.raw.clone(),
            checked.has_edits,
            hash.clone(),
            now_ms(),
        );
        let out = self
            .db
            .call(move |c| store::save(c, photo_id, &raw, &h2, has_edits, now))
            .await?;
        if out.changed {
            if let Some(old) = &out.old_hash {
                self.render.purge_edited(&edited_key(&before.fast_key, old));
            }
            self.gc_patches(photo_id).await;
            self.spawn_edit_refresh(vec![photo_id]);
        }
        let thumb_version =
            catalog::thumb_version_for(&before.fast_key, has_edits.then_some(&hash[..]));
        Ok(EditPut {
            photo_id,
            stack: checked.raw,
            updated_at: out.updated_at,
            thumb_version,
        })
    }

    /// Resets a photo to its original and broadcasts `edits.updated`.
    pub async fn delete_edit(&self, photo_id: i64) -> Result<()> {
        let before = self
            .db
            .call(move |c| catalog::photo_ref(c, photo_id))
            .await?;
        let old = self.db.call(move |c| store::clear(c, photo_id)).await?;
        if let Some(old) = old {
            self.render
                .purge_edited(&edited_key(&before.fast_key, &old));
        }
        self.events.emit(Event::EditsUpdated {
            items: vec![EditUpdate {
                id: photo_id,
                has_edits: false,
                thumb_version: catalog::thumb_version(&before.fast_key),
            }],
        });
        Ok(())
    }

    /// `POST /api/render/preview`.
    pub async fn render_preview(self: &Arc<Self>, req: PreviewRequest) -> Result<Preview> {
        let long_edge = req.long_edge.unwrap_or(PREVIEW_DEFAULT_EDGE);
        if !(16..=PREVIEW_MAX_EDGE).contains(&long_edge) {
            return Err(CoreError::bad_request(format!(
                "long_edge must be within 16..{PREVIEW_MAX_EDGE}"
            )));
        }
        let id = req.photo_id;
        let given = req
            .stack
            .map(|s| validate_stack_for(s, id, &self.render.patches))
            .transpose()?;
        let (r, saved) = self
            .db
            .call(move |c| Ok((catalog::photo_ref(c, id)?, store::current(c, id)?)))
            .await?;
        let stack = if req.original.unwrap_or(false) {
            EditStack::default()
        } else if let Some(g) = given {
            g.stack
        } else if let Some(s) = saved {
            parse_saved(&s.stack)?
        } else {
            EditStack::default()
        };
        let out = self
            .render
            .render_async(r, stack, Some(long_edge), MaskMode::Strict)
            .await?;
        let backend = out.backend;
        let t0 = Instant::now();
        let img = out.image;
        let jpeg = tokio::task::spawn_blocking(move || encode_jpeg(&img, PREVIEW_QUALITY))
            .await
            .map_err(|e| CoreError::Internal(anyhow::anyhow!("encode task failed: {e}")))??;
        Ok(Preview {
            jpeg,
            ms: out.ms + t0.elapsed().as_millis() as u64,
            backend,
        })
    }

    async fn auto_context(&self, photo_id: i64) -> Result<AutoContext> {
        self.db
            .call(move |c| {
                let photo = catalog::get_photo(c, photo_id)?;
                let faces = crate::analysis::store::faces_of_photo(c, photo_id)?;
                Ok(AutoContext {
                    scene_type: photo.scene_type,
                    faces: faces
                        .iter()
                        .map(|f| {
                            [
                                f.bbox[0] as f32,
                                f.bbox[1] as f32,
                                f.bbox[2] as f32,
                                f.bbox[3] as f32,
                            ]
                        })
                        .collect(),
                })
            })
            .await
    }

    /// `POST /api/edits/{id}/auto`: suggested global sliders (nothing is saved).
    pub async fn auto_adjust(&self, photo_id: i64, mode: AutoMode) -> Result<Adjust> {
        let r = self
            .db
            .call(move |c| catalog::photo_ref(c, photo_id))
            .await?;
        let ctx = self.auto_context(photo_id).await?;
        self.render
            .run_gated(move |svc, _| {
                let proxy = svc.proxy(&r, 1024)?;
                Ok(ip_render::auto_adjust(&proxy, &ctx, mode))
            })
            .await
    }

    /// `POST /api/edits/sync`; returns the number of target photos processed.
    pub async fn sync_edits(self: &Arc<Self>, req: SyncRequest) -> Result<usize> {
        if req.include.is_empty() {
            return Err(CoreError::bad_request("include must not be empty"));
        }
        if req.to_ids.len() > 20_000 {
            return Err(CoreError::bad_request("too many to_ids"));
        }
        let adaptive = req.adaptive.unwrap_or(true) && req.include.contains(&SyncKind::Global);
        let from = req.from_id;
        let mut seen = HashSet::new();
        let to_ids: Vec<i64> = req
            .to_ids
            .iter()
            .copied()
            .filter(|i| *i != from && seen.insert(*i))
            .collect();
        let ids = to_ids.clone();
        let (src_ref, src_cur, targets) = self
            .db
            .call(move |c| {
                let r = catalog::photo_ref(c, from)?;
                let cur = store::current(c, from)?;
                let refs = catalog::photo_refs(c, &ids)?;
                let mut out = Vec::with_capacity(refs.len());
                for t in refs {
                    let id = t.id;
                    let cur = store::current(c, id)?;
                    let faces = face_boxes(c, id)?;
                    let people = person_set(c, id)?;
                    out.push((t, cur, faces, people));
                }
                Ok((r, cur, out))
            })
            .await?;
        let src_ops = src_cur
            .as_ref()
            .map(|c| ops_of(&c.stack))
            .unwrap_or_default();
        let include = req.include.clone();

        // What the source looks like when rendered with only the synced ops.
        let src_stats = if adaptive && !targets.is_empty() {
            let synced: Vec<Value> = sync::merge_ops(&[], &src_ops, &include)
                .into_iter()
                .filter(|o| sync::op_type(o) != Some("crop"))
                .collect();
            let src_faces = self
                .db
                .call(move |c| face_boxes(c, from))
                .await
                .unwrap_or_default();
            let sr = src_ref.clone();
            match serde_json::from_value::<EditStack>(stack_json(synced)) {
                Ok(stack) => self
                    .render
                    .run_gated(move |svc, h| {
                        let out =
                            svc.render(&sr, &stack, Some(MATCH_EDGE), MaskMode::CachedOnly, h)?;
                        Ok(sync::measure(&out.image, &src_faces))
                    })
                    .await
                    .map_err(
                        |e| tracing::warn!(error = %e, "adaptive sync: cannot render the source"),
                    )
                    .ok(),
                Err(_) => None,
            }
        } else {
            None
        };

        let mut jobs = tokio::task::JoinSet::new();
        for (t, cur, faces, people) in targets {
            let merged = sync::merge_ops_scoped(
                &cur.as_ref().map(|c| ops_of(&c.stack)).unwrap_or_default(),
                &src_ops,
                &include,
                Some(&people),
            );
            let svc = self.render.clone();
            jobs.spawn(async move {
                let id = t.id;
                let ops = match (adaptive, src_stats) {
                    (true, Some(stats)) => {
                        let m2 = merged.clone();
                        let rr = t.clone();
                        let res = svc
                            .run_gated(move |svc, h| adapt_target(svc, h, &rr, &m2, &faces, &stats))
                            .await;
                        match res {
                            Ok(ops) => ops,
                            Err(e) => {
                                tracing::warn!(photo = id, error = %e, "adaptive sync failed; copying as is");
                                merged
                            }
                        }
                    }
                    _ => merged,
                };
                (t, ops)
            });
        }
        let mut done = Vec::new();
        while let Some(j) = jobs.join_next().await {
            done.push(
                j.map_err(|e| CoreError::Internal(anyhow::anyhow!("sync task failed: {e}")))?,
            );
        }
        let processed = done.len();

        self.persist_stacks(done).await?;
        Ok(processed)
    }

    /// Saves the given op lists as the current stacks (an empty list resets the photo), drops
    /// the stale cached renders and broadcasts `edits.updated`. Returns the ids that changed.
    pub(crate) async fn persist_stacks(
        self: &Arc<Self>,
        stacks: Vec<(catalog::PhotoRef, Vec<Value>)>,
    ) -> Result<Vec<i64>> {
        let changed_ids = self.persist_stacks_quiet(stacks).await?;
        self.spawn_edit_refresh(changed_ids.clone());
        Ok(changed_ids)
    }

    /// [`Core::persist_stacks`] without the `edits.updated` refresh (the caller sends it).
    pub(crate) async fn persist_stacks_quiet(
        self: &Arc<Self>,
        stacks: Vec<(catalog::PhotoRef, Vec<Value>)>,
    ) -> Result<Vec<i64>> {
        let mut writes = Vec::new();
        for (t, ops) in stacks {
            let checked = validate_stack(stack_json(ops))?;
            writes.push((t, checked));
        }
        let results = self
            .db
            .call(move |c| {
                let mut res = Vec::new();
                for (t, ch) in writes {
                    let now = now_ms();
                    if ch.stack.ops.is_empty() {
                        let old = store::clear(c, t.id)?;
                        res.push((t, old, true));
                    } else {
                        let hash = stack_hash(&ch.raw);
                        let o = store::save(c, t.id, &ch.raw, &hash, ch.has_edits, now)?;
                        res.push((t, o.old_hash, o.changed));
                    }
                }
                Ok(res)
            })
            .await?;
        let mut changed_ids = Vec::new();
        for (t, old, changed) in results {
            if let Some(old) = old {
                self.render.purge_edited(&edited_key(&t.fast_key, &old));
            }
            if changed {
                changed_ids.push(t.id);
            }
        }
        for id in &changed_ids {
            self.gc_patches(*id).await;
        }
        Ok(changed_ids)
    }

    // ------------------------------------------------------------ presets

    pub async fn presets(&self) -> Result<Vec<PresetOut>> {
        let mut out: Vec<PresetOut> = ip_render::builtin_presets()
            .into_iter()
            .map(|(id, name, stack)| PresetOut {
                id,
                name,
                builtin: true,
                stack: serde_json::to_value(stack).unwrap_or_else(|_| empty_stack_json()),
            })
            .collect();
        let rows = self.db.call(|c| store::presets(c)).await?;
        out.extend(rows.into_iter().map(|p| PresetOut {
            id: user_preset_id(p.id),
            name: p.name,
            builtin: false,
            stack: p.stack,
        }));
        Ok(out)
    }

    pub async fn create_preset(&self, req: PresetCreate) -> Result<PresetOut> {
        let name = req.name.trim().to_string();
        if name.is_empty() || name.chars().count() > 100 {
            return Err(bad("name must be 1..100 characters"));
        }
        let checked = validate_stack(req.stack)?;
        let ops = preset_ops(&ops_of(&checked.raw));
        if ops.is_empty() {
            return Err(bad(
                "the stack has nothing a preset can hold (global, local, lut, output_sharpen)",
            ));
        }
        let stack = stack_json(ops);
        let (n, s) = (name.clone(), stack.clone());
        let id = self
            .db
            .call(move |c| store::insert_preset(c, &n, &s, now_ms()))
            .await?;
        Ok(PresetOut {
            id: user_preset_id(id),
            name,
            builtin: false,
            stack,
        })
    }

    pub async fn delete_preset(&self, id: &str) -> Result<()> {
        if ip_render::builtin_presets().iter().any(|(b, _, _)| b == id) {
            return Err(CoreError::bad_request("built-in presets cannot be deleted"));
        }
        let n: i64 = id
            .strip_prefix("user_")
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| CoreError::not_found(format!("preset {id} not found")))?;
        self.db.call(move |c| store::delete_preset(c, n)).await
    }

    // ------------------------------------------------------------ LUTs & masks

    /// `GET /api/luts`: built-in looks and imported `.cube` files.
    pub async fn luts(&self) -> Result<Vec<LutInfo>> {
        let svc = self.render.clone();
        let list = tokio::task::spawn_blocking(move || svc.luts.list())
            .await
            .map_err(|e| CoreError::Internal(anyhow::anyhow!("lut listing failed: {e}")))?;
        Ok(list
            .into_iter()
            .map(|(id, name, builtin)| LutInfo { id, name, builtin })
            .collect())
    }

    pub async fn import_lut(&self, path: String) -> Result<LutOut> {
        let svc = self.render.clone();
        let (id, name) = tokio::task::spawn_blocking(move || svc.luts.import(&path))
            .await
            .map_err(|e| CoreError::Internal(anyhow::anyhow!("lut import failed: {e}")))??;
        Ok(LutOut { id, name })
    }

    /// `GET /api/masks/{photo_id}`: the mask PNG, generated by the worker on a cache miss.
    pub async fn mask_png(
        &self,
        photo_id: i64,
        target: &str,
        person_id: Option<i64>,
    ) -> Result<Vec<u8>> {
        let target: MaskTarget = parse_target(target).ok_or_else(|| {
            CoreError::bad_request("target must be subject|background|sky|person|skin|hair|clothes")
        })?;
        if target == MaskTarget::Person && person_id.is_none() {
            return Err(CoreError::bad_request(
                "person_id is required for target=person",
            ));
        }
        let r = self
            .db
            .call(move |c| catalog::photo_ref(c, photo_id))
            .await?;
        let svc = self.render.clone();
        let handle = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || -> Result<Vec<u8>> {
            let p = svc
                .masks
                .mask_file(&r, target, person_id, &handle, true)?
                .ok_or_else(|| CoreError::Internal(anyhow::anyhow!("mask unavailable")))?;
            Ok(std::fs::read(p)?)
        })
        .await
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("mask task failed: {e}")))?
    }

    /// Renders a file that is not in the catalog (CLI `render <path>`).
    pub fn render_path_blocking(
        &self,
        path: &std::path::Path,
        stack: &EditStack,
        long_edge: Option<u32>,
    ) -> Result<service::RenderOut> {
        let format = ip_imaging::ImageFormat::from_path(path)
            .ok_or_else(|| CoreError::bad_request("unsupported image format"))?;
        let md = std::fs::metadata(path)
            .map_err(|e| CoreError::bad_request(format!("cannot read {}: {e}", path.display())))?;
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let orientation = self
            .imaging
            .read_metadata(path, format)
            .map(|m| m.orientation.clamp(1, 8))
            .unwrap_or(1);
        let r = catalog::PhotoRef {
            id: 0,
            path: path.to_path_buf(),
            file_name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            format,
            orientation,
            fast_key: ip_imaging::fast_key(path, md.len(), mtime),
            taken_at: None,
            mtime_ms: Some(mtime),
            edit_hash: None,
            width: None,
            height: None,
        };
        let handle = tokio::runtime::Handle::current();
        self.render
            .render_gated(&r, stack, long_edge, MaskMode::Strict, &handle)
    }
}

fn face_boxes(c: &rusqlite::Connection, photo_id: i64) -> Result<Vec<[f32; 4]>> {
    Ok(crate::analysis::store::faces_of_photo(c, photo_id)?
        .iter()
        .map(|f| {
            [
                f.bbox[0] as f32,
                f.bbox[1] as f32,
                f.bbox[2] as f32,
                f.bbox[3] as f32,
            ]
        })
        .collect())
}

/// Ids of the people (clustered identities) that appear in the photo.
fn person_set(c: &rusqlite::Connection, photo_id: i64) -> Result<HashSet<i64>> {
    Ok(crate::analysis::store::faces_of_photo(c, photo_id)?
        .iter()
        .filter_map(|f| f.person_id)
        .collect())
}

/// Adaptive part of `sync` for one target (runs on a blocking thread under a render slot):
/// returns the merged ops with global exposure/temp/tint tuned to match `stats`.
fn adapt_target(
    svc: &RenderService,
    handle: &tokio::runtime::Handle,
    target: &catalog::PhotoRef,
    merged: &[Value],
    faces: &[[f32; 4]],
    stats: &sync::Stats,
) -> Result<Vec<Value>> {
    let start = sync::first_global(merged);
    let eval_ops: Vec<Value> = merged
        .iter()
        .filter(|o| sync::op_type(o) != Some("crop"))
        .cloned()
        .collect();
    let best = sync::solve(stats, &start, |adj| {
        let mut ops = eval_ops.clone();
        sync::set_global_wb(&mut ops, adj);
        let stack: EditStack = serde_json::from_value(stack_json(ops))?;
        let out = svc.render(
            target,
            &stack,
            Some(MATCH_EDGE),
            MaskMode::CachedOnly,
            handle,
        )?;
        Ok(sync::measure(&out.image, faces))
    })
    .map_err(CoreError::Internal)?;
    let mut ops = merged.to_vec();
    if best != start {
        sync::set_global_wb(&mut ops, &best);
    }
    Ok(ops)
}
