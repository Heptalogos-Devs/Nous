"""Provider failures retain actionable context without collecting request credentials."""

import json

import httpx2
from openai import RateLimitError
from pydantic_ai.exceptions import ModelHTTPError

from nous.models.policy import attempt_scope, error_report, model_error
from nous.presentation import render_error


def test_rate_limit_reports_service_details_and_retry_after() -> None:
    request = httpx2.Request(
        "POST",
        "https://gateway.invalid/v1/embeddings?credential=secret-query",
        headers={"Authorization": "Bearer secret-token"},
    )
    body = {
        "error": {
            "message": "Requests are too frequent; retry later.",
            "type": "rate_limit",
            "code": "provider_limit",
        },
        "unrelated": "secret-body",
    }
    response = httpx2.Response(
        429, request=request, headers={"retry-after": "3", "x-request-id": "request-123"}, json=body
    )
    error = RateLimitError("Request failed", response=response, body=body)
    route = {
        "provider": "gateway",
        "model_id": "embedding",
        "protocol": "openai-chat-completions",
        "endpoint_path": "/v1/embeddings",
    }
    with attempt_scope(1, route, "embedding") as scope:
        scope.complete(**error_report(error, next_action="retry_same_route"))
        mapped = model_error(error, 1)
        report = dict(scope.report)
    assert mapped.code == "model.request_failed"
    assert "Requests are too frequent" in mapped.message
    assert mapped.retryable and mapped.retry_after == 3
    assert mapped.details["status_code"] == 429
    assert mapped.details["request_id"] == "request-123"
    assert mapped.details["endpoint_path"] == "/v1/embeddings"
    assert mapped.details["route"] == route
    assert mapped.details["provider_error"] == body["error"]
    assert report["provider_error"] == body["error"]
    assert report["retry_after"] == 3
    assert report["provider_request_id"] == "request-123"
    assert report["endpoint_path"] == "/v1/embeddings"
    serialized = json.dumps({"error": mapped.to_dict(), "attempt": report})
    assert "secret-token" not in serialized
    assert "secret-query" not in serialized
    assert "secret-body" not in serialized
    mapped.operation_id = "operation-123"
    mapped.details["completed_refs"] = [{"identifier": "res(completed)"}]
    mapped.details["attempts"] = [{"number": 1, "extra": "verbose diagnostic"}]
    display = render_error(mapped, "zh-CN")
    assert "请求" in display and "429" in display and "3.0 秒后" in display
    assert "request-123" in display and "operation-123" in display and "res(completed)" in display
    assert "verbose diagnostic" not in display
    echoed = RateLimitError(
        "Request failed",
        response=response,
        body={"error": {"message": "credential secret-token and secret-query were rejected"}},
    )
    safe = json.dumps(model_error(echoed, 1).to_dict())
    assert "secret-token" not in safe and "secret-query" not in safe
    assert "[redacted]" in safe


def test_model_sdk_error_uses_declared_route_and_provider_fields() -> None:
    error = ModelHTTPError(
        429,
        "configured-model",
        body="Requests are too frequent.",
        headers={"x-request-id": "sdk-request"},
        provider_error_code="rate_limited",
        provider_error_type="throttled",
        retry_after=5,
    )
    with attempt_scope(
        2,
        {
            "provider": "gateway",
            "model_id": "text",
            "protocol": "openai-chat-completions",
            "endpoint_path": "/v1/chat/completions",
        },
        "summary",
    ) as scope:
        scope.complete(**error_report(error))
        mapped = model_error(error, 2)
        report = dict(scope.report)
    assert mapped.details["provider_error"] == {
        "message": "Requests are too frequent.",
        "code": "rate_limited",
        "type": "throttled",
    }
    assert mapped.details["endpoint_path"] == "/v1/chat/completions"
    assert mapped.details["request_id"] == "sdk-request"
    assert mapped.retry_after == 5
    assert report["provider_error"] == mapped.details["provider_error"]
