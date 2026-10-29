"""The management workbench consumes the same authenticated HTTP operations."""

import mimetypes
from importlib.resources import files

from fastapi import APIRouter, HTTPException, Request
from fastapi.responses import Response
from starlette.staticfiles import StaticFiles

router = APIRouter()
mimetypes.add_type("text/javascript", ".js")
_ASSETS = StaticFiles(packages=[("nous", "resources/web")])
_HEADERS = {
    "Content-Security-Policy": "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' blob: data:; media-src 'self' blob:; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'",
    "X-Content-Type-Options": "nosniff",
    "Cache-Control": "no-cache",
    "X-Robots-Tag": "noindex, nofollow, noarchive",
}


@router.get("/", include_in_schema=False)
async def workbench() -> Response:
    content = files("nous").joinpath("resources/web/index.html").read_bytes()
    return Response(content, media_type="text/html", headers=_HEADERS)


@router.get("/assets/locales/{locale}.ftl", include_in_schema=False)
async def locale_resource(locale: str) -> Response:
    if locale not in {"en-US", "zh-CN"}:
        raise HTTPException(status_code=404, detail="Language is not available")
    content = files("nous").joinpath("resources", "locales", f"{locale}.ftl").read_bytes()
    return Response(content, media_type="text/plain", headers=_HEADERS)


@router.get("/assets/{resource_path:path}", include_in_schema=False)
async def asset(request: Request, resource_path: str) -> Response:
    response = await _ASSETS.get_response(resource_path, request.scope)
    response.headers.update(_HEADERS)
    return response
