"""Present completed imports with reusable source and cognition references."""

from nous_sdk.contracts.text_import import TextImportResult, TextImportSubmission
from nous_sdk.namespace import NamespaceClient
from nous_sdk.types import JobDetail, JsonObject
from pydantic import TypeAdapter


async def import_result(
    namespace: NamespaceClient, submission: TextImportSubmission, *, wait: bool, raw: bool = False
) -> JsonObject | TextImportResult:
    job_ref = "job:" + submission.job_id
    if not wait:
        return {"ref": job_ref, "status": "queued"}
    _ = await namespace.jobs.wait(submission.job_id)
    result = await namespace.client.text_import_result(namespace.namespace_id, submission.request_id)
    if raw:
        return result
    materials: list[JsonObject] = []
    seen: set[str] = set()
    if result.admission.catalog is not None:
        for unit in result.admission.catalog.units:
            if unit.material_id in seen:
                continue
            seen.add(unit.material_id)
            material = await namespace.materials.get(unit.material_id)
            materials.append(
                {
                    "ref": (material.addresses.object or material.addresses.exact)
                    if material.addresses else f"material:{material.id}",
                    "name": material.filename,
                    "media_type": material.media_type,
                }
            )
    cognition = tuple(
        dict.fromkeys(
            ref.exact()
            for output in (*result.admission.outputs, *result.formations)
            for ref in output.cognition
        )
    )
    representations = tuple(
        dict.fromkeys(
            ref.media().exact()
            for output in (*result.admission.outputs, *result.formations)
            for ref in output.representations
        )
    )
    items: list[JsonObject] = []
    for ref in (*cognition, *representations):
        resolved = await namespace.resolve_reference(ref)
        items.append(
            {
                "ref": resolved.addresses.exact if resolved.addresses else ref,
                "kind": resolved.reference.reference.kind,
            }
        )
    return TypeAdapter(JsonObject).validate_python(
        {"ref": job_ref, "status": "completed", "materials": materials, "items": items}
    )


async def job_result(namespace: NamespaceClient, detail: JobDetail, *, raw: bool = False) -> object:
    job = detail.job
    if job.status != "SUCCESS" or job.name != "nous.imports.text":
        return detail
    if not isinstance(job.output, dict):
        raise ValueError("Completed import has no result")
    request_id = job.output.get("request_id")
    completion = job.output.get("completion")
    if not isinstance(request_id, str) or not isinstance(completion, str) or completion not in {"accepted", "formed"}:
        raise ValueError("Completed import has an invalid result")
    return await import_result(
        namespace,
        TextImportSubmission.model_validate(
            {"request_id": request_id, "job_id": job.workflow_id, "completion": completion}
        ),
        wait=True,
        raw=raw,
    )
