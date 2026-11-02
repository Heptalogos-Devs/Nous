"""Compose source catalog formation and a real portable artifact on shared owners."""
from __future__ import annotations
from nous.cognition.memory import MemoryService
from hashlib import sha256
from typing import TYPE_CHECKING, Literal
from nous_sdk.contracts import BlobDocument, BundleRequest, CallContext, DomainError, FreezeCatalog, MediaInfo, MemoryBody, ProcessCatalog, RecordsQuery
from nous_sdk.contracts.bundles import MaterialSelection, PackageDescriptor
from nous_sdk.contracts.dataset_imports import BackfillSourceTimes, ImportDataset
from pydantic import JsonValue, TypeAdapter
from nous.platform.execution import JobSpec
from nous.platform.persistence import WriteTx
if TYPE_CHECKING:
    from nous.cognition.records import CognitiveRecords
    from nous.data_management.bundles import BundleService
    from nous.imports.service import ImportService
    from nous.material.service import MaterialService
    from nous.platform.content import ContentStore
    from nous.platform.persistence import Authority

class DatasetImportService:

    async def convert_lpmm_graph(self, ctx: CallContext, material_id: str) -> str:
        from nous.imports.lpmm_graph import convert_lpmm_graph
        return await convert_lpmm_graph(ctx, material_id, authority=self.authority, materials=self.material, content=self.content)

    def __init__(self, authority: Authority, cognition: CognitiveRecords, imports: ImportService, bundles: BundleService, material: MaterialService, content: ContentStore, *, memories: MemoryService) -> None:
        self.memories = memories
        self.authority = authority
        self.cognition = cognition
        self.imports = imports
        self.bundles = bundles
        self.material = material
        self.content = content

    async def submit_time_backfill(self, ctx: CallContext, command: BackfillSourceTimes) -> dict[str, JsonValue]:
        from nous.identity.access import require_namespace_in
        async with self.authority.write(ctx, command.operation_id, operation_kind='imports.source_times', scope=ctx.namespace_id, payload=command) as tx:
            if tx.replayed:
                return TypeAdapter(dict[str, JsonValue]).validate_python(tx.replay_result)
            await require_namespace_in(tx, ctx, capability='memory')
            job = await tx.enqueue(JobSpec('nous.imports.temporal_backfill', 'default', args=(ctx.model_dump(mode='json'), command.model_dump(mode='json'))))
            return tx.result({'job_id': job.workflow_id, 'dry_run': command.dry_run})

    async def run_time_backfill(self, context: JsonValue, payload: JsonValue) -> dict[str, JsonValue]:
        from nous.platform.time import parse_source_time
        ctx, command = (CallContext.model_validate(context), BackfillSourceTimes.model_validate(payload))
        query = RecordsQuery(kinds=('memory',), limit=min(command.limit, 1000))
        candidates: list[str] = []
        updated: list[str] = []
        scanned = 0
        while scanned < command.limit:
            page = await self.cognition.records(ctx, query)
            for item in page.items:
                record = item.record
                scanned += 1
                if not isinstance(record.body, MemoryBody) or record.body.role != 'declarative':
                    continue
                raw = record.body.structured if isinstance(record.body.structured, dict) else {}
                original = raw.get('payload') if isinstance(raw.get('payload'), dict) else raw
                fields = {**record.body.metadata, **TypeAdapter(dict[str, JsonValue]).validate_python(original)}
                explicit = next((fields[key] for key in ('event_time', 'event_time_start', 'time_start', 'timestamp') if fields.get(key) is not None), None)
                event = parse_source_time(explicit) if record.times.event is None else None
                observation = parse_source_time(fields.get('observed_at') if fields.get('observed_at') is not None else fields.get('created_at') if not command.no_created_fallback else None) if record.times.observation is None else None
                if event is None and observation is None:
                    continue
                candidates.append(record.reference.exact())
                if not command.dry_run:
                    async with self.authority.write(ctx, command.operation_id + ':' + record.reference.id, operation_kind='memory.source_times', scope=ctx.namespace_id, payload={'reference': record.reference.model_dump(), 'event': event, 'observation': observation}) as tx:
                        if tx.replayed:
                            updated.append(str(record.reference.id))
                        else:
                            result = await self.memories.revise_source_times_in(tx, ctx, record, event=event, observation=observation)
                            tx.result(result)
                            updated.append(result.reference.id)
                if scanned >= command.limit:
                    break
            if page.next_cursor is None or scanned >= command.limit:
                break
            query = query.model_copy(update={'cursor': page.next_cursor, 'limit': min(command.limit - scanned, 1000)})
        return TypeAdapter(dict[str, JsonValue]).validate_python({'dry_run': command.dry_run, 'scanned': scanned, 'candidates': len(candidates), 'updated': len(updated), 'references': candidates})

    async def prepare(self, ctx: CallContext, command: ImportDataset):
        if any((document.material_id is None for document in command.inputs)):
            raise DomainError('dataset_original_required', 'Dataset conversion requires captured uploaded originals')
        documents = tuple((document.model_copy(update={'origin': {**document.origin, 'dataset_ref': document.origin.get('dataset_ref') or 'operation:' + command.operation_id}}) for document in command.inputs))
        freeze = FreezeCatalog.model_validate({**command.model_dump(mode='json', include=set(FreezeCatalog.model_fields)), 'operation_id': command.operation_id + ':freeze', 'record_units': True, 'inputs': [document.model_dump(mode='json') for document in documents]})
        return await self.imports.freeze_catalog(ctx, freeze)

    async def enqueue_in(self, tx: WriteTx, ctx: CallContext, command: ImportDataset, catalog_id: str) -> dict[str, JsonValue]:
        await self.imports.read_catalog_in(tx, ctx, catalog_id)
        options = command.model_dump(mode='json', exclude={'inputs'})
        options['material_ids'] = [document.material_id for document in command.inputs]
        job_id = 'dataset:' + sha256(tx.operation_key.encode()).hexdigest()
        options['job_id'] = job_id
        job = await tx.enqueue(JobSpec('nous.imports.dataset', 'formation', args=(ctx.model_dump(mode='json'), catalog_id, options), workflow_id=job_id))
        return {'job_id': job.workflow_id, 'catalog_id': catalog_id}

    async def submit(self, ctx: CallContext, command: ImportDataset) -> dict[str, JsonValue]:
        catalog = await self.prepare(ctx, command)
        async with self.authority.write(ctx, command.operation_id, operation_kind='dataset.import', scope=ctx.namespace_id, payload=command) as tx:
            if tx.replayed:
                return TypeAdapter(dict[str, JsonValue]).validate_python(tx.replay_result)
            return tx.result(await self.enqueue_in(tx, ctx, command, catalog.id))

    async def run(self, context: JsonValue, catalog_id: str, options: JsonValue) -> dict[str, JsonValue]:
        ctx = CallContext.model_validate(context)
        fields = TypeAdapter(dict[str, JsonValue]).validate_python(options)
        catalog = await self.imports.read_catalog(ctx, catalog_id)
        if fields.get('dry_run'):
            return {'catalog_id': catalog_id, 'dry_run': True, 'candidates': len(catalog.units), 'errors': [error.model_dump(mode='json') for error in catalog.errors], 'updated': 0, 'sample_unit_ids': [unit.id for unit in catalog.units[:int(str(fields.get('preview_limit') or 20))]]}
        operation = str(fields['operation_id'])
        if len(catalog.errors) > int(str(fields.get('max_errors') or 0)):
            raise DomainError('dataset_input_invalid', 'Dataset contains undecodable source records', errors=[error.model_dump(mode='json') for error in catalog.errors])
        result = await self.imports.process_catalog(ctx, ProcessCatalog(operation_id=operation + ':process', catalog_id=catalog_id, llm_enabled=bool(fields.get('llm_enabled')), max_errors=int(str(fields.get('max_errors') or 0)) - len(catalog.errors), vectors=TypeAdapter(Literal['disabled', 'metadata_allowed', 'required']).validate_python(fields.get('vectors'))), attempt_job_id=str(fields['job_id']))
        response = TypeAdapter(dict[str, JsonValue]).validate_python({'catalog_id': catalog_id, 'output_ids': [output.id for output in result.outputs], 'references': [reference.model_dump(mode='json') for output in result.outputs for reference in output.cognition], 'source_material_ids': fields['material_ids'], 'updated': len(result.outputs), 'errors': [error.model_dump(mode='json') for error in (*catalog.errors, *result.failures)]})
        if not fields.get('export_bundle'):
            return response
        stable_payload = {'catalog_id': catalog_id}
        receipt = await self.authority.receipt(ctx, operation + ':archive', operation_kind='dataset.archive', scope=ctx.namespace_id, payload=stable_payload)
        if receipt is not None:
            return TypeAdapter(dict[str, JsonValue]).validate_python(receipt.result)
        materials = TypeAdapter(tuple[str, ...]).validate_python(fields['material_ids'])
        if not materials:
            raise DomainError('dataset_empty', 'Dataset conversion has no actual source records')
        archive = await self.bundles.export_archive(ctx, BundleRequest(selector=MaterialSelection(material_ids=materials), package=PackageDescriptor(id='dataset:' + operation, name=str(fields.get('name') or 'dataset'))))
        lease = await self.content.publish_bytes(archive.data, 'application/zip')
        try:
            async with self.authority.write(ctx, operation + ':archive', operation_kind='dataset.archive', scope=ctx.namespace_id, payload=stable_payload) as tx:
                if tx.replayed:
                    return TypeAdapter(dict[str, JsonValue]).validate_python(tx.replay_result)
                for identifier in materials:
                    await self.material.read_in(tx, ctx, identifier)
                document = BlobDocument(filename='dataset.nous.zip', media_type='application/zip', source_type='dataset_archive', origin={'catalog_id': catalog_id})
                artifact = await self.material.put_blob_in(tx, ctx, lease, document, MediaInfo(kind='document', media_type='application/zip'), required_source_ids=tuple((f'material:{identifier}' for identifier in materials)))
                await self.material.observe_in(tx, ctx, artifact['id'], document)
                return tx.result({**response, 'artifact_material_id': artifact['id'], 'artifact_filename': 'dataset.nous.zip', 'artifact_content_hash': artifact['content_hash'], 'artifact_bytes': artifact['byte_len'], 'artifact_format': 'nous.subject'})
        finally:
            await self.content.release(lease)
