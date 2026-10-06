"""`llm.plan`, `vlm.suggest`, `vlm.describe` (docs/api-contract-m6.md A.3).

The worker never executes anything: `llm.plan` returns tool calls that Core validates again and the
user confirms. Decoding is greedy (temperature 0) with a bounded token budget and a deadline; the
output is constrained to the expected JSON shape by the runtime's grammar engine (llguidance) and
validated again here (with one repair retry) because small models and cut-off outputs exist.
"""

from __future__ import annotations

import asyncio
import logging
import os
import threading
import time
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Any

from .. import errors
from ..errors import InvalidParams, OutOfMemory, RpcError
from ..hw import HardwareInfo
from ..imgio import PhotoRef, load_rgb, parse_photo
from ..models.manager import ModelManager
from ..models.registry import ModelSpec
from ..steps.embed import _is_oom
from .adjust import adjust_schema, clamp_adjust
from .backend import (
    GenaiHandle,
    GenerationCancelled,
    GenerationTimeout,
    TextBackend,
    VisionBackend,
    genai_available,
    vision_backend,
)
from .facts import (
    MEASURED_PROBLEMS,
    SUGGEST_SIDE,
    baseline_adjust,
    image_facts,
    measured_problems,
    measurements_text,
    merge_adjust,
    resize_long_edge,
)
from .jsonx import JsonExtractError, extract_json
from .prompts import (
    PROBLEM_TAGS,
    build_describe_prompt,
    build_plan_messages,
    build_suggest_prompt,
    build_translate_messages,
    describe_schema,
    needs_translation,
    repair_messages,
    suggest_schema,
    translate_schema,
)
from .schema import Tool, check_plan, parse_tools, plan_schema, prune_invented

log = logging.getLogger(__name__)

# Preference order per role (first suitable + installed wins). CUDA builds are fp16 + CUDA only and
# 10x faster than the CPU builds; the CPU builds run anywhere.
LLM_MODELS = ["phi-4-mini-genai-int4-cuda", "qwen3-4b-instruct-genai-int4"]
# translation (describe / reason in zh, ...): quality first, the output is only a few tokens
TRANSLATE_MODELS = ["qwen3-4b-instruct-genai-int4", "phi-4-mini-genai-int4-cuda"]
VLM_MODELS = ["phi-3.5-vision-genai-int4-cuda", "phi-3.5-vision-genai-int4-cpu"]
ROLES = {"llm": LLM_MODELS, "vlm": VLM_MODELS}

TIER_RANK = {"T0": 0, "T1": 1, "T2": 2, "T3": 3}
MAX_MESSAGE_CHARS = 2000
PLAN_MAX_TOKENS = 512
SUGGEST_MAX_TOKENS = 400
DESCRIBE_MAX_TOKENS = 256
HARD_MAX_TOKENS = 2048
PLAN_TIMEOUT_S = 60.0
VISION_TIMEOUT_S = 120.0

GENERATION_TIMEOUT = -32013
GENERATION_FAILED = -32014
ANY_TIER_ENV = "IMAGEPICKER_ASSISTANT_ANY_TIER"


def _locale(params: dict[str, Any]) -> str:
    loc = params.get("locale")
    if not isinstance(loc, str):
        ctx = params.get("context")
        loc = ctx.get("locale") if isinstance(ctx, dict) else None
    return loc if isinstance(loc, str) and loc else "en"


def _bounded(params: dict[str, Any], key: str, default: float, lo: float, hi: float) -> float:
    v = params.get(key, default)
    if isinstance(v, bool) or not isinstance(v, (int, float)):
        raise InvalidParams(f"{key} must be a number")
    return float(min(hi, max(lo, v)))


class Assistant:
    def __init__(
        self,
        manager: ModelManager,
        hw: HardwareInfo,
        text_backend: TextBackend | None = None,
        vision_backend_: VisionBackend | None = None,
        any_tier: bool | None = None,
    ):
        self.manager = manager
        self.hw = hw
        # injected generators (unit tests): no runtime, model files or tier needed
        self._text_backend = text_backend
        self._vision_backend = vision_backend_
        self.any_tier = (
            any_tier if any_tier is not None else os.environ.get(ANY_TIER_ENV, "") not in ("", "0")
        )
        # one generation at a time: the models share the GPU and the exclusive group
        self.pool = ThreadPoolExecutor(1, thread_name_prefix="llm")

    def shutdown(self) -> None:
        self.pool.shutdown(wait=False, cancel_futures=True)

    # ------------------------------------------------------------------ models / availability
    def _provider_ok(self, spec: ModelSpec) -> bool:
        prov = spec.extra.get("genai_provider", "cpu")
        return prov == "cpu" or (prov == "cuda" and self.hw.device == "cuda")

    def candidates(self, role: str) -> list[ModelSpec]:
        reg = self.manager.registry
        return [reg.get(i) for i in ROLES[role] if i in reg and self._provider_ok(reg.get(i))]

    def tier_ok(self, role: str) -> bool:
        """LLM: T2+ (T1 with a GPU); VLM: T2+ (doc 03 section 8.1)."""
        if self.any_tier:
            return True
        rank = TIER_RANK.get(self.hw.tier, 0)
        if role == "llm":
            return rank >= 2 or (rank == 1 and self.hw.device != "cpu")
        return rank >= 2

    def default_models(self) -> dict[str, list[str]]:
        """What `models.ensure` has to fetch on this machine (empty when the tier is too low)."""
        out: dict[str, list[str]] = {}
        for role in ROLES:
            c = self.candidates(role)
            out[role] = [c[0].id] if c and self.tier_ok(role) else []
        return out

    def installed(self, role: str) -> list[ModelSpec]:
        return [s for s in self.candidates(role) if self.manager.is_installed(s.id)]

    def status(self) -> dict[str, Any]:
        """Fields for `system.info` (and Core's `/api/assistant/status`)."""
        runtime = genai_available()
        out: dict[str, Any] = {"runtime": runtime}
        for role in ROLES:
            inst = self.installed(role)
            ok = bool(self._backend_for(role)) or (runtime and self.tier_ok(role) and bool(inst))
            out[f"{role}_available"] = ok
            out[f"{role}_model"] = inst[0].id if inst and ok else None
        return out

    def _backend_for(self, role: str) -> Any:
        return self._text_backend if role == "llm" else self._vision_backend

    def _unavailable(self, role: str, reason: str, message: str) -> RpcError:
        ids = self.default_models()[role] or [c.id for c in self.candidates(role)[:1]]
        return RpcError(
            errors.MODEL_UNAVAILABLE,
            message,
            "model_unavailable",
            {"models": ids, "reason": reason},
        )

    async def _ensure_ready(
        self, role: str, params: dict[str, Any], progress: Callable[[dict[str, Any]], None] | None
    ) -> ModelSpec | None:
        """Raise -32010 unless `role` can run; download on `allow_download`. None = injected backend."""
        if self._backend_for(role) is not None:
            return None
        what = "llm.plan" if role == "llm" else "vlm.suggest / vlm.describe"
        if not genai_available():
            raise self._unavailable(
                role,
                "runtime_missing",
                f"{what} needs the `llm` extra (onnxruntime-genai): "
                "uv sync --extra cuda --extra llm-cuda   (CPU: --extra cpu --extra llm)",
            )
        if not self.tier_ok(role):
            raise self._unavailable(
                role,
                "tier_too_low",
                f"{what} needs hardware tier T2+ (this machine is {self.hw.tier}); "
                f"set {ANY_TIER_ENV}=1 to override",
            )
        requested = params.get("model")
        cands = self.candidates(role)
        if requested is not None:
            cands = [c for c in cands if c.id == requested]
            if not cands:
                raise InvalidParams(
                    f"model {requested!r} is not an assistant {role} model usable on this machine"
                )
        inst = [c for c in cands if self.manager.is_installed(c.id)]
        if inst:
            return inst[0]
        if not cands:
            raise self._unavailable(role, "no_candidate", f"no {role} model fits this machine")
        mid = cands[0].id
        if not params.get("allow_download"):
            raise self._unavailable(role, "not_installed", f"model {mid!r} is not installed")
        loop = asyncio.get_running_loop()
        notify = progress or (lambda _p: None)

        def on_progress(e: dict[str, Any]) -> None:
            loop.call_soon_threadsafe(notify, {"kind": "model.download", "done": e["bytes"], **e})

        try:
            await loop.run_in_executor(self.pool, lambda: self.manager.ensure(mid, on_progress))
        except Exception as e:  # noqa: BLE001
            if isinstance(e, RpcError):
                raise
            raise errors.DownloadFailed(f"{mid}: {e}") from e
        return cands[0]

    # ------------------------------------------------------------------ generation plumbing
    def _lease(self, role: str, spec: ModelSpec | None):
        """Context manager -> (backend, model_id)."""
        import contextlib

        @contextlib.contextmanager
        def injected():
            yield self._backend_for(role), "injected"

        if spec is None:
            return injected()

        vision = role == "vlm"
        order = [spec] + [s for s in self.installed(role) if s.id != spec.id]

        @contextlib.contextmanager
        def leased():
            last: Exception | None = None
            for s in order:
                # CPU builds also run through the CUDA EP (prefill on the GPU); CUDA builds need it
                prov = "cuda" if self.hw.device == "cuda" else "cpu"
                cost = None

                def loader(sp: ModelSpec, path: Path, _p: str = prov) -> GenaiHandle:
                    return GenaiHandle(Path(path).parent, _p, vision, sp.id)

                try:
                    cm = self.manager.lease(s.id, loader, cost)
                    handle = cm.__enter__()
                except OutOfMemory:
                    raise
                except Exception as e:  # noqa: BLE001
                    if _is_oom(e):
                        raise OutOfMemory(str(e)) from e
                    log.warning("loading %s failed (%s); trying the next model", s.id, e)
                    last = e
                    continue
                try:
                    yield (vision_backend(handle) if vision else handle), s.id
                finally:
                    cm.__exit__(None, None, None)
                return
            raise RpcError(
                GENERATION_FAILED, f"could not load an assistant model: {last}", "generation_failed"
            ) from last

        return leased()

    def _guard(self, fn: Callable[[], Any], cancel: threading.Event) -> Any:
        try:
            return fn()
        except GenerationTimeout as e:
            raise RpcError(GENERATION_TIMEOUT, str(e), "timeout") from e
        except GenerationCancelled:
            raise RpcError(errors.CANCELLED, "request cancelled", "cancelled") from None
        except RpcError:
            raise
        except Exception as e:  # noqa: BLE001
            if _is_oom(e):
                raise OutOfMemory(str(e)) from e
            log.exception("assistant generation failed")
            raise RpcError(GENERATION_FAILED, f"generation failed: {e}", "generation_failed") from e

    async def _run(self, role: str, spec: ModelSpec | None, work: Callable[[Any, str], Any]) -> Any:
        loop = asyncio.get_running_loop()
        cancel = threading.Event()

        def job() -> Any:
            def body() -> Any:
                with self._lease(role, spec) as (backend, model_id):
                    return work(backend, model_id)

            return self._guard(body, cancel)

        try:
            return await loop.run_in_executor(self.pool, job)
        except asyncio.CancelledError:
            cancel.set()
            raise

    # ------------------------------------------------------------------ llm.plan
    @staticmethod
    def _parse_plan(raw: str, tools: list[Tool]) -> tuple[str, list[dict[str, Any]], list[str]]:
        try:
            obj = extract_json(raw)
        except JsonExtractError as e:
            return "", [], [f"output is not valid JSON ({e})"]
        return check_plan(obj, tools)

    def _plan_sync(
        self, backend: TextBackend, model_id: str, req: dict[str, Any]
    ) -> dict[str, Any]:
        t0 = time.monotonic()
        deadline = t0 + req["timeout"]
        msgs = build_plan_messages(req["message"], req["tools"], req["context"], req["locale"])
        schema = plan_schema(req["tools"])
        raw = backend.generate(msgs, schema, req["max_tokens"], deadline)
        reply, calls, problems = self._parse_plan(raw, req["tools"])
        repaired = False
        if problems:
            repaired = True
            try:
                raw2 = backend.generate(
                    repair_messages(msgs, raw, problems), schema, req["max_tokens"], deadline
                )
            except GenerationTimeout:
                if not (reply or calls):
                    raise
            else:
                reply2, calls2, problems2 = self._parse_plan(raw2, req["tools"])
                if len(problems2) <= len(problems) and (calls2 or reply2 or not (calls or reply)):
                    reply, calls, problems = reply2, calls2, problems2
        calls, notes = prune_invented(calls, req["tools"], req["message"], req["context"])
        problems = [*problems, *notes]
        if not (reply or calls):
            raise RpcError(
                GENERATION_FAILED,
                "the model produced no usable plan: " + "; ".join(problems[:4]),
                "generation_failed",
            )
        return {
            "reply": reply,
            "calls": calls,
            "model": model_id,
            "repaired": repaired,
            "warnings": problems,
            "latency_ms": round((time.monotonic() - t0) * 1000),
        }

    async def plan(
        self, params: Any, progress: Callable[[dict[str, Any]], None] | None = None
    ) -> dict[str, Any]:
        if not isinstance(params, dict):
            raise InvalidParams("params must be an object")
        message = params.get("message")
        if not isinstance(message, str) or not message.strip():
            raise InvalidParams("message must be a non-empty string")
        ctx = params.get("context", {})
        if ctx is None:
            ctx = {}
        if not isinstance(ctx, dict):
            raise InvalidParams("context must be an object")
        req = {
            "message": message.strip()[:MAX_MESSAGE_CHARS],
            "tools": parse_tools(params.get("tools")),
            "context": ctx,
            "locale": _locale(params),
            "max_tokens": int(_bounded(params, "max_tokens", PLAN_MAX_TOKENS, 32, HARD_MAX_TOKENS)),
            "timeout": _bounded(params, "timeout_s", PLAN_TIMEOUT_S, 1, 600),
        }
        spec = await self._ensure_ready("llm", params, progress)
        return await self._run("llm", spec, lambda b, m: self._plan_sync(b, m, req))

    # ------------------------------------------------------------------ vlm.suggest / describe
    def _load_image(self, photo: PhotoRef):
        return resize_long_edge(load_rgb(photo, 2 * SUGGEST_SIDE), SUGGEST_SIDE)

    def _suggest_sync(
        self, backend: VisionBackend, model_id: str, req: dict[str, Any]
    ) -> dict[str, Any]:
        t0 = time.monotonic()
        deadline = t0 + req["timeout"]
        rgb = self._load_image(req["photo"])
        facts = image_facts(rgb)
        baseline = baseline_adjust(facts)
        prompt = build_suggest_prompt(measurements_text(facts, req["context"]), "en", baseline)
        schema = suggest_schema(adjust_schema())
        obj = None
        err = ""
        for attempt in range(2):
            p = prompt if attempt == 0 else prompt + "\n\nOutput the JSON object only."
            raw = backend.generate(rgb, p, schema, req["max_tokens"], deadline)
            try:
                obj = extract_json(raw)
                break
            except JsonExtractError as e:
                err = str(e)
        if obj is None:
            raise RpcError(
                GENERATION_FAILED, f"no valid JSON from the model ({err})", "generation_failed"
            )
        problems = measured_problems(facts)
        for p_ in obj.get("problems") if isinstance(obj.get("problems"), list) else []:
            if (
                isinstance(p_, str)
                and p_ in PROBLEM_TAGS
                and p_ not in problems
                and p_ not in MEASURED_PROBLEMS
            ):
                problems.append(p_)
        adjust = clamp_adjust(merge_adjust(baseline, clamp_adjust(obj.get("adjust"))))
        reason = obj.get("reason")
        return {
            "problems": problems[:5],
            "adjust": adjust,
            "reason": reason.strip() if isinstance(reason, str) else "",
            "model": model_id,
            "latency_ms": round((time.monotonic() - t0) * 1000),
        }

    def _describe_sync(
        self, backend: VisionBackend, model_id: str, req: dict[str, Any]
    ) -> dict[str, Any]:
        t0 = time.monotonic()
        deadline = t0 + req["timeout"]
        rgb = self._load_image(req["photo"])
        prompt = build_describe_prompt()
        schema = describe_schema()
        obj = None
        err = ""
        for attempt in range(2):
            p = prompt if attempt == 0 else prompt + " Output the JSON object only."
            raw = backend.generate(rgb, p, schema, req["max_tokens"], deadline)
            try:
                obj = extract_json(raw)
                break
            except JsonExtractError as e:
                err = str(e)
        if obj is None:
            raise RpcError(
                GENERATION_FAILED, f"no valid JSON from the model ({err})", "generation_failed"
            )
        cap = obj.get("caption")
        kws: list[str] = []
        for k in obj.get("keywords") if isinstance(obj.get("keywords"), list) else []:
            if isinstance(k, str) and k.strip() and k.strip() not in kws:
                kws.append(k.strip())
        return {
            "caption": cap.strip() if isinstance(cap, str) else "",
            "keywords": kws[:8],
            "model": model_id,
            "latency_ms": round((time.monotonic() - t0) * 1000),
        }

    def _vision_req(self, params: Any, default_tokens: int) -> dict[str, Any]:
        if not isinstance(params, dict):
            raise InvalidParams("params must be an object")
        ctx = params.get("context", {})
        if ctx is None:
            ctx = {}
        if not isinstance(ctx, dict):
            raise InvalidParams("context must be an object")
        return {
            "photo": parse_photo(params.get("photo")),
            "context": ctx,
            "locale": _locale(params),
            "max_tokens": int(_bounded(params, "max_tokens", default_tokens, 32, HARD_MAX_TOKENS)),
            "timeout": _bounded(params, "timeout_s", VISION_TIMEOUT_S, 1, 900),
        }

    def _translator(self) -> tuple[bool, ModelSpec | None]:
        """(available, spec) of the model that translates results; spec None = injected backend."""
        if self._text_backend is not None:
            return True, None
        if not genai_available() or not self.tier_ok("llm"):
            return False, None
        inst = {s.id: s for s in self.installed("llm")}
        for mid in TRANSLATE_MODELS:
            if mid in inst:
                return True, inst[mid]
        return False, None

    def _translate_sync(
        self, backend: TextBackend, payload: dict[str, Any], locale: str, timeout: float
    ) -> dict[str, Any]:
        msgs = build_translate_messages(payload, locale)
        raw = backend.generate(
            msgs, translate_schema(payload, locale), 400, time.monotonic() + timeout
        )
        obj = extract_json(raw)
        out: dict[str, Any] = {}
        for k, v in payload.items():
            got = obj.get(k)
            if isinstance(v, list):
                if not isinstance(got, list):
                    raise JsonExtractError(f"{k} is not a list")
                out[k] = [str(x).strip() for x in got if str(x).strip()][:8]
            else:
                if not isinstance(got, str) or not got.strip():
                    raise JsonExtractError(f"{k} is not a string")
                out[k] = got.strip()
        return out

    async def _localize(
        self, payload: dict[str, Any], locale: str, limit_s: float
    ) -> tuple[dict[str, Any], str]:
        """Translate `payload` into `locale` with the text LLM; falls back to English (and says so)."""
        if not needs_translation(locale):
            return payload, "en"
        ok, spec = self._translator()
        if not ok:
            log.info("no LLM installed to translate to %s: returning English", locale)
            return payload, "en"
        try:
            out = await self._run(
                "llm", spec, lambda b, _m: self._translate_sync(b, payload, locale, limit_s)
            )
        except RpcError as e:
            if e.code == errors.CANCELLED:
                raise
            log.warning("translation to %s failed (%s): returning English", locale, e.message)
            return payload, "en"
        except JsonExtractError as e:
            log.warning("translation to %s unusable (%s): returning English", locale, e)
            return payload, "en"
        return out, locale

    async def suggest(
        self, params: Any, progress: Callable[[dict[str, Any]], None] | None = None
    ) -> dict[str, Any]:
        req = self._vision_req(params, SUGGEST_MAX_TOKENS)
        spec = await self._ensure_ready("vlm", params, progress)
        res = await self._run("vlm", spec, lambda b, m: self._suggest_sync(b, m, req))
        res["locale"] = "en"
        if res["reason"]:
            out, res["locale"] = await self._localize(
                {"reason": res["reason"]}, req["locale"], req["timeout"]
            )
            res["reason"] = out["reason"]
        return res

    async def describe(
        self, params: Any, progress: Callable[[dict[str, Any]], None] | None = None
    ) -> dict[str, Any]:
        req = self._vision_req(params, DESCRIBE_MAX_TOKENS)
        spec = await self._ensure_ready("vlm", params, progress)
        res = await self._run("vlm", spec, lambda b, m: self._describe_sync(b, m, req))
        payload = {"caption": res["caption"], "keywords": res["keywords"]}
        out, res["locale"] = await self._localize(payload, req["locale"], req["timeout"])
        res.update(out)
        return res
