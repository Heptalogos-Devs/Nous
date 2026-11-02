"""Persistent host history and the single actual model-input construction owner."""

from __future__ import annotations

from .selection import SelectionRequest
from nous_sdk.contracts.history import WorkingHistoryGroup
import json
from dataclasses import asdict
from hashlib import sha256
from uuid import uuid4
import sqlalchemy as sa
from pydantic import JsonValue, TypeAdapter
from nous_sdk.contracts import BlobDocument, CallContext, DomainError, MediaReference
from nous_sdk.contracts.context import (
    ChangeContextSession,
    ChangeResidents,
    ChangeWorkContext,
    ContextProjection,
    ContextResidentAdmission,
    ContextSession,
    WorkContext,
    MediaTarget,
)
from nous_sdk.contracts.derivation import (
    CreateRepresentation,
    DerivationInput,
    DerivationQuality,
    TextRepresentation,
)
from nous_sdk.contracts.imports import FormObservation
from nous_sdk.contracts.runtime import (
    HostEvent,
    RuntimeOpen,
    RuntimeBindingInfo,
    RuntimePrepare,
    RuntimePreparation,
    RuntimeSent,
    RuntimeComplete,
    RuntimeResult,
    RuntimeTool,
    RuntimeBlock,
    RuntimeRead,
)
from nous.identity.access import require_namespace_in
from nous.platform.persistence import metadata
from nous.platform.persistence.authority import canonical_json
from nous.platform.concurrency import owned_thread
from .lifecycle import session_key
from .jobs import DurableContexts
from .projection import Projection
from .models import sessions, works
from . import compilation
from .host_input import (
    HostBlock,
    block_identity,
    clean_messages,
    message_role,
    message_text,
    fingerprint,
    response_message,
    completed_prefix,
    construct,
)

events = sa.Table(
    "runtime_events",
    metadata,
    sa.Column("namespace_id", sa.String, primary_key=True),
    sa.Column("caller_id", sa.String, primary_key=True),
    sa.Column("session_id", sa.String, nullable=False),
    sa.Column("event_id", sa.String, primary_key=True),
    sa.Column("record", sa.JSON, nullable=False),
    sa.Column("commit_seq", sa.Integer, nullable=False),
)
calls = sa.Table(
    "runtime_calls",
    metadata,
    sa.Column("id", sa.String, primary_key=True),
    sa.Column("namespace_id", sa.String, nullable=False, index=True),
    sa.Column("caller_id", sa.String, nullable=False),
    sa.Column("session_id", sa.String, nullable=False),
    sa.Column("instance_id", sa.String, nullable=False),
    sa.Column("invocation_id", sa.String, nullable=False),
    sa.Column("record", sa.JSON, nullable=False),
    sa.Column("commit_seq", sa.Integer, nullable=False),
    sa.UniqueConstraint("namespace_id", "caller_id", "session_id", "instance_id", "invocation_id"),
)
source_keys = sa.Table(
    "runtime_source_keys",
    metadata,
    sa.Column("namespace_id", sa.String, primary_key=True),
    sa.Column("session_id", sa.String, primary_key=True),
    sa.Column("number", sa.Integer, primary_key=True),
    sa.Column("target", sa.Text, nullable=False),
    sa.UniqueConstraint("namespace_id", "session_id", "target"),
)
_OBJECT = TypeAdapter(dict[str, JsonValue])


class ContextRuntime(Projection, DurableContexts):
    def __init__(
        self,
        authority,
        cognition,
        *,
        retrieval,
        materials,
        content,
        derivations,
        imports,
        models,
        configuration,
        execution,
        feedback_owner,
        symbols,
        motivation,
        reading,
    ):
        self.authority = authority
        self.cognition = cognition
        self.retrieval = retrieval
        self.materials = materials
        self.content = content
        self.derivations = derivations
        self.imports = imports
        self.models = models
        self.configuration = configuration
        self.execution = execution
        self.feedback_owner = feedback_owner
        self.symbols = symbols
        self.motivation = motivation
        self.reading = reading

    async def open(self, ctx: CallContext, command: RuntimeOpen) -> RuntimeBindingInfo:
        info = RuntimeBindingInfo(
            session=command.session,
            instance=command.instance,
            consumer=command.consumer,
            work=command.work.removeprefix("work:") if command.work else None,
            usage=command.usage,
        )
        async with self.authority.write(ctx) as tx:
            await require_namespace_in(tx, ctx)
            raw = await tx.connection.scalar(
                sa.select(sessions.c.record).where(
                    sessions.c.id == session_key(ctx, command.session),
                    sessions.c.namespace_id == ctx.namespace_id,
                )
            )
            session = (
                ContextSession.model_validate(raw)
                if raw is not None
                else ContextSession(
                    id=command.session,
                    namespace_id=ctx.namespace_id,
                    closed=False,
                    recorded_at=tx.recorded_at,
                    commit_seq=tx.commit_seq,
                )
            )
            if session.closed:
                raise DomainError(
                    "session_closed", "Open a new session after closing this conversation"
                )
            if info.work is not None:
                await self.work_in(tx, ctx, info.work)
            if command.work is None:
                info = info.model_copy(update={"work": session.active_work_context_id})
            instances = dict(session.host_instances)
            instances[info.instance] = {
                "info": info.model_dump(mode="json", exclude={"work"}),
                "form_long_term": command.form_long_term,
                "context_window_tokens": command.context_window_tokens,
                "output_reserve": command.output_reserve,
            }
            updated = session.model_copy(
                update={
                    "active_work_context_id": info.work
                    if command.work is not None
                    else session.active_work_context_id,
                    "host_instances": instances,
                    "revision": session.revision + bool(raw),
                    "commit_seq": tx.commit_seq,
                }
            )
            await tx.connection.execute(
                sa.dialects.sqlite.insert(sessions)
                .values(
                    id=session_key(ctx, session.id),
                    namespace_id=ctx.namespace_id,
                    record=updated.model_dump(mode="json"),
                )
                .on_conflict_do_update(
                    index_elements=[sessions.c.id], set_={"record": updated.model_dump(mode="json")}
                )
            )
        return info

    async def change_session(
        self, ctx: CallContext, command: ChangeContextSession
    ) -> ContextSession:
        async with self.authority.write(ctx) as tx:
            return await self.change_session_in(tx, ctx, command)

    async def change_session_in(self, tx, ctx, command) -> ContextSession:
        current = await self.session_in(tx, ctx, command.session_id)
        if current.revision != command.expected_revision:
            raise DomainError("session_revision_conflict", "Session revision changed")
        updates = {"revision": current.revision + 1, "commit_seq": tx.commit_seq}
        if "active_work_context_id" in command.model_fields_set:
            if command.active_work_context_id is not None:
                work = await self.work_in(tx, ctx, command.active_work_context_id)
                if work.state != "open":
                    raise DomainError(
                        "work_context_inactive", "Resume the Work before selecting it"
                    )
            updates["active_work_context_id"] = command.active_work_context_id
        if command.invalidate_history:
            updates["history_invalidated"] = True
        if command.close:
            updates.update(closed=True, closed_at=tx.recorded_at)
        revised = current.model_copy(update=updates)
        await tx.connection.execute(
            sessions.update()
            .where(
                sessions.c.id == session_key(ctx, current.id),
                sessions.c.namespace_id == ctx.namespace_id,
            )
            .values(record=revised.model_dump(mode="json"))
        )
        return revised

    async def close_session(self, ctx: CallContext, session_id: str) -> ContextSession:
        async with self.authority.write(ctx) as tx:
            current = await self.session_in(tx, ctx, session_id)
            return await self.change_session_in(
                tx,
                ctx,
                ChangeContextSession(
                    session_id=session_id, expected_revision=current.revision, close=True
                ),
            )

    async def _binding(self, ctx, info):
        async with self.authority.read(ctx) as view:
            session = await self.session_in(view, ctx, info.session)
            settings = session.host_instances.get(info.instance)
            if settings is None:
                raise DomainError(
                    "runtime_binding_not_found", "Open this runtime instance before preparing input"
                )
            latest = await view.connection.scalar(
                sa.select(calls.c.record)
                .where(
                    calls.c.namespace_id == ctx.namespace_id,
                    calls.c.caller_id == ctx.caller_id,
                    calls.c.session_id == info.session,
                    calls.c.instance_id == info.instance,
                )
                .order_by(calls.c.commit_seq.desc())
                .limit(1)
            )
        return {
            **settings,
            "history": list(latest.get("host_history", ())) if latest else [],
            "generation": latest["generation"] if latest else str(uuid4()),
            "last_call_ref": latest["call_ref"]
            if latest and latest["state"] in {"sent", "completed"}
            else latest.get("parent")
            if latest
            else None,
            "current_call": latest["call_ref"]
            if latest and latest["state"] in {"prepared", "sent"}
            else None,
            "model": latest.get("model") if latest else None,
            "format": latest.get("format") if latest else None,
            "latest": latest,
        }

    async def _raw(self, ctx, ref):
        reference = MediaReference.model_validate(ref)
        material = await self.materials.read(ctx, reference.id)
        data = await owned_thread(
            lambda: self.content.path_for(material["content_hash"]).read_bytes()
        )
        return json.loads(data)

    async def _observe_message(self, ctx, info, message, format, identity, form, actual_event=None):
        role = message_role(message, format)
        text = message_text(message, format)
        response_id = message.get("provider_response_id") if format == "pydantic-ai" else None
        event_id = (
            "response:" + format + ":" + response_id if isinstance(response_id, str) else identity
        )
        raw = message
        tool_id = None
        if role == "tool":
            parts = message.get("parts", []) if format == "pydantic-ai" else [message]
            tool_parts = [
                part
                for part in parts
                if isinstance(part, dict) and isinstance(part.get("tool_call_id"), str)
            ]
            if len(tool_parts) == 1:
                part = tool_parts[0]
                tool_id = part["tool_call_id"]
                event_id = "tool:" + tool_id
                raw = {"tool_call_id": tool_id, "result": part.get("content")}
                text = (
                    raw["result"]
                    if isinstance(raw["result"], str)
                    else canonical_json(raw["result"])
                )
        if actual_event is not None:
            event = await self._capture(ctx, info, actual_event, form=form)
            event_id = actual_event.event_id
        else:
            event = await self._capture(
                ctx,
                info,
                HostEvent(
                    event_id=event_id,
                    role=role,
                    payload=raw,
                    text=text,
                    tool_call_id=tool_id,
                    response_id=response_id,
                ),
                form=form and bool(text),
            )
        wire = event
        if (actual_event is None and raw is not message) or (
            actual_event is not None and actual_event.payload != message
        ):
            wire = await self._capture(
                ctx, info, HostEvent(event_id="wire:" + identity, role="system", payload=message)
            )
        return {
            "fingerprint": fingerprint(message, format),
            "event_id": event_id,
            "role": role,
            "wire_source": wire["raw_source"],
            "source": event["source"],
            "payload_kind": "message",
            "format": format,
        }

    async def _history(self, ctx, command, state):
        incoming = clean_messages(command.request, command.format)
        known_blocks = {block["id"] for block in (state.get("latest") or {}).get("block_plan", ())}
        for message in command.request.get("messages", ()):
            marker = block_identity(message, command.format)
            if marker is not None and marker not in known_blocks:
                raise DomainError(
                    "runtime_history_namespace_mismatch",
                    "Context block is outside this runtime call history",
                )
        signatures = [fingerprint(message, command.format) for message in incoming]
        prior = state["history"]
        same = command.format == state["format"] and signatures[: len(prior)] == [
            item["fingerprint"] for item in prior
        ]
        if command.history_mode == "append":
            compatible = command.format == state["format"]
            new_messages = incoming[len(prior) :] if same else incoming
            same = compatible
            history = list(prior) if compatible else []
        elif same:
            new_messages = incoming[len(prior) :]
            history = list(prior)
        else:
            new_messages = incoming
            history = []
        supplied = list(command.events)
        for index, message in enumerate(new_messages):
            actual_event = next(
                (
                    event
                    for event in supplied
                    if event.role == message_role(message, command.format)
                    and (
                        event.payload == message
                        or bool(event.text)
                        and event.text == message_text(message, command.format)
                    )
                ),
                None,
            )
            if actual_event is not None:
                supplied.remove(actual_event)
            history.append(
                await self._observe_message(
                    ctx,
                    command.binding,
                    message,
                    command.format,
                    actual_event.event_id
                    if actual_event is not None
                    else "host:" + command.invocation_id + ":" + str(index),
                    bool(state["form_long_term"]),
                    actual_event=actual_event,
                )
            )
        actual = []
        for entry in history:
            payload = await self._raw(ctx, entry["wire_source"])
            message = (
                response_message(payload, entry["format"])
                if entry["payload_kind"] == "response"
                else payload
            )
            actual.append(message)
        return history, actual, not same

    async def _keys(self, ctx, info, projection, extra_targets=()):
        targets = {
            canonical_json(target) for segment in projection.segments for target in segment.sources
        } | {canonical_json(target) for target in extra_targets}
        async with self.authority.write(ctx) as tx:
            rows = (
                await tx.connection.execute(
                    sa.select(source_keys.c.number, source_keys.c.target).where(
                        source_keys.c.namespace_id == ctx.namespace_id,
                        source_keys.c.session_id == session_key(ctx, info.session),
                    )
                )
            ).all()
            known = {row.target: "c" + str(row.number) for row in rows}
            number = max((row.number for row in rows), default=0)
            for target in sorted(targets - known.keys()):
                number += 1
                known[target] = "c" + str(number)
                await tx.connection.execute(
                    source_keys.insert().values(
                        namespace_id=ctx.namespace_id,
                        session_id=session_key(ctx, info.session),
                        number=number,
                        target=target,
                    )
                )
            return {target: known[target] for target in targets}

    async def _call(self, ctx, ref):
        async with self.authority.read(ctx) as view:
            value = await view.connection.scalar(
                sa.select(calls.c.record).where(
                    calls.c.id == ref.removeprefix("call:"),
                    calls.c.namespace_id == ctx.namespace_id,
                    calls.c.caller_id == ctx.caller_id,
                )
            )
        if value is None:
            raise DomainError("runtime_call_not_found", "Runtime call is unavailable")
        return value

    async def _prepared(self, ctx, record):
        request = await self._raw(ctx, record["prepared_request_ref"])
        result = RuntimePreparation(
            call_ref=record["call_ref"],
            binding=record["binding"],
            invocation_id=record["invocation_id"],
            projection=record["projection"],
            format=record["format"],
            request=request,
            request_hash=record["request_hash"],
            history_generation=record["generation"],
            stable_prefix_fingerprint=record["stable_prefix_fingerprint"],
            input_bindings=record["input_bindings"],
            blocks=record["manifest"],
            measurement=record["measurement"],
            working_source=record["working_source"],
            replayed_output=await self._raw(ctx, record["output_raw_source"])
            if record["state"] == "completed" and record.get("output_raw_source")
            else None,
        )
        return result

    async def prepare(self, ctx: CallContext, command: RuntimePrepare) -> RuntimePreparation:
        async with self.authority.read(ctx) as view:
            existing = await view.connection.scalar(
                sa.select(calls.c.record).where(
                    calls.c.namespace_id == ctx.namespace_id,
                    calls.c.caller_id == ctx.caller_id,
                    calls.c.session_id == command.binding.session,
                    calls.c.instance_id == command.binding.instance,
                    calls.c.invocation_id == command.invocation_id,
                )
            )
        incoming_hash = sha256(
            canonical_json(
                {
                    "request": command.request,
                    "format": command.format,
                    "history_mode": command.history_mode,
                }
            ).encode()
        ).hexdigest()
        if existing is not None:
            if existing["state"] in {"failed", "cancelled"}:
                raise DomainError(
                    "runtime_invocation_closed",
                    "A finished failed call requires a new invocation identity",
                )
            if existing["incoming_hash"] != incoming_hash:
                raise DomainError(
                    "runtime_invocation_conflict",
                    "Invocation identity selects a different host input",
                )
            return await self._prepared(ctx, existing)
        state = await self._binding(ctx, command.binding)
        if state["current_call"] is not None:
            raise DomainError(
                "runtime_call_in_flight", "Finish the current call before preparing another input"
            )
        for event in command.events:
            await self._capture(
                ctx, command.binding, event, form=bool(state["form_long_term"]) and bool(event.text)
            )
        history, host_messages, rebase = await self._history(ctx, command, state)
        model = command.request.get("model")
        reset = rebase or model != state["model"]
        generation = str(uuid4()) if reset else state["generation"]
        present = tuple(
            MediaTarget(reference=MediaReference.model_validate(entry["source"]))
            for entry in history
        )
        await self._reside(
            ctx,
            command.binding,
            tuple(target.reference for target in present[-8:]),
            "Actual host interaction",
        )
        original_input = await self._capture(
            ctx,
            command.binding,
            HostEvent(
                event_id="input:" + command.invocation_id, role="system", payload=command.request
            ),
        )
        cached_compression = None
        cached_summary = None
        cached_block_id = None
        budget_messages = host_messages
        if state.get("latest") is not None and not rebase:
            previous_call = state["latest"]
            previous_wire = await self._raw(ctx, previous_call["prepared_request_ref"])
            prior_input_count = len(previous_call["host_history"]) - int(
                previous_call.get("output_raw_source") is not None
            )
            budget_messages = previous_wire["messages"] + host_messages[prior_input_count:]
            cached_compression = previous_call.get("compression")
            if cached_compression is not None:
                async with self.authority.read(ctx) as view:
                    value = await self.derivations.selected_in(
                        view, ctx, MediaReference.model_validate(cached_compression["summary"])
                    )
                cached_summary = value.payload.text
                original_target = {"kind": "media", "reference": cached_compression["source"]}
                cached_block_id = next(
                    block["id"]
                    for block in previous_call["block_plan"]
                    if not block["segment_keys"] and original_target in block["bindings"].values()
                )
        baseline = await self.models.measure_host(
            ctx,
            {**command.request, "messages": budget_messages, "format": command.format},
            context_window=state["context_window_tokens"],
            output_reserve=state["output_reserve"],
        )
        pressure = (
            baseline.context_window is not None
            and baseline.input_tokens + baseline.output_reserve
            >= int(baseline.context_window * 0.85)
        )
        boundary = completed_prefix(host_messages, command.format)
        working_source = None
        history_source_bytes = 0
        if pressure and boundary:
            prefix = host_messages[:boundary]
            text = "\n\n".join(
                message_role(item, command.format) + ": " + message_text(item, command.format)
                for item in prefix
                if message_role(item, command.format) != "system"
            )
            if text:
                working = await self._capture(
                    ctx,
                    command.binding,
                    HostEvent(
                        event_id="history:" + command.invocation_id,
                        role="system",
                        payload={"messages": prefix},
                        text=text,
                    ),
                )
                working_source = MediaReference.model_validate(working["source"])
                history_source_bytes = len(text.encode("utf-8"))
        question = next(
            (
                message_text(message, command.format)
                for message in reversed(host_messages)
                if message_role(message, command.format) == "user"
            ),
            "",
        )
        usage = (
            command.binding.usage.model_copy(update={"intent": question})
            if question
            else command.binding.usage
        )
        history_group = None
        if pressure and boundary and working_source is not None:
            available = max(256, int(baseline.context_window or 4096) - baseline.output_reserve)
            history_group = WorkingHistoryGroup(
                group_id="history:" + command.invocation_id,
                label="已完成的会话",
                sources=(MediaTarget(reference=working_source),),
                purpose=usage.purpose,
                continuation=question,
                token_budget=max(128, min(512, available // 4)),
                start=0,
                end=boundary,
            )
        selection_bytes = command.max_text_bytes
        if baseline.complete and baseline.context_window is not None:
            remaining_tokens = max(
                0, baseline.context_window - baseline.output_reserve - baseline.input_tokens
            )
            selection_bytes = min(selection_bytes, max(1024, remaining_tokens * 2))
        projection = await compilation.select_projection(
            self,
            ctx,
            SelectionRequest(
                session_id=command.binding.session,
                consumer_id=command.binding.consumer,
                instance_id=command.binding.instance,
                usage=usage,
                query=command.query,
                known_cursor=ContextProjection.model_validate(state["latest"]["projection"]).cursor
                if state.get("latest") and not reset
                else None,
                present_sources=present,
                working_groups=(history_group,) if history_group is not None else (),
                history_source_bytes=history_source_bytes,
                trigger="budget" if pressure else command.trigger,
                include_optional=baseline.complete,
                max_items=command.max_items,
                max_text_bytes=selection_bytes,
            ),
            previous=ContextProjection.model_validate(state["latest"]["projection"])
            if state.get("latest") and not reset
            else None,
        )
        prior_blocks = ()
        prior_projection = (
            ContextProjection.model_validate(state["latest"]["projection"])
            if state.get("latest")
            else None
        )
        reset = (
            reset
            or prior_projection is None
            or projection.cursor.epoch_id != prior_projection.cursor.epoch_id
        )
        assigned = await self._keys(
            ctx,
            command.binding,
            projection,
            history_group.sources
            if history_group
            else (
                MediaTarget(reference=MediaReference.model_validate(cached_compression["source"])),
            )
            if cached_compression
            else (),
        )
        prior_request = None
        prior_count = 0
        if state["last_call_ref"] is not None and not reset:
            previous = await self._call(ctx, state["last_call_ref"])
            prior_request = await self._raw(ctx, previous["prepared_request_ref"])
            prior_count = len(previous["host_history"]) - int(
                previous.get("output_raw_source") is not None
            )
            block_text = {
                block_identity(message, command.format): message_text(message, command.format)
                for message in prior_request["messages"]
                if block_identity(message, command.format) is not None
            }
            prior_blocks = tuple(
                HostBlock(
                    id=block["id"],
                    role=block["role"],
                    text=block_text[block["id"]],
                    bindings=block["bindings"],
                    stable=block["stable"],
                    segment_keys=tuple(block["segment_keys"]),
                )
                for block in previous["block_plan"]
            )
        summary = None
        accepted_summary = None
        decision = projection.steward_decision
        if history_group is not None and decision is not None:
            accepted_summary = next(
                (
                    item
                    for item in decision.group_summaries
                    if item.group_id == history_group.group_id
                    and item.covered_sources == history_group.sources
                ),
                None,
            )
            if accepted_summary is not None:
                summary = accepted_summary.text
                generation = str(uuid4())
                reset = True
        summary_boundary = boundary
        summary_sources = history_group.sources if history_group else ()
        if summary is None and cached_compression is not None:
            summary = cached_summary
            summary_boundary = cached_compression["end"]
            summary_sources = (
                MediaTarget(reference=MediaReference.model_validate(cached_compression["source"])),
            )
        built = construct(
            command.request,
            command.format,
            host_messages,
            projection,
            assigned,
            namespace_id=ctx.namespace_id,
            generation=generation,
            prior_request=prior_request,
            prior_host_count=prior_count,
            prior_blocks=prior_blocks,
            reset=reset,
            history_summary=summary,
            history_boundary=summary_boundary,
            history_block_id=cached_block_id if accepted_summary is None else None,
            history_bindings={
                assigned[canonical_json(target)]: target.model_dump(mode="json")
                for target in summary_sources
            }
            if summary
            else None,
        )
        measurement = await self.models.measure_host(
            ctx,
            {**built.request, "format": command.format},
            context_window=state["context_window_tokens"],
            output_reserve=state["output_reserve"],
        )
        if (
            measurement.complete
            and measurement.context_window is not None
            and measurement.input_tokens + measurement.output_reserve > measurement.context_window
        ):
            raise DomainError(
                "runtime_compression_waiting",
                "The current history needs the configured compression model before it can be sent",
                input_tokens=measurement.input_tokens,
                context_window=measurement.context_window,
            )
        wire_event = HostEvent(
            event_id="prepared:" + command.invocation_id, role="system", payload=built.request
        )
        wire_lease = await self.content.publish_bytes(
            canonical_json(built.request).encode(), "application/json"
        )
        identity = str(uuid4())
        call_ref = "call:" + identity
        manifest = tuple(
            RuntimeBlock(
                block_id=block.id,
                role=block.role,
                segment_keys=block.segment_keys,
                input_bindings=block.bindings,
                stable=block.stable,
            )
            for block in built.blocks
        )
        record = {
            "call_ref": call_ref,
            "binding": command.binding.model_dump(mode="json"),
            "invocation_id": command.invocation_id,
            "incoming_hash": incoming_hash,
            "projection": projection.model_dump(mode="json"),
            "parent": state["last_call_ref"],
            "original_input_ref": original_input["raw_source"],
            "model": model,
            "format": command.format,
            "prepared_request_ref": None,
            "request_hash": sha256(canonical_json(built.request).encode()).hexdigest(),
            "generation": generation,
            "stable_prefix_fingerprint": built.stable_prefix_fingerprint,
            "input_bindings": built.input_bindings,
            "manifest": [item.model_dump(mode="json") for item in manifest],
            "block_plan": [
                {key: value for key, value in asdict(block).items() if key != "text"}
                for block in built.blocks
            ],
            "measurement": measurement.model_dump(mode="json"),
            "working_source": working_source.model_dump(mode="json") if working_source else None,
            "host_sources": [target.reference.model_dump(mode="json") for target in present],
            "state": "prepared",
            "host_history": history,
        }
        if cached_compression is not None and accepted_summary is None:
            record["compression"] = cached_compression
        try:
            async with self.authority.write(ctx) as tx:
                await self.require_projection_in(tx, ctx, projection)
                actual = await self._capture_in(tx, ctx, command.binding, wire_event, wire_lease)
                record["prepared_request_ref"] = actual["raw_source"]
                record["recorded_at"] = tx.recorded_at
                if accepted_summary is not None:
                    representation = await self.derivations.adopt_in(
                        tx,
                        ctx,
                        CreateRepresentation(
                            kind="summary",
                            strategy="description_only",
                            inputs=(DerivationInput(ordinal=0, source=working_source),),
                            payload=TextRepresentation(text=accepted_summary.text),
                            producer=accepted_summary.producer,
                        ),
                    )
                    record["compression"] = {
                        "group_id": history_group.group_id,
                        "start": history_group.start,
                        "end": history_group.end,
                        "source": working_source.model_dump(mode="json"),
                        "summary": representation.reference.media().model_dump(mode="json"),
                        "token_budget": history_group.token_budget,
                        "before_tokens": baseline.input_tokens,
                        "after_tokens": measurement.input_tokens,
                    }
                session = await self.session_in(tx, ctx, command.binding.session)
                if session.history_invalidated:
                    updated = session.model_copy(
                        update={"history_invalidated": False, "revision": session.revision + 1}
                    )
                    await tx.connection.execute(
                        sessions.update()
                        .where(sessions.c.id == session_key(ctx, session.id))
                        .values(record=updated.model_dump(mode="json"))
                    )
                await tx.connection.execute(
                    calls.insert().values(
                        id=identity,
                        namespace_id=ctx.namespace_id,
                        caller_id=ctx.caller_id,
                        session_id=command.binding.session,
                        instance_id=command.binding.instance,
                        invocation_id=command.invocation_id,
                        record=record,
                        commit_seq=tx.commit_seq,
                    )
                )
        finally:
            await self.content.release(wire_lease)
        return await self._prepared(ctx, record)

    async def read(self, ctx: CallContext, command: RuntimeRead):
        from nous_sdk.contracts.public import ReadRequest
        from nous_sdk.contracts.targets import ContextTarget

        await self._binding(ctx, command.binding)
        target = None
        if command.ref.startswith("c") and command.ref[1:].isdigit():
            async with self.authority.read(ctx) as view:
                raw = await view.connection.scalar(
                    sa.select(source_keys.c.target).where(
                        source_keys.c.namespace_id == ctx.namespace_id,
                        source_keys.c.session_id == session_key(ctx, command.binding.session),
                        source_keys.c.number == int(command.ref[1:]),
                    )
                )
            if raw is None:
                raise DomainError(
                    "reference_not_found", "Runtime source reference does not exist in this session"
                )
            target = TypeAdapter(ContextTarget).validate_json(raw)
        return await self.reading.read(
            ctx,
            ReadRequest(ref=command.ref, sources=command.sources, max_chars=command.max_chars),
            target=target,
        )

    async def sent(self, ctx: CallContext, command: RuntimeSent) -> RuntimeResult:
        value = await self._call(ctx, command.call_ref)
        if value["request_hash"] != command.request_hash:
            raise DomainError("runtime_input_changed", "Send the request constructed for this call")
        if value["state"] in {"sent", "completed"}:
            return RuntimeResult(call_ref=command.call_ref, state=value["state"])
        projection = ContextProjection.model_validate(value["projection"])
        async with self.authority.write(ctx) as tx:
            await self.require_projection_in(tx, ctx, projection)
            from .feedback_events import record_in

            targets = tuple(
                dict.fromkeys(
                    tuple(target for segment in projection.segments for target in segment.sources)
                    + tuple(
                        MediaTarget(reference=MediaReference.model_validate(ref))
                        for ref in value["host_sources"]
                    )
                )
            )
            await self.require_targets_in(tx, ctx, targets)
            for target in targets:
                await record_in(
                    self,
                    tx,
                    ctx,
                    context_id=value["call_ref"],
                    event_id="model-contact:" + target.model_dump_json(),
                    target=target,
                    kind="model_contact",
                    occurred_at=tx.recorded_at,
                    invocation_id=value["invocation_id"],
                    session_id=value["binding"]["session"],
                )
            value.update(state="sent", sent_at=tx.recorded_at, sent_evidence=command.evidence)
            await tx.connection.execute(
                calls.update()
                .where(
                    calls.c.id == command.call_ref.removeprefix("call:"),
                    calls.c.namespace_id == ctx.namespace_id,
                    calls.c.caller_id == ctx.caller_id,
                )
                .values(record=value, commit_seq=tx.commit_seq)
            )
        return RuntimeResult(call_ref=command.call_ref, state="sent")

    async def complete(self, ctx: CallContext, command: RuntimeComplete) -> RuntimeResult:
        value = await self._call(ctx, command.call_ref)
        if command.outcome == "completed" and command.output is None:
            raise DomainError(
                "runtime_output_required", "Record the complete actual model response"
            )
        info = RuntimeBindingInfo.model_validate(value["binding"])
        settings = await self._binding(ctx, info)
        policy = await self.policy(ctx)
        output_digest = (
            sha256(canonical_json(command.output).encode()).hexdigest()
            if command.output is not None
            else None
        )
        if value.get("result") is not None and value.get("output_digest") == output_digest:
            return RuntimeResult.model_validate(value["result"])
        message = (
            response_message(command.output, value["format"])
            if command.output is not None
            else None
        )
        event = (
            HostEvent(
                event_id="response:"
                + value["format"]
                + ":"
                + (command.response_id or value["invocation_id"]),
                role="assistant",
                payload=command.output,
                text=command.text or message_text(message, value["format"]),
                response_id=command.response_id,
            )
            if message is not None
            else None
        )
        lease = (
            await self.content.publish_bytes(
                canonical_json(command.output).encode(), "application/json"
            )
            if event
            else None
        )
        try:
            async with self.authority.write(ctx) as tx:
                output = (
                    await self._capture_in(
                        tx, ctx, info, event, lease, form=bool(settings["form_long_term"])
                    )
                    if event
                    else None
                )
                history = list(value["host_history"])
                if output is not None:
                    history.append(
                        {
                            "fingerprint": fingerprint(message, value["format"]),
                            "event_id": output["event_id"],
                            "role": "assistant",
                            "wire_source": output["raw_source"],
                            "source": output["source"],
                            "payload_kind": "response",
                            "format": value["format"],
                        }
                    )
                    await self._reside_in(
                        tx,
                        ctx,
                        info,
                        (MediaReference.model_validate(output["source"]),),
                        "Actual model response",
                        policy,
                    )
                prepared = RuntimePreparation(
                    call_ref=value["call_ref"],
                    binding=info,
                    invocation_id=value["invocation_id"],
                    projection=ContextProjection.model_validate(value["projection"]),
                    format=value["format"],
                    request={},
                    request_hash=value["request_hash"],
                    history_generation=value["generation"],
                    stable_prefix_fingerprint=value["stable_prefix_fingerprint"],
                    input_bindings=value["input_bindings"],
                    blocks=value["manifest"],
                    measurement=value["measurement"],
                    working_source=value["working_source"],
                )
                await self._apply_steward_in(tx, ctx, prepared, policy)
                result = RuntimeResult(
                    call_ref=command.call_ref,
                    state=command.outcome,
                    output_ref=MediaReference.model_validate(output["source"]) if output else None,
                )
                value.update(
                    state=command.outcome,
                    output_digest=output_digest,
                    output_raw_source=output["raw_source"] if output else None,
                    output_source=output["source"] if output else None,
                    result=result.model_dump(mode="json"),
                    host_history=history,
                    usage=command.usage,
                    error=command.error,
                    completed_at=tx.recorded_at,
                )
                await tx.connection.execute(
                    calls.update()
                    .where(
                        calls.c.id == command.call_ref.removeprefix("call:"),
                        calls.c.namespace_id == ctx.namespace_id,
                        calls.c.caller_id == ctx.caller_id,
                    )
                    .values(record=value, commit_seq=tx.commit_seq)
                )
                return result
        finally:
            if lease is not None:
                await self.content.release(lease)

    async def _capture(
        self, ctx: CallContext, info: RuntimeBindingInfo, event: HostEvent, *, form: bool = False
    ):
        lease = await self.content.publish_bytes(
            canonical_json(event.payload).encode(), "application/json"
        )
        try:
            async with self.authority.write(ctx) as tx:
                return await self._capture_in(tx, ctx, info, event, lease, form=form)
        finally:
            await self.content.release(lease)

    async def _capture_in(self, tx, ctx, info, event, lease, *, form=False):
        digest = sha256(canonical_json(event.payload).encode()).hexdigest()
        existing = await tx.connection.scalar(
            sa.select(events.c.record).where(
                events.c.namespace_id == ctx.namespace_id,
                events.c.caller_id == ctx.caller_id,
                events.c.event_id == event.event_id,
            )
        )
        if existing is not None:
            if (existing["payload_digest"], existing["role"], existing.get("text_digest")) != (
                digest,
                event.role,
                sha256(event.text.encode()).hexdigest(),
            ):
                raise DomainError(
                    "runtime_event_conflict", "Host event identity has different actual content"
                )
            await self.materials.read_in(tx, ctx, existing["material_id"])
            return existing
        document = BlobDocument(
            filename=event.role + "-event.json",
            media_type="application/json",
            source_type="runtime_" + event.role,
            source=event.event_id,
            origin={"caller_id": ctx.caller_id, "session_id": info.session, "role": event.role},
        )
        admission = await self.materials.admit_blob_in(
            tx, ctx, lease, document, kind="contact" if event.role != "system" else "import"
        )
        raw = MediaReference(
            kind="material", id=admission.material["id"], observation_id=admission.event["id"]
        )
        source = raw
        if event.text:
            representation = await self.derivations.adopt_in(
                tx,
                admission.context,
                CreateRepresentation(
                    kind="extracted_text",
                    strategy="preprocess",
                    inputs=(DerivationInput(ordinal=0, source=raw),),
                    payload=TextRepresentation(text=event.text[:1000000]),
                    quality=DerivationQuality(truncated=len(event.text) > 1000000),
                ),
            )
            source = representation.reference.media()
        if admission.event["kind"] == "contact":
            from nous.material.service import observations

            contact = {
                **admission.event,
                "contact_source": source.model_dump(mode="json"),
                "external_event_id": event.event_id,
                "caller_id": ctx.caller_id,
            }
            await tx.connection.execute(
                observations.update()
                .where(
                    observations.c.namespace_id == ctx.namespace_id,
                    observations.c.id == admission.event["id"],
                )
                .values(record=contact, caller_id=ctx.caller_id, external_event_id=event.event_id)
            )
        value = {
            "event_id": event.event_id,
            "role": event.role,
            "session_id": info.session,
            "caller_id": ctx.caller_id,
            "payload_digest": digest,
            "text_digest": sha256(event.text.encode()).hexdigest(),
            "material_id": raw.id,
            "raw_source": raw.model_dump(mode="json"),
            "source": source.model_dump(mode="json"),
            "observation_id": admission.event["id"],
            "response_id": event.response_id,
            "tool_call_id": event.tool_call_id,
            "commit_seq": tx.commit_seq,
            "recorded_at": tx.recorded_at,
        }
        if form and event.text and event.role != "system":
            job = await self.imports.submit_form_observation_in(
                tx,
                admission.context,
                FormObservation(
                    operation_id="runtime-form:" + event.event_id,
                    observation_id=admission.event["id"],
                    source=source,
                ),
            )
            value["formation"] = job.model_dump(mode="json")
        await tx.connection.execute(
            events.insert().values(
                namespace_id=ctx.namespace_id,
                caller_id=ctx.caller_id,
                session_id=info.session,
                event_id=event.event_id,
                record=value,
                commit_seq=tx.commit_seq,
            )
        )
        return value

    async def capture_tool(self, ctx: CallContext, command: RuntimeTool):
        state = await self._binding(ctx, command.binding)
        if command.event.role != "tool" or command.event.tool_call_id is None:
            raise DomainError(
                "tool_event_invalid", "Capture the actual tool result with its tool-call identity"
            )
        value = await self._capture(
            ctx, command.binding, command.event, form=bool(state["form_long_term"])
        )
        await self._reside(
            ctx,
            command.binding,
            (MediaReference.model_validate(value["source"]),),
            "Actual tool result",
        )
        return value

    async def _reside(
        self,
        ctx: CallContext,
        info: RuntimeBindingInfo,
        sources: tuple[MediaReference, ...],
        reason: str,
    ):
        if not sources:
            return
        session = await self.read_session(ctx, info.session)
        await self.change_residents(
            ctx,
            ChangeResidents(
                session_id=info.session,
                expected_revision=session.revision,
                action="admit",
                residents=tuple(
                    ContextResidentAdmission(target=MediaTarget(reference=ref), entry_reason=reason)
                    for ref in sources
                ),
            ),
        )

    async def _reside_in(self, tx, ctx, info, sources, reason, policy):
        session = await self.session_in(tx, ctx, info.session)
        return await self._change_residents_in(
            tx,
            ctx,
            ChangeResidents(
                session_id=session.id,
                expected_revision=session.revision,
                action="admit",
                residents=tuple(
                    ContextResidentAdmission(
                        target=MediaTarget(reference=source), entry_reason=reason
                    )
                    for source in sources
                ),
            ),
            policy,
        )

    async def _apply_steward_in(self, tx, ctx, prepared, policy):
        decision = prepared.projection.steward_decision
        if decision is None:
            return
        session = await self.session_in(tx, ctx, prepared.binding.session)
        if decision.evict:
            session = await self._change_residents_in(
                tx,
                ctx,
                ChangeResidents(
                    session_id=session.id,
                    expected_revision=session.revision,
                    action="evict",
                    targets=decision.evict,
                ),
                policy,
            )
        if decision.promote:
            session = await self._change_residents_in(
                tx,
                ctx,
                ChangeResidents(
                    session_id=session.id,
                    expected_revision=session.revision,
                    action="admit",
                    residents=tuple(
                        ContextResidentAdmission(
                            target=target, entry_reason="Steward working-set selection"
                        )
                        for target in decision.promote
                    ),
                ),
                policy,
            )
        if session.active_work_context_id and (
            decision.work_summary is not None or decision.unresolved
        ):
            work = await self.work_in(tx, ctx, session.active_work_context_id)
            content = work.content.model_copy(
                update={
                    "context_text": decision.work_summary
                    if decision.work_summary is not None
                    else work.content.context_text,
                    "unresolved_questions": decision.unresolved,
                }
            )
            await self.change_work_in(
                tx,
                ctx,
                ChangeWorkContext(
                    work_context_id=work.id,
                    expected_revision=work.revision,
                    action="edit",
                    content=content,
                ),
            )

    async def purge_references_in(self, tx, ctx, references):
        identifiers = {reference.id for reference in references}
        work_rows = (
            (
                await tx.connection.execute(
                    sa.select(works).where(works.c.namespace_id == ctx.namespace_id)
                )
            )
            .mappings()
            .all()
        )
        for row in work_rows:
            value = WorkContext.model_validate(row["record"])
            if any(
                target.kind == "cognition" and target.reference.id in identifiers
                for target in value.content.anchors
            ):
                cleaned = value.model_copy(
                    update={
                        "purged": True,
                        "content": value.content.model_copy(
                            update={"context_text": "", "anchors": ()}
                        ),
                    }
                )
                await tx.connection.execute(
                    works.update()
                    .where(
                        works.c.namespace_id == ctx.namespace_id,
                        works.c.caller_id == row["caller_id"],
                        works.c.id == value.id,
                        works.c.revision == value.revision,
                    )
                    .values(record=cleaned.model_dump(mode="json"))
                )
