"""The management workbench consumes the same authenticated HTTP operations."""

from importlib.resources import files

from fastapi import APIRouter, HTTPException
from fastapi.responses import Response

router = APIRouter()
_ASSETS = {
    "workbench.css": "text/css",
    "responsive.css": "text/css",
    "workbench.js": "text/javascript",
    "client.js": "text/javascript",
    "knowledge.js": "text/javascript",
    "management.js": "text/javascript",
}
_HEADERS = {
    "Content-Security-Policy": "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' blob: data:; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'",
    "X-Content-Type-Options": "nosniff",
    "Cache-Control": "no-cache",
    "X-Robots-Tag": "noindex, nofollow, noarchive",
}


@router.get("/", include_in_schema=False)
async def workbench() -> Response:
    content = files("nous").joinpath("resources/web/index.html").read_bytes()
    return Response(content, media_type="text/html", headers=_HEADERS)


@router.get("/assets/{name}", include_in_schema=False)
async def asset(name: str) -> Response:
    if name not in _ASSETS:
        raise HTTPException(status_code=404, detail="Asset does not exist")
    content = files("nous").joinpath(f"resources/web/{name}").read_bytes()
    return Response(content, media_type=_ASSETS[name], headers=_HEADERS)
