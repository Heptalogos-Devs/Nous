"""FastAPI host for the same operations exposed by CLI and MCP."""

from collections.abc import AsyncGenerator
from contextlib import asynccontextmanager

from fastapi import FastAPI, Request
from fastapi.responses import JSONResponse
from nous_wave.contracts import DomainError

from nous.bootstrap import application
from nous.platform.configuration import Settings
from nous.platform.errors import NousError
from nous.platform.persistence import OperationConflict

from .amemorix import router as amemorix
from .credentials import router as credentials
from .domain import router as domain
from .evolution import router as evolution
from .knowledge import router as knowledge
from .management import router as management
from .media import router as media
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

    @app.exception_handler(DomainError)
    async def domain_error(request: Request, error: DomainError) -> JSONResponse:
        code = error.code
        status = (
            403 if "forbidden" in code or "scope" in code else 404 if "not_found" in code else 400
        )
        return JSONResponse({"error": {"code": code, "message": str(error)}}, status_code=status)

    @app.exception_handler(NousError)
    async def service_error(request: Request, error: NousError) -> JSONResponse:
        return JSONResponse(
            {"error": {"code": error.code, "message": str(error)}}, status_code=error.status
        )

    @app.exception_handler(OperationConflict)
    async def conflict(request: Request, error: OperationConflict) -> JSONResponse:
        return JSONResponse({"error": {"code": "Conflict", "message": str(error)}}, status_code=409)

    @app.exception_handler(ValueError)
    async def invalid_input(request: Request, error: ValueError) -> JSONResponse:
        return JSONResponse(
            {"error": {"code": "InvalidArgument", "message": str(error)}}, status_code=400
        )

    @app.exception_handler(LookupError)
    async def missing(request: Request, error: LookupError) -> JSONResponse:
        return JSONResponse({"error": {"code": "NotFound", "message": str(error)}}, status_code=404)

    return app
