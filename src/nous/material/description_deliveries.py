"""Leased delivery of an actual media description to its consuming host.

This outbox owns external delivery only. Formation and vector execution stay in DBOS.
"""
from __future__ import annotations
from nous.cognition.memory import MemoryService
from hashlib import sha256
from time import time
from typing import TYPE_CHECKING
from uuid import uuid4
import sqlalchemy as sa
from nous_sdk.contracts import CallContext, CognitiveRef, DomainError, MemoryBody
from nous_sdk.contracts.media_policy import MediaPolicy
from pydantic import JsonValue, TypeAdapter
from nous.identity.access import require_namespace_in
from nous.platform.persistence import Authority, WriteTx, metadata
from . import external_descriptions
from .service import SourceRecord, require_observation_in
if TYPE_CHECKING:
    from nous.cognition.records import CognitiveRecords
    from nous.platform.configuration.service import ConfigurationService
deliveries = sa.Table('material_description_deliveries', metadata, sa.Column('id', sa.String, primary_key=True), sa.Column('namespace_id', sa.String, nullable=False, index=True), sa.Column('observation_id', sa.String, nullable=False, index=True), sa.Column('material_id', sa.String, nullable=False), sa.Column('record', sa.JSON, nullable=False))
_OBJECT = TypeAdapter(dict[str, JsonValue])

class DescriptionDeliveryService:
    record_external = external_descriptions.record_external
    matching_external = external_descriptions.matching_external
    adopt_external = external_descriptions.adopt_external
    pending_external_count = external_descriptions.pending_external_count

    def __init__(self, authority: Authority, cognition: CognitiveRecords, configuration: ConfigurationService, *, memories: MemoryService):
        self.memories = memories
        self.authority, self.cognition, self.configuration = (authority, cognition, configuration)

    async def items(self, ctx: CallContext) -> tuple[dict[str, JsonValue], ...]:
        result: list[dict[str, JsonValue]] = []
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view, ctx)
            rows = (await view.connection.execute(sa.select(deliveries).where(deliveries.c.namespace_id == ctx.namespace_id))).mappings()
            for row in rows:
                value = _OBJECT.validate_python(row['record'])
                try:
                    await require_observation_in(view, ctx, str(row['observation_id']))
                    await self.cognition.get_in(view, ctx, CognitiveRef.model_validate(value['description']))
                except DomainError as error:
                    if error.code in {'source_unavailable', 'cognition_unavailable', 'cognition_not_found', 'cognition_deleted', 'source_purged', 'cognition_purged'}:
                        continue
                    raise
                result.append({'id': str(row['id']), 'kind': 'description', 'asset_id': str(row['material_id']), 'occurrence_id': str(row['observation_id']), **value})
        return tuple(result)

    async def enqueue_in(self, tx: WriteTx, ctx: CallContext, observation_id: str, description: CognitiveRef) -> str | None:
        contact = TypeAdapter(SourceRecord).validate_python((await require_observation_in(tx, ctx, observation_id))['record'])
        record = await self.cognition.get_in(tx, ctx, description)
        if contact['kind'] != 'contact' or not isinstance(record.body, MemoryBody):
            raise DomainError('description_contact_required', 'Description delivery requires an actual contact and Memory')
        if observation_id not in record.source_roots:
            raise DomainError('description_contact_mismatch', 'Description must retain the delivered contact')
        origin = contact['origin']
        occurred = contact.get('event_time')
        if not origin.get('conversation_ref') or not origin.get('message_id') or (not occurred) or (occurred.get('domain') != 'unix_ms'):
            return None
        digest = sha256(record.body.text.strip().encode()).hexdigest()
        rows = (await tx.connection.execute(sa.select(deliveries).where(deliveries.c.namespace_id == ctx.namespace_id, deliveries.c.observation_id == observation_id))).mappings()
        for row in rows:
            previous = _OBJECT.validate_python(row['record'])
            if previous.get('description_hash') == digest:
                return str(row['id'])
            if previous.get('status') in {'pending', 'running', 'failed'}:
                await tx.connection.execute(deliveries.update().where(deliveries.c.id == row['id']).values(record={**previous, 'status': 'cancelled', 'lease_token': None, 'lease_until': None}))
        identifier = str(uuid4())
        await tx.connection.execute(deliveries.insert().values(id=identifier, namespace_id=ctx.namespace_id, observation_id=observation_id, material_id=contact['material_id'], record={'description': description.model_dump(mode='json'), 'description_hash': digest, 'status': 'pending', 'attempt_count': 0, 'lease_token': None, 'lease_until': None, 'last_error': '', 'created_at': tx.recorded_at}))
        return identifier

    async def claim(self, ctx: CallContext, *, limit: int, operation_id: str) -> dict[str, JsonValue]:
        if not 1 <= limit <= 1000:
            raise DomainError('invalid_limit', 'Description delivery limit must be between 1 and 1000')
        policy = MediaPolicy.model_validate((await self.configuration.effective_instance('media'))['value'])
        token = str(uuid4())
        async with self.authority.write(ctx, operation_id, operation_kind='media.delivery.claim', scope=ctx.namespace_id, payload={'limit': limit}) as tx:
            await require_namespace_in(tx, ctx, 'memory')
            if tx.replayed:
                result = _OBJECT.validate_python(tx.replay_result)
                token = str(result['lease_token'])
            now = time()
            rows = (await tx.connection.execute(sa.select(deliveries).where(deliveries.c.namespace_id == ctx.namespace_id).order_by(deliveries.c.id))).mappings()
            items: list[JsonValue] = []
            for row in rows:
                value = _OBJECT.validate_python(row['record'])
                until = value.get('lease_until')
                expired = not isinstance(until, (int, float)) or until <= now
                if tx.replayed:
                    eligible = value.get('status') == 'running' and value.get('lease_token') == token and (not expired)
                else:
                    eligible = value.get('status') == 'pending' or (value.get('status') == 'running' and expired)
                if not eligible:
                    continue
                try:
                    event = TypeAdapter(SourceRecord).validate_python((await require_observation_in(tx, ctx, str(row['observation_id'])))['record'])
                    description = CognitiveRef.model_validate(value['description'])
                    await self.cognition.get_in(tx, ctx, description)
                except DomainError as error:
                    if error.code in {'source_unavailable', 'cognition_unavailable', 'cognition_not_found', 'cognition_deleted', 'source_purged', 'cognition_purged'}:
                        continue
                    raise
                attempts = int(str(value.get('attempt_count', 0)))
                if not tx.replayed:
                    if attempts >= policy.description_delivery_max_attempts:
                        await tx.connection.execute(deliveries.update().where(deliveries.c.id == row['id']).values(record={**value, 'status': 'failed', 'lease_token': None, 'lease_until': None}))
                        continue
                    value = {**value, 'status': 'running', 'attempt_count': attempts + 1, 'lease_token': token, 'lease_until': now + policy.description_delivery_lease_seconds}
                    await tx.connection.execute(deliveries.update().where(deliveries.c.id == row['id']).values(record=value))
                origin = event['origin']
                occurred = event.get('event_time')
                if not occurred:
                    continue
                items.append({'delivery_id': str(row['id']), 'occurrence_id': str(row['observation_id']), 'description_hash': value['description_hash'], 'description': description.model_dump(mode='json'), 'chat_id': origin.get('conversation_ref', ''), 'message_id': origin.get('message_id', ''), 'occurred_at': float(str(occurred['coordinate'])) / 1000, 'lease_until': value['lease_until']})
                if len(items) >= limit:
                    break
            result: dict[str, JsonValue] = {'success': True, 'lease_token': token, 'items': items}
            return result if tx.replayed else tx.result(result)

    async def complete(self, ctx: CallContext, *, observation_id: str, description_hash: str, lease_token: str, success: bool, error: str, operation_id: str) -> bool:
        policy = MediaPolicy.model_validate((await self.configuration.effective_instance('media'))['value'])
        payload = {'observation_id': observation_id, 'description_hash': description_hash, 'lease_token': lease_token, 'success': success, 'error': error}
        async with self.authority.write(ctx, operation_id, operation_kind='media.delivery.complete', scope=ctx.namespace_id, payload=payload) as tx:
            await require_namespace_in(tx, ctx, 'memory')
            await require_observation_in(tx, ctx, observation_id)
            if tx.replayed:
                return bool(tx.replay_result)
            now = time()
            rows = (await tx.connection.execute(sa.select(deliveries).where(deliveries.c.namespace_id == ctx.namespace_id, deliveries.c.observation_id == observation_id))).mappings()
            for row in rows:
                value = _OBJECT.validate_python(row['record'])
                if value.get('description_hash') != description_hash or value.get('lease_token') != lease_token or value.get('status') != 'running':
                    continue
                until = value.get('lease_until')
                if not isinstance(until, (float, int)) or until <= now:
                    return tx.result(False)
                await self.cognition.get_in(tx, ctx, CognitiveRef.model_validate(value['description']))
                status = 'done' if success else 'failed' if int(str(value['attempt_count'])) >= policy.description_delivery_max_attempts else 'pending'
                await tx.connection.execute(deliveries.update().where(deliveries.c.id == row['id']).values(record={**value, 'status': status, 'lease_token': None, 'lease_until': None, 'last_error': error if not success else ''}))
                return tx.result(True)
            return tx.result(False)

    async def purge_in(self, tx: WriteTx, *, material_ids: tuple[str, ...]=(), references: tuple[CognitiveRef, ...]=()) -> None:
        await external_descriptions.purge_external_in(self, tx, material_ids=material_ids, references=references)
        clauses: list[sa.ColumnElement[bool]] = []
        if material_ids:
            clauses.append(deliveries.c.material_id.in_(material_ids))
        if references:
            clauses.append(deliveries.c.record['description']['id'].as_string().in_([ref.id for ref in references]))
        if clauses:
            await tx.connection.execute(deliveries.delete().where(sa.or_(*clauses)))

    async def retry_failed(self, ctx: CallContext, operation_id: str) -> int:
        async with self.authority.write(ctx, operation_id, operation_kind='media.delivery.retry', scope=ctx.namespace_id, payload={}) as tx:
            await require_namespace_in(tx, ctx, 'memory')
            if tx.replayed:
                return int(str(tx.replay_result))
            count = 0
            rows = (await tx.connection.execute(sa.select(deliveries).where(deliveries.c.namespace_id == ctx.namespace_id))).mappings()
            for row in rows:
                value = _OBJECT.validate_python(row['record'])
                if value.get('status') != 'failed':
                    continue
                await require_observation_in(tx, ctx, str(row['observation_id']))
                await self.cognition.get_in(tx, ctx, CognitiveRef.model_validate(value['description']))
                await tx.connection.execute(deliveries.update().where(deliveries.c.id == row['id']).values(record={**value, 'status': 'pending', 'attempt_count': 0, 'last_error': ''}))
                count += 1
            return tx.result(count)
