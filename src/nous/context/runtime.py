"""Persistent host history and the single actual model-input construction owner."""
from __future__ import annotations
import json
from dataclasses import asdict
from datetime import UTC, datetime
from hashlib import sha256
from uuid import uuid4
import sqlalchemy as sa
from pydantic import JsonValue,TypeAdapter
from nous_sdk.contracts import BlobDocument,CallContext,DomainError,MediaReference
from nous_sdk.contracts.context import ChangeContextSession,ChangeResidents,ChangeWorkContext,ContextAdmissionCommand,ContextInvocationOutput,ContextPrepare,ContextProjection,ContextReport,ContextResidentAdmission,ContextTrack,MediaTarget
from nous_sdk.contracts.derivation import CreateRepresentation,DerivationInput,DerivationProducer,DerivationQuality,TextRepresentation
from nous_sdk.contracts.imports import FormObservation
from nous_sdk.contracts.runtime import HostEvent,RuntimeOpen,RuntimeBindingInfo,RuntimePrepare,RuntimePreparation,RuntimeSent,RuntimeComplete,RuntimeResult,RuntimeTool,RuntimeBlock,RuntimeRead
from nous_sdk.contracts.model_measurement import ModelMeasurement
from nous.identity.access import require_namespace_in
from nous.platform.persistence import Authority,metadata
from nous.platform.persistence.authority import canonical_json
from nous.platform.content import ContentStore
from nous.platform.concurrency import owned_thread
from nous.material.service import MaterialService
from nous.material.derived import DerivedService
from nous.imports.service import ImportService
from nous.platform.models import ModelService
from .service import ContextService
from .host_input import HostBlock,HostInput,block_identity,clean_messages,message_role,message_text,fingerprint,response_message,completed_prefix,construct

bindings=sa.Table('runtime_bindings',metadata,sa.Column('namespace_id',sa.String,primary_key=True),sa.Column('caller_id',sa.String,primary_key=True),sa.Column('session_id',sa.String,primary_key=True),sa.Column('instance_id',sa.String,primary_key=True),sa.Column('record',sa.JSON,nullable=False))
events=sa.Table('runtime_events',metadata,sa.Column('namespace_id',sa.String,primary_key=True),sa.Column('session_id',sa.String,primary_key=True),sa.Column('event_id',sa.String,primary_key=True),sa.Column('record',sa.JSON,nullable=False),sa.Column('commit_seq',sa.Integer,nullable=False))
calls=sa.Table('runtime_calls',metadata,sa.Column('id',sa.String,primary_key=True),sa.Column('namespace_id',sa.String,nullable=False,index=True),sa.Column('caller_id',sa.String,nullable=False),sa.Column('session_id',sa.String,nullable=False),sa.Column('instance_id',sa.String,nullable=False),sa.Column('invocation_id',sa.String,nullable=False),sa.Column('record',sa.JSON,nullable=False),sa.Column('commit_seq',sa.Integer,nullable=False),sa.UniqueConstraint('namespace_id','caller_id','session_id','instance_id','invocation_id'))
blocks=sa.Table('runtime_blocks',metadata,sa.Column('id',sa.String,primary_key=True),sa.Column('namespace_id',sa.String,nullable=False,index=True),sa.Column('record',sa.JSON,nullable=False))
generations=sa.Table('runtime_generations',metadata,sa.Column('id',sa.String,primary_key=True),sa.Column('namespace_id',sa.String,nullable=False,index=True),sa.Column('session_id',sa.String,nullable=False),sa.Column('record',sa.JSON,nullable=False))
source_keys=sa.Table('runtime_source_keys',metadata,sa.Column('namespace_id',sa.String,primary_key=True),sa.Column('session_id',sa.String,primary_key=True),sa.Column('number',sa.Integer,primary_key=True),sa.Column('target',sa.Text,nullable=False),sa.UniqueConstraint('namespace_id','session_id','target'))
_OBJECT=TypeAdapter(dict[str,JsonValue])

class RuntimeService:
    def __init__(self,authority: Authority,contexts: ContextService,materials: MaterialService,content: ContentStore,derivations: DerivedService,imports: ImportService,models: ModelService):
        self.authority,self.contexts,self.materials,self.content,self.derivations,self.imports,self.models=authority,contexts,materials,content,derivations,imports,models

    def _where(self,ctx,info):
        return sa.and_(bindings.c.namespace_id==ctx.namespace_id,bindings.c.caller_id==ctx.caller_id,bindings.c.session_id==info.session,bindings.c.instance_id==info.instance)

    async def open(self,ctx: CallContext,command: RuntimeOpen) -> RuntimeBindingInfo:
        session=await self.contexts.open_session(ctx,command.operation_id+':session',command.session)
        if session.closed:raise DomainError('session_closed','Open a new session after closing this conversation')
        work=command.work.removeprefix('work:') if command.work else None
        if work is not None:
            await self.contexts.read_work(ctx,work)
            if session.active_work_context_id!=work:
                await self.contexts.change_session(ctx,ChangeContextSession(operation_id=command.operation_id+':work',session_id=session.id,expected_revision=session.revision,active_work_context_id=work))
        info=RuntimeBindingInfo(session=command.session,instance=command.instance,consumer=command.consumer,work=work,usage=command.usage)
        async with self.authority.write(ctx,command.operation_id,operation_kind='runtime.open',scope=ctx.namespace_id,payload=command) as tx:
            if tx.replayed:return RuntimeBindingInfo.model_validate(tx.replay_result)
            await require_namespace_in(tx,ctx)
            state=await tx.connection.scalar(sa.select(bindings.c.record).where(self._where(ctx,info)))
            if state is None:
                state={'history':[],'generation':str(uuid4()),'last_call_ref':None,'current_call':None,'model':None,'format':None}
                await tx.connection.execute(bindings.insert().values(namespace_id=ctx.namespace_id,caller_id=ctx.caller_id,session_id=info.session,instance_id=info.instance,record={**state,'info':info.model_dump(mode='json'),'form_long_term':command.form_long_term,'context_window_tokens':command.context_window_tokens,'output_reserve':command.output_reserve}))
            else:
                state.update(info=info.model_dump(mode='json'),form_long_term=command.form_long_term,context_window_tokens=command.context_window_tokens,output_reserve=command.output_reserve)
                await tx.connection.execute(bindings.update().where(self._where(ctx,info)).values(record=state))
            return tx.result(info)

    async def _binding(self,ctx,info):
        async with self.authority.read(ctx) as view:
            await require_namespace_in(view,ctx)
            state=await view.connection.scalar(sa.select(bindings.c.record).where(self._where(ctx,info)))
        if state is None:raise DomainError('runtime_binding_not_found','Open the runtime before preparing a model request')
        if RuntimeBindingInfo.model_validate(state['info'])!=info:raise DomainError('runtime_binding_conflict','The opened runtime has a different work or usage selection')
        return state

    async def _raw(self,ctx,ref):
        reference=MediaReference.model_validate(ref)
        material=await self.materials.read(ctx,reference.id)
        data=await owned_thread(lambda:self.content.path_for(material['content_hash']).read_bytes())
        return json.loads(data)

    async def _observe_message(self,ctx,info,message,format,identity,form):
        role=message_role(message,format);text=message_text(message,format)
        response_id=message.get('provider_response_id') if format=='pydantic-ai' else None
        event_id='response:'+format+':'+response_id if isinstance(response_id,str) else identity
        raw=message;tool_id=None
        if role=='tool':
            parts=message.get('parts',[]) if format=='pydantic-ai' else [message]
            tool_parts=[part for part in parts if isinstance(part,dict) and isinstance(part.get('tool_call_id'),str)]
            if len(tool_parts)==1:
                part=tool_parts[0];tool_id=part['tool_call_id'];event_id='tool:'+tool_id
                raw={'tool_call_id':tool_id,'result':part.get('content')}
                text=raw['result'] if isinstance(raw['result'],str) else canonical_json(raw['result'])
        event=await self._capture(ctx,info,HostEvent(event_id=event_id,role=role,payload=raw,text=text,tool_call_id=tool_id,response_id=response_id),form=form and bool(text))
        wire=event
        if raw is not message:
            wire=await self._capture(ctx,info,HostEvent(event_id='wire:'+identity,role='system',payload=message))
        return {'fingerprint':fingerprint(message,format),'event_id':event_id,'role':role,'wire_source':wire['raw_source'],'source':event['source'],'payload_kind':'message','format':format}

    async def _history(self,ctx,command,state):
        incoming=clean_messages(command.request,command.format)
        async with self.authority.read(ctx) as view:
            for message in command.request.get('messages',[]):
                marker=block_identity(message,command.format)
                if marker is not None:
                    owner=await view.connection.scalar(sa.select(blocks.c.namespace_id).where(blocks.c.id==marker))
                    if owner!=ctx.namespace_id:raise DomainError('runtime_history_namespace_mismatch','Cognitive history belongs to another Namespace')
        signatures=[fingerprint(message,command.format) for message in incoming]
        prior=state['history']
        same=command.format==state['format'] and signatures[:len(prior)]==[item['fingerprint'] for item in prior]
        if command.history_mode=='append':same=command.format==state['format'];new_messages=incoming;history=list(prior) if same else []
        elif same:new_messages=incoming[len(prior):];history=list(prior)
        else:new_messages=incoming;history=[]
        for index,message in enumerate(new_messages):
            history.append(await self._observe_message(ctx,command.binding,message,command.format,'host:'+command.invocation_id+':'+str(index),bool(state['form_long_term'])))
        actual=[]
        for entry in history:
            payload=await self._raw(ctx,entry['wire_source'])
            message=response_message(payload,entry['format']) if entry['payload_kind']=='response' else payload
            actual.append(message)
        return history,actual,not same

    async def _keys(self,ctx,info,projection,operation_id):
        targets={canonical_json(target) for segment in projection.segments for target in segment.sources}
        async with self.authority.write(ctx,operation_id,operation_kind='runtime.source_keys',scope=info.session,payload={'targets':sorted(targets)}) as tx:
            if tx.replayed:return dict(tx.replay_result)
            rows=(await tx.connection.execute(sa.select(source_keys.c.number,source_keys.c.target).where(source_keys.c.namespace_id==ctx.namespace_id,source_keys.c.session_id==info.session))).all()
            known={row.target:'c'+str(row.number) for row in rows};number=max((row.number for row in rows),default=0)
            for target in sorted(targets-known.keys()):
                number+=1;known[target]='c'+str(number)
                await tx.connection.execute(source_keys.insert().values(namespace_id=ctx.namespace_id,session_id=info.session,number=number,target=target))
            return tx.result({target:known[target] for target in targets})

    async def _call(self,ctx,ref):
        async with self.authority.read(ctx) as view:
            value=await view.connection.scalar(sa.select(calls.c.record).where(calls.c.id==ref.removeprefix('call:'),calls.c.namespace_id==ctx.namespace_id,calls.c.caller_id==ctx.caller_id))
        if value is None:raise DomainError('runtime_call_not_found','Runtime call is unavailable')
        return value

    async def _prepared(self,ctx,record):
        request=await self._raw(ctx,record['prepared_request_ref'])
        result=RuntimePreparation(call_ref=record['call_ref'],binding=record['binding'],invocation_id=record['invocation_id'],projection=record['projection'],format=record['format'],request=request,request_hash=record['request_hash'],history_generation=record['generation'],stable_prefix_fingerprint=record['stable_prefix_fingerprint'],input_bindings=record['input_bindings'],blocks=record['manifest'],measurement=record['measurement'],working_source=record['working_source'],replayed_output=await self._raw(ctx,record['output_raw_source']) if record['state']=='completed' and record.get('output_raw_source') else None)
        return result

    async def prepare(self,ctx: CallContext,command: RuntimePrepare) -> RuntimePreparation:
        async with self.authority.read(ctx) as view:
            existing=await view.connection.scalar(sa.select(calls.c.record).where(calls.c.namespace_id==ctx.namespace_id,calls.c.caller_id==ctx.caller_id,calls.c.session_id==command.binding.session,calls.c.instance_id==command.binding.instance,calls.c.invocation_id==command.invocation_id))
        incoming_hash=sha256(canonical_json({'request':command.request,'format':command.format,'history_mode':command.history_mode}).encode()).hexdigest()
        if existing is not None:
            if existing['state'] in {'failed','cancelled'}:raise DomainError('runtime_invocation_closed','A finished failed call requires a new invocation identity')
            if existing['incoming_hash']!=incoming_hash:raise DomainError('runtime_invocation_conflict','Invocation identity selects a different host input')
            return await self._prepared(ctx,existing)
        state=await self._binding(ctx,command.binding)
        if state['current_call'] is not None:
            prior=await self._call(ctx,state['current_call'])
            if prior['state']=='sent':raise DomainError('runtime_call_in_flight','Complete or reconcile the sent call before continuing this runtime instance')
            if prior['state']=='prepared':await self.contexts.cancel_preparation(ctx,prior['context_preparation_id'],command.operation_id+':abandon')
        for event in command.events:await self._capture(ctx,command.binding,event,form=bool(state['form_long_term']) and bool(event.text))
        history,host_messages,rebase=await self._history(ctx,command,state)
        model=command.request.get('model');reset=rebase or model!=state['model']
        generation=str(uuid4()) if reset else state['generation']
        present=tuple(MediaTarget(reference=MediaReference.model_validate(entry['source'])) for entry in history)
        await self._reside(ctx,command.binding,tuple(target.reference for target in present[-8:]),'Actual host interaction')
        input_event=await self._capture(ctx,command.binding,HostEvent(event_id='input:'+command.invocation_id,role='system',payload=command.request))
        baseline=await self.models.measure_host(ctx,{**command.request,'format':command.format},context_window=state['context_window_tokens'],output_reserve=state['output_reserve'])
        pressure=baseline.context_window is not None and baseline.input_tokens+baseline.output_reserve>=int(baseline.context_window*.85)
        boundary=completed_prefix(host_messages,command.format);working_source=None
        if pressure and boundary:
            prefix=host_messages[:boundary]
            text='\n\n'.join(message_role(item,command.format)+': '+message_text(item,command.format) for item in prefix if message_role(item,command.format)!='system')
            if text:
                working=await self._capture(ctx,command.binding,HostEvent(event_id='history:'+command.invocation_id,role='system',payload={'messages':prefix},text=text))
                working_source=MediaReference.model_validate(working['source'])
        question=next((message_text(message,command.format) for message in reversed(host_messages) if message_role(message,command.format)=='user'),'')
        usage=command.binding.usage.model_copy(update={'intent':question}) if question else command.binding.usage
        track=await self.contexts.read_track(ctx,ContextTrack(session_id=command.binding.session,consumer_id=command.binding.consumer,instance_id=command.binding.instance))
        preparation=await self.contexts.prepare(ctx,ContextPrepare(operation_id=command.operation_id+':context',session_id=command.binding.session,consumer_id=command.binding.consumer,instance_id=command.binding.instance,usage=usage,query=command.query,known_cursor=None if reset else track.projection.cursor if track.projection else None,present_sources=present,working_sources=(MediaTarget(reference=working_source),) if working_source else (),trigger='budget' if pressure else command.trigger,include_optional=baseline.complete,max_items=command.max_items,max_text_bytes=command.max_text_bytes))
        async with self.authority.read(ctx) as view:
            stored=await self.contexts.read_preparation_in(view,ctx,preparation.id);projection=ContextProjection.model_validate(stored['full_projection'])
            prior_blocks=()
        reset=reset or preparation.response.kind=='reset'
        assigned=await self._keys(ctx,command.binding,projection,command.operation_id+':keys')
        prior_request=None;prior_count=0
        if state['last_call_ref'] is not None and not reset:
            previous=await self._call(ctx,state['last_call_ref'])
            prior_request=await self._raw(ctx,previous['prepared_request_ref'])
            prior_count=len(previous['host_history'])
            prior_blocks=tuple(HostBlock(**block) for block in previous['block_plan'])
        summary=None
        decision=projection.steward_decision
        if pressure and boundary and working_source and decision and decision.history_summary and MediaTarget(reference=working_source) in decision.covered_working_sources:
            summary=decision.history_summary;generation=str(uuid4());reset=True
        built=construct(command.request,command.format,host_messages,projection,assigned,namespace_id=ctx.namespace_id,generation=generation,prior_request=prior_request,prior_host_count=prior_count,prior_blocks=prior_blocks,reset=reset,history_summary=summary,history_boundary=boundary)
        measurement=await self.models.measure_host(ctx,{**built.request,'format':command.format},context_window=state['context_window_tokens'],output_reserve=state['output_reserve'])
        if measurement.complete and measurement.context_window is not None and measurement.input_tokens+measurement.output_reserve>measurement.context_window:
            await self.contexts.cancel_preparation(ctx,preparation.id,command.operation_id+':budget-wait')
            raise DomainError('runtime_compression_waiting','The current history needs the configured compression model before it can be sent',input_tokens=measurement.input_tokens,context_window=measurement.context_window)
        actual=await self._capture(ctx,command.binding,HostEvent(event_id='prepared:'+command.invocation_id,role='system',payload=built.request))
        identity=str(uuid4());call_ref='call:'+identity
        manifest=tuple(RuntimeBlock(block_id=block.id,role=block.role,segment_keys=block.segment_keys,input_bindings=block.bindings,stable=block.stable) for block in built.blocks)
        record={'call_ref':call_ref,'binding':command.binding.model_dump(mode='json'),'invocation_id':command.invocation_id,'incoming_hash':incoming_hash,'projection':projection.model_dump(mode='json'),'context_preparation_id':preparation.id,'format':command.format,'prepared_request_ref':actual['raw_source'],'request_hash':sha256(canonical_json(built.request).encode()).hexdigest(),'generation':generation,'stable_prefix_fingerprint':built.stable_prefix_fingerprint,'input_bindings':built.input_bindings,'manifest':[item.model_dump(mode='json') for item in manifest],'block_plan':[asdict(block) for block in built.blocks],'measurement':measurement.model_dump(mode='json'),'working_source':working_source.model_dump(mode='json') if working_source else None,'host_sources':[target.reference.model_dump(mode='json') for target in present],'state':'prepared','host_history':history}
        state.update(history=history,generation=generation,current_call=call_ref,model=model,format=command.format)
        async with self.authority.write(ctx,command.operation_id,operation_kind='runtime.prepare',scope=ctx.namespace_id,payload={'incoming_hash':incoming_hash,'binding':command.binding,'invocation_id':command.invocation_id}) as tx:
            for block in built.blocks:
                exists=await tx.connection.scalar(sa.select(blocks.c.id).where(blocks.c.id==block.id))
                if exists is None:await tx.connection.execute(blocks.insert().values(id=block.id,namespace_id=ctx.namespace_id,record={'role':block.role,'text':block.text,'stable':block.stable}))
            if generation!=state.get('persisted_generation'):
                await tx.connection.execute(generations.insert().values(id=generation,namespace_id=ctx.namespace_id,session_id=command.binding.session,record={'previous':state.get('persisted_generation'),'history':history,'summary_source':working_source.model_dump(mode='json') if summary and working_source else None}))
                state['persisted_generation']=generation
            await tx.connection.execute(calls.insert().values(id=identity,namespace_id=ctx.namespace_id,caller_id=ctx.caller_id,session_id=command.binding.session,instance_id=command.binding.instance,invocation_id=command.invocation_id,record=record,commit_seq=tx.commit_seq))
            await tx.connection.execute(bindings.update().where(self._where(ctx,command.binding)).values(record=state));tx.result({'call_ref':call_ref})
        return await self._prepared(ctx,record)

    async def read(self,ctx: CallContext,command: RuntimeRead):
        from nous_sdk.contracts.public import ReadRequest
        from nous_sdk.contracts.targets import ContextTarget
        await self._binding(ctx,command.binding)
        target=None
        if command.ref.startswith('c') and command.ref[1:].isdigit():
            async with self.authority.read(ctx) as view:
                raw=await view.connection.scalar(sa.select(source_keys.c.target).where(source_keys.c.namespace_id==ctx.namespace_id,source_keys.c.session_id==command.binding.session,source_keys.c.number==int(command.ref[1:])))
            if raw is None:raise DomainError('reference_not_found','Runtime source reference does not exist in this session')
            target=TypeAdapter(ContextTarget).validate_json(raw)
        return await self.contexts.reading.read(ctx,ReadRequest(ref=command.ref,sources=command.sources,max_chars=command.max_chars),target=target)

    async def sent(self,ctx: CallContext,command: RuntimeSent) -> RuntimeResult:
        value=await self._call(ctx,command.call_ref)
        if value['request_hash']!=command.request_hash:raise DomainError('runtime_input_changed','Send the request constructed for this prepared call')
        if value['state'] in {'sent','completed'}:return RuntimeResult(call_ref=command.call_ref,state=value['state'])
        prepared=await self._prepared(ctx,value)
        admission=await self.contexts.admit_managed(ctx,ContextAdmissionCommand(operation_id=command.operation_id+':admit',preparation_id=value['context_preparation_id'],invocation_id=prepared.invocation_id,applied_segment_ids=tuple(segment.key for segment in prepared.projection.segments),rendered_digest=prepared.request_hash,history_generation=prepared.history_generation,measurement=prepared.measurement.host_measurement(),host_sources=tuple(MediaTarget(reference=MediaReference.model_validate(ref)) for ref in value['host_sources'])))
        await self._apply_steward(ctx,prepared)
        state=await self._binding(ctx,prepared.binding)
        state.update(last_call_ref=command.call_ref)
        value.update(state='sent',admission_id=admission.id,sent_evidence=command.evidence)
        async with self.authority.write(ctx,command.operation_id,operation_kind='runtime.sent',scope=ctx.namespace_id,payload=command) as tx:
            if tx.replayed:return RuntimeResult.model_validate(tx.replay_result)
            await tx.connection.execute(calls.update().where(calls.c.id==command.call_ref.removeprefix('call:')).values(record=value,commit_seq=tx.commit_seq))
            await tx.connection.execute(bindings.update().where(self._where(ctx,prepared.binding)).values(record=state))
            return tx.result(RuntimeResult(call_ref=command.call_ref,state='sent'))

    async def complete(self,ctx: CallContext,command: RuntimeComplete) -> RuntimeResult:
        value=await self._call(ctx,command.call_ref);prepared=await self._prepared(ctx,value)
        if command.outcome=='completed' and command.output is None:raise DomainError('runtime_output_required','Capture the complete actual model response')
        if value['state']=='completed' and command.outcome!='completed':raise DomainError('runtime_call_completed','A completed response cannot be replaced by a failure')
        output_digest=sha256(canonical_json(command.output).encode()).hexdigest() if command.output is not None else None
        if value.get('result') is not None and value['state']==command.outcome and value.get('output_digest')==output_digest and value.get('error')==command.error:return RuntimeResult.model_validate(value['result'])
        if value['state']=='prepared' and command.output is None:await self.contexts.cancel_preparation(ctx,value['context_preparation_id'],command.operation_id+':not-sent')
        output=None;message=None
        if command.output is not None:
            message=response_message(command.output,prepared.format)
            event_id='response:'+prepared.format+':'+(command.response_id or prepared.invocation_id)+':'+command.outcome
            state=await self._binding(ctx,prepared.binding)
            output=await self._capture(ctx,prepared.binding,HostEvent(event_id=event_id,role='assistant',payload=command.output,text=command.text or message_text(message,prepared.format),response_id=command.response_id),form=bool(state['form_long_term']))
        if value['state']=='prepared' and command.output is not None:
            # A returned native response establishes sending even when the caller lost its sent acknowledgement.
            try:await self.sent(ctx,RuntimeSent(operation_id=prepared.invocation_id+':observed-sent',call_ref=command.call_ref,request_hash=prepared.request_hash,evidence='response_observed'));value=await self._call(ctx,command.call_ref)
            except DomainError as error:
                if error.code not in {'source_unavailable','context_source_changed','context_not_prepared'}:raise
        notices=[];report=None
        if value.get('admission_id'):
            try:
                report=await self.contexts.report_managed(ctx,ContextReport(operation_id=command.operation_id+':report',admission_id=value['admission_id'],invocation_id=prepared.invocation_id,outcome='failed' if command.outcome=='unknown_outcome' else command.outcome,occurred_at=datetime.fromisoformat(output['recorded_at']) if output else datetime.now(UTC),output=ContextInvocationOutput(raw_source=MediaReference.model_validate(output['raw_source']),usage=command.usage,model=command.model,provider=command.provider,response_id=command.response_id,status=command.outcome) if output else None))
            except DomainError as error:
                if error.code not in {'source_unavailable','context_source_changed','cognition_not_found'}:raise
                notices.append(error.code)
        state=await self._binding(ctx,prepared.binding)
        if state['current_call']==command.call_ref:
            if output is not None and message is not None:
                entry={'fingerprint':fingerprint(message,prepared.format),'event_id':output['event_id'],'role':'assistant','wire_source':output['raw_source'],'source':output['source'],'payload_kind':'response','format':prepared.format}
                if not state['history'] or state['history'][-1]['event_id']!=entry['event_id']:state['history'].append(entry)
                await self._reside(ctx,prepared.binding,(MediaReference.model_validate(output['source']),),'Actual model response')
            state['current_call']=None
        result=RuntimeResult(call_ref=command.call_ref,state=command.outcome,output_ref=MediaReference.model_validate(output['source']) if output else None,report_ref=report.id if report else None,notices=tuple(notices))
        value.update(state=command.outcome,output_digest=output_digest,output_raw_source=output['raw_source'] if output else None,result=result.model_dump(mode='json'),usage=command.usage,error=command.error)
        async with self.authority.write(ctx,command.operation_id,operation_kind='runtime.complete',scope=ctx.namespace_id,payload={'call_ref':command.call_ref,'outcome':command.outcome,'output_digest':output['payload_digest'] if output else None,'error':command.error}) as tx:
            if tx.replayed:return RuntimeResult.model_validate(tx.replay_result)
            await tx.connection.execute(calls.update().where(calls.c.id==command.call_ref.removeprefix('call:')).values(record=value,commit_seq=tx.commit_seq))
            await tx.connection.execute(bindings.update().where(self._where(ctx,prepared.binding)).values(record=state))
            return tx.result(result)
    async def _capture(
        self, ctx: CallContext, info: RuntimeBindingInfo, event: HostEvent, *, form: bool = False
    ):
        payload = canonical_json(event.payload).encode()
        digest = sha256(payload).hexdigest()
        lease = await self.content.publish_bytes(payload, "application/json")
        try:
            async with self.authority.write(
                ctx,
                "runtime-event:" + info.session + ":" + event.event_id,
                operation_kind="runtime.event",
                scope=ctx.namespace_id,
                payload={
                    "event_id": event.event_id,
                    "role": event.role,
                    "payload_digest": digest,
                    "text_digest": sha256(event.text.encode()).hexdigest(),
                },
            ) as tx:
                if tx.replayed:
                    value = _OBJECT.validate_python(tx.replay_result)
                    await self.materials.read_in(tx, ctx, str(value["material_id"]))
                    return value
                existing = await tx.connection.scalar(
                    sa.select(events.c.record).where(
                        events.c.namespace_id == ctx.namespace_id,
                        events.c.session_id == info.session,
                        events.c.event_id == event.event_id,
                    )
                )
                if existing is not None:
                    if existing["payload_digest"] != digest:
                        raise DomainError(
                            "runtime_event_conflict",
                            "Host event identity has different actual content",
                        )
                    await self.materials.read_in(tx, ctx, existing["material_id"])
                    return tx.result(existing)
                document = BlobDocument(
                    filename=event.role + "-event.json",
                    media_type="application/json",
                    source_type="runtime_" + event.role,
                    source=event.event_id,
                    origin={"caller_id": ctx.caller_id, "session_id": info.session},
                )
                admission = await self.materials.admit_blob_in(tx, ctx, lease, document)
                raw = MediaReference(
                    kind="material",
                    id=admission.material["id"],
                    observation_id=admission.event["id"],
                )
                source = raw
                if event.text:
                    signature = sha256(b"nous.runtime.host-text").hexdigest()
                    representation = await self.derivations.adopt_in(
                        tx,
                        admission.context,
                        CreateRepresentation(
                            kind="extracted_text",
                            strategy="preprocess",
                            inputs=(DerivationInput(ordinal=0, source=raw),),
                            payload=TextRepresentation(text=event.text[:1000000]),
                            producer=DerivationProducer(
                                signature=signature,
                                implementation="host-message-projection",
                                config_digest=signature,
                            ),
                            quality=DerivationQuality(truncated=len(event.text) > 1000000),
                        ),
                    )
                    source = representation.reference.media()
                value = {
                    "event_id": event.event_id,
                    "role": event.role,
                    "session_id": info.session,
                    "caller_id": ctx.caller_id,
                    "payload_digest": digest,
                    "material_id": raw.id,
                    "raw_source": raw.model_dump(mode="json"),
                    "source": source.model_dump(mode="json"),
                    "observation_id": admission.event["id"],
                    "response_id": event.response_id,
                    "tool_call_id": event.tool_call_id,
                    "commit_seq": tx.commit_seq,
                    "recorded_at": tx.recorded_at,
                }
                if form and event.role != "system":
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
                        session_id=info.session,
                        event_id=event.event_id,
                        record=value,
                        commit_seq=tx.commit_seq,
                    )
                )
                return tx.result(value)
        finally:
            await self.content.release(lease)


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
        session = await self.contexts.read_session(ctx, info.session)
        await self.contexts.change_residents(
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


    async def _apply_steward(self, ctx: CallContext, prepared: RuntimePreparation):
        decision = prepared.projection.steward_decision
        if decision is None:
            return
        if decision.evict:
            session = await self.contexts.read_session(ctx, prepared.binding.session)
            await self.contexts.change_residents(
                ctx,
                ChangeResidents(
                    operation_id=prepared.invocation_id + ":steward:evict",
                    session_id=session.id,
                    expected_revision=session.revision,
                    action="evict",
                    targets=decision.evict,
                ),
            )
        if decision.promote:
            session = await self.contexts.read_session(ctx, prepared.binding.session)
            await self.contexts.change_residents(
                ctx,
                ChangeResidents(
                    operation_id=prepared.invocation_id + ":steward:promote",
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
            )
        if prepared.binding.work and (decision.work_summary is not None or decision.unresolved):
            work = await self.contexts.read_work(ctx, prepared.binding.work)
            content = work.content.model_copy(
                update={
                    "context_text": decision.work_summary
                    if decision.work_summary is not None
                    else work.content.context_text,
                    "unresolved_questions": decision.unresolved,
                }
            )
            await self.contexts.change_work(
                ctx,
                ChangeWorkContext(
                    operation_id=prepared.invocation_id + ":steward:work",
                    work_context_id=work.id,
                    expected_revision=work.revision,
                    action="edit",
                    content=content,
                ),
            )
