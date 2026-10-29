"""FastAPI host for the same operations exposed by CLI and MCP."""

from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager

from fastapi import FastAPI, Request
from fastapi.exceptions import RequestValidationError
from fastapi.responses import JSONResponse
from jinja2 import TemplateError
from nous_wave.contracts import DomainError
from starlette.exceptions import HTTPException

from nous.bootstrap import application
from nous.platform.configuration import Settings
from nous.platform.errors import NousError
from nous.platform.persistence import OperationConflict

from .amemorix import router as amemorix
from .credentials import router as credentials
from .domain import router as domain
from .evolution import router as evolution
from .knowledge import router as knowledge
from .maintenance import router as maintenance
from .management import router as management
from .media import router as media
from .observability import instrument_app
from .sdk_routes import router as sdk_routes
from .web import router as web


def create_app(settings: Settings | None = None) -> FastAPI:
    settings = settings or Settings()

    @asynccontextmanager
    async def lifespan(app: FastAPI) -> AsyncGenerator[None]:
        async with application(settings) as service:
            app.state.application = service
            yield

    app = FastAPI(title="Nous Wave", version="0.1.0", lifespan=lifespan)
    app.include_router(domain)
    app.include_router(management)
    app.include_router(knowledge)
    app.include_router(media)
    app.include_router(evolution)
    app.include_router(sdk_routes)
    app.include_router(credentials)
    app.include_router(web)
    app.include_router(amemorix)
    app.include_router(maintenance)

    @app.exception_handler(DomainError)
    async def domain_error(request: Request, error: DomainError) -> JSONResponse:
        code = error.code
        status = (
            401
            if code == "credential_invalid"
            else 503
            if code in {"model_unavailable", "capability_unavailable"}
            else 403
            if code.endswith("_forbidden")
            else 404
            if "not_found" in code or code == "source_unavailable"
            else 409
            if "conflict" in code
            or code in {"operation_result_purged", "model_invocation_unavailable"}
            else 400
        )
        if code == "model_unavailable":
            status = 503
        return JSONResponse(
            {"error": {"code": code, "message": str(error), "context": error.details}},
            status_code=status,
            headers={"WWW-Authenticate": "Bearer"} if status == 401 else None,
        )

    @app.exception_handler(HTTPException)
    async def http_error(request: Request, error: HTTPException) -> JSONResponse:
        code = {
            401: "credential_invalid",
            403: "Forbidden",
            404: "NotFound",
            409: "Conflict",
            429: "ResourceExhausted",
            503: "Unavailable",
        }.get(error.status_code, "InvalidArgument")
        return JSONResponse(
            {"error": {"code": code, "message": str(error.detail)}},
            status_code=error.status_code,
            headers=error.headers,
        )

    @app.exception_handler(RequestValidationError)
    async def invalid_request(request: Request, error: RequestValidationError) -> JSONResponse:
        return JSONResponse(
            {
                "error": {
                    "code": "InvalidArgument",
                    "message": "Invalid request",
                    "context": {
                        "fields": [
                            {
                                "location": [str(part) for part in item["loc"]],
                                "message": item["msg"],
                            }
                            for item in error.errors()
                        ]
                    },
                }
            },
            status_code=422,
        )

    @app.exception_handler(NousError)
    async def service_error(request: Request, error: NousError) -> JSONResponse:
        return JSONResponse(
            {"error": {"code": error.code, "message": str(error), "context": error.context}},
            status_code=error.status,
        )

    @app.exception_handler(OperationConflict)
    async def conflict(request: Request, error: OperationConflict) -> JSONResponse:
        return JSONResponse({"error": {"code": "Conflict", "message": str(error)}}, status_code=409)

    @app.exception_handler(OSError)
    async def storage_error(request: Request, error: OSError) -> JSONResponse:
        return JSONResponse(
            {
                "error": {
                    "code": "Unavailable",
                    "message": "The service could not complete its I/O operation",
                    "context": {"error_type": type(error).__name__},
                }
            },
            status_code=503,
        )

    @app.exception_handler(ValueError)
    async def invalid_input(request: Request, error: ValueError) -> JSONResponse:
        return JSONResponse(
            {"error": {"code": "InvalidArgument", "message": str(error)}}, status_code=400
        )

    @app.exception_handler(TemplateError)
    async def invalid_template(request: Request, error: TemplateError) -> JSONResponse:
        return JSONResponse(
            {"error": {"code": "InvalidTemplate", "message": str(error)}}, status_code=400
        )

    @app.exception_handler(LookupError)
    async def missing(request: Request, error: LookupError) -> JSONResponse:
        return JSONResponse({"error": {"code": "NotFound", "message": str(error)}}, status_code=404)

    @app.exception_handler(Exception)
    async def internal_error(request: Request, error: Exception) -> JSONResponse:
        return JSONResponse(
            {
                "error": {
                    "code": "Internal",
                    "message": "The service could not complete the operation",
                    "context": {"error_type": type(error).__name__},
                }
            },
            status_code=500,
        )

    instrument_app(app)
    return app
