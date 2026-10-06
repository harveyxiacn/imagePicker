"""Structured errors carried over JSON-RPC (`error.data.kind` is the stable machine-readable tag)."""

from __future__ import annotations

from typing import Any

# JSON-RPC 2.0 reserved
PARSE_ERROR = -32700
INVALID_REQUEST = -32600
METHOD_NOT_FOUND = -32601
INVALID_PARAMS = -32602
INTERNAL_ERROR = -32603
# Application range (-32000..-32099)
UNAUTHORIZED = -32001
MODEL_UNAVAILABLE = -32010  # model not installed and downloads not allowed
DOWNLOAD_FAILED = -32011
OUT_OF_MEMORY = -32012
DECODE_FAILED = -32020  # per-item only (never fails a whole batch)
STEP_FAILED = -32021  # per-item only
CANCELLED = -32800  # same code LSP uses for RequestCancelled


class RpcError(Exception):
    def __init__(self, code: int, message: str, kind: str | None = None, data: Any = None):
        super().__init__(message)
        self.code = code
        self.message = message
        self.kind = kind
        self.extra = data

    def to_obj(self) -> dict[str, Any]:
        data: dict[str, Any] = {}
        if self.kind:
            data["kind"] = self.kind
        if self.extra is not None:
            data["detail"] = self.extra
        obj: dict[str, Any] = {"code": self.code, "message": self.message}
        if data:
            obj["data"] = data
        return obj


class InvalidParams(RpcError):
    def __init__(self, message: str, data: Any = None):
        super().__init__(INVALID_PARAMS, message, "invalid_params", data)


class ModelUnavailable(RpcError):
    def __init__(self, message: str, model_ids: list[str] | None = None):
        super().__init__(
            MODEL_UNAVAILABLE, message, "model_unavailable", {"models": model_ids or []}
        )


class DownloadFailed(RpcError):
    def __init__(self, message: str, data: Any = None):
        super().__init__(DOWNLOAD_FAILED, message, "download_failed", data)


class OutOfMemory(RpcError):
    def __init__(self, message: str):
        super().__init__(OUT_OF_MEMORY, message, "out_of_memory")
