"""Preserve an actual context feedback event as a durable material contact."""

from __future__ import annotations

from nous_sdk.contracts import (
    CallContext,
    ContactMaterial,
    DomainError,
    ImportDocument,
    MediaReference,
)
from nous_sdk.contracts.common import utc_now
from sqlalchemy import select

from nous.context.service import ContextService
from nous.imports.service import ImportService
from nous.material.service import MaterialService
from nous.platform.content import ContentStore
from nous.platform.persistence import Authority


class ContextContactService:
    def __init__(
        self,
        authority: Authority,
        contexts: ContextService,
        material: MaterialService,
        content: ContentStore,
        imports: ImportService,
    ) -> None:
        self.authority = authority
        self.contexts = contexts
        self.material = material
        self.content = content
        self.imports = imports

    async def capture_context_event(
        self, context: dict[str, object], event_id: str
    ) -> dict[str, object]:
        from nous_sdk.contracts.targets import CognitiveTarget, ContextTarget
        from pydantic import TypeAdapter

        from nous.context.models import contexts, feedback

        ctx = CallContext.model_validate(context).model_copy(update={"clock": utc_now()})
        async with self.authority.read(ctx) as view:
            event = await view.connection.scalar(
                select(feedback.c.record)
                .join(contexts, contexts.c.id == feedback.c.context_id)
                .where(
                    feedback.c.id == event_id,
                    contexts.c.namespace_id == ctx.namespace_id,
                )
            )
            if event is None or event.get("caller_id") != ctx.caller_id:
                raise DomainError("feedback_not_found", "Context feedback is unavailable")
            description = str(event.get("description", "")).strip()
            if (
                not description
                or not event.get("subject_view")
                or event.get("kind") == "model_contact"
            ):
                return {"skipped": True}
            target = TypeAdapter(ContextTarget).validate_python(event["target"])
            await self.contexts.require_targets_in(view, ctx, (target,))
        document = ImportDocument(
            text=description,
            filename="feedback.txt",
            source_type="context_feedback",
            origin={
                "conversation_ref": str(event.get("session_id") or event["context_id"]),
                "caller_id": ctx.caller_id,
            },
        )
        lease = await self.content.publish_bytes(
            description.encode("utf-8"), document.media_type
        )
        operation_id = "feedback-contact:" + event_id
        try:
            async with self.authority.write(
                ctx,
                operation_id,
                operation_kind="feedback.contact_input",
                scope=ctx.namespace_id,
                payload={"event_id": event_id},
            ) as tx:
                await self.contexts.require_targets_in(tx, ctx, (target,))
                if tx.replayed:
                    restored = await self.material.restore_input_in(
                        tx,
                        ctx,
                        operation_id=operation_id,
                        operation_kind="feedback.contact_input",
                        scope=ctx.namespace_id,
                    )
                    effective = restored.context
                    source = MediaReference.model_validate(tx.replay_result)
                else:
                    admission = await self.material.admit_text_in(tx, ctx, document, lease)
                    effective = admission.context
                    source = MediaReference(
                        kind="material",
                        id=admission.material["id"],
                        observation_id=admission.event["id"],
                    )
                    tx.result(source)
            receipt = await self.imports.contact(
                effective,
                ContactMaterial(
                    operation_id=operation_id + ":contact",
                    source=source,
                    external_event_id=operation_id,
                    origin=document.origin,
                    related=(target.reference,) if isinstance(target, CognitiveTarget) else (),
                ),
            )
            return receipt.model_dump(mode="json")
        finally:
            await self.content.release(lease)

