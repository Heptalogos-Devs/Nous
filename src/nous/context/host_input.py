"""Construct one actual host request while preserving native message and tool groups."""
from __future__ import annotations
from copy import deepcopy
from dataclasses import dataclass
from hashlib import sha256
from typing import Literal
from uuid import uuid4
from pydantic import JsonValue
from pydantic_ai.messages import ModelMessagesTypeAdapter, ModelRequest, ModelResponse, SystemPromptPart, UserPromptPart, ToolCallPart, ToolReturnPart, RetryPromptPart
from nous_sdk.contracts import DomainError
from nous_sdk.contracts.context import ContextProjection
from nous.platform.persistence.authority import canonical_json
HostFormat=Literal['openai-chat','pydantic-ai']

@dataclass(frozen=True)
class HostBlock:
    id: str
    role: str
    text: str
    bindings: dict[str,dict]
    stable: bool=False
    segment_keys: tuple[str,...]=()

@dataclass(frozen=True)
class HostInput:
    request: dict[str,JsonValue]
    blocks: tuple[HostBlock,...]
    input_bindings: dict[str,dict]
    stable_prefix_fingerprint: str


def block_identity(message: dict,format: HostFormat) -> str | None:
    if format=='pydantic-ai':
        metadata=message.get('metadata') or {}
        marker=metadata.get('nous_runtime') if isinstance(metadata,dict) else None
        return marker.get('block_id') if isinstance(marker,dict) else None
    name=message.get('name')
    if isinstance(name,str) and name.startswith('nous_') and len(name)==37:
        return name[5:]
    return None


def clean_messages(request: dict,format: HostFormat) -> list[dict]:
    raw=request.get('messages',[])
    if not isinstance(raw,list):raise DomainError('host_request_invalid','Host request requires a message list')
    if format=='pydantic-ai':
        raw=ModelMessagesTypeAdapter.dump_python(ModelMessagesTypeAdapter.validate_python(raw),mode='json')
    return [deepcopy(message) for message in raw if block_identity(message,format) is None]


def message_role(message: dict,format: HostFormat) -> str:
    if format=='openai-chat':
        role=message.get('role')
        return 'system' if role=='developer' else str(role)
    if message.get('kind')=='response':return 'assistant'
    kinds={part.get('part_kind') for part in message.get('parts',[]) if isinstance(part,dict)}
    if 'user-prompt' in kinds:return 'user'
    if kinds.intersection({'tool-return','retry-prompt'}):return 'tool'
    return 'system'


def message_text(message: dict,format: HostFormat) -> str:
    if format=='pydantic-ai':
        parts=message.get('parts',[])
        values=[]
        for part in parts:
            if not isinstance(part,dict):continue
            kind=part.get('part_kind')
            if kind in {'text','thinking','system-prompt'} and isinstance(part.get('content'),str):values.append(part['content'])
            elif kind in {'user-prompt','tool-return','retry-prompt'}:
                content=part.get('content')
                if isinstance(content,str):values.append(content)
                elif isinstance(content,list):values.extend(item if isinstance(item,str) else str(item.get('text','')) for item in content if isinstance(item,(str,dict)))
                elif content is not None:values.append(canonical_json(content))
        return '\n'.join(values)
    content=message.get('content')
    if isinstance(content,str):return content
    if isinstance(content,list):return '\n'.join(str(part['text']) for part in content if isinstance(part,dict) and isinstance(part.get('text'),str))
    return ''


def model_visible_message(message: dict) -> dict:
    value=deepcopy(message)
    for field in ('metadata','timestamp','run_id','conversation_id','history_digest'):value.pop(field,None)
    for part in value.get('parts',[]):
        if isinstance(part,dict):
            for field in ('metadata','timestamp'):part.pop(field,None)
    return value


def fingerprint(message: dict,format: HostFormat) -> str:
    return sha256(canonical_json(model_visible_message(message)).encode()).hexdigest()


def response_message(output: JsonValue,format: HostFormat) -> dict:
    if not isinstance(output,dict):raise DomainError('host_response_invalid','Capture the actual native model response object')
    if format=='pydantic-ai':
        value=ModelMessagesTypeAdapter.validate_python([output])[0]
        if not isinstance(value,ModelResponse):raise DomainError('host_response_invalid','Expected a native model response')
        return ModelMessagesTypeAdapter.dump_python([value],mode='json')[0]
    choices=output.get('choices')
    if not isinstance(choices,list) or not choices or not isinstance(choices[0],dict) or not isinstance(choices[0].get('message'),dict):
        raise DomainError('host_response_invalid','Expected the complete Chat Completions response')
    return deepcopy(choices[0]['message'])


def completed_prefix(messages: list[dict],format: HostFormat) -> int:
    """Only older complete user turns can be replaced; the current tool loop remains native."""
    users=[index for index,message in enumerate(messages) if message_role(message,format)=='user']
    if not users:return 0
    boundary=users[-1]
    pending=set()
    if format=='pydantic-ai':
        native=ModelMessagesTypeAdapter.validate_python(messages[:boundary])
        for message in native:
            for part in message.parts:
                if isinstance(part,ToolCallPart):pending.add(part.tool_call_id)
                elif isinstance(part,(ToolReturnPart,RetryPromptPart)) and part.tool_call_id:pending.discard(part.tool_call_id)
    else:
        for message in messages[:boundary]:
            if message.get('role')=='assistant':pending.update(item['id'] for item in message.get('tool_calls',[]) if isinstance(item,dict) and isinstance(item.get('id'),str))
            elif message.get('role')=='tool':pending.discard(message.get('tool_call_id'))
    return 0 if pending else boundary


def _message(block: HostBlock,format: HostFormat,namespace_id: str,generation: str) -> dict:
    if format=='pydantic-ai':
        part=SystemPromptPart(block.text) if block.role=='system' else UserPromptPart(block.text)
        value=ModelRequest(parts=[part],metadata={'nous_runtime':{'namespace_id':namespace_id,'generation_id':generation,'block_id':block.id}})
        return ModelMessagesTypeAdapter.dump_python([value],mode='json')[0]
    return {'role':block.role,'name':'nous_'+block.id,'content':block.text}


def construct(request: dict,format: HostFormat,host_messages: list[dict],projection: ContextProjection,keys: dict[str,str], *, namespace_id: str,generation: str,prior_request: dict | None=None,prior_host_count: int=0,prior_blocks: tuple[HostBlock,...]=(),reset: bool=False,history_summary: str | None=None,history_boundary: int=0) -> HostInput:
    """The returned request, source map and manifest are one frozen sending plan."""
    messages=deepcopy(host_messages)
    blocks=list(prior_blocks) if prior_request is not None and not reset else []
    if prior_request is not None and not reset:
        messages=deepcopy(prior_request['messages'])+messages[prior_host_count:]
    if history_summary is not None:
        system=[message for message in host_messages[:history_boundary] if message_role(message,format)=='system']
        messages=system+deepcopy(host_messages[history_boundary:]);blocks=[]
        summary=HostBlock(uuid4().hex,'user',history_summary,{},False)
        index=len(system)
        messages.insert(index,_message(summary,format,namespace_id,generation));blocks.append(summary)
    existing_segments={key for block in blocks for key in block.segment_keys}
    stable=[];dynamic=[];stable_bindings={};dynamic_bindings={};stable_keys=[];dynamic_keys=[]
    for segment in projection.segments:
        bindings={keys[canonical_json(target)]:target.model_dump(mode='json') for target in segment.sources}
        if segment.key in existing_segments and not reset:continue
        labels=' '.join('['+key+']' for key in bindings)
        text=(labels+' '+segment.text).strip()
        if segment.required and segment.provenance.get('supply')=='current_personality':stable.append(segment.text);stable_bindings.update(bindings);stable_keys.append(segment.key)
        else:dynamic.append(text);dynamic_bindings.update(bindings);dynamic_keys.append(segment.key)
    if stable:
        stable_text='\n\n'.join(stable)
        block=HostBlock(sha256(canonical_json({'namespace_id':namespace_id,'text':stable_text}).encode()).hexdigest()[:32],'system',stable_text,stable_bindings,True,tuple(stable_keys))
        messages.insert(0,_message(block,format,namespace_id,generation));blocks.append(block)
    if dynamic:
        block=HostBlock(uuid4().hex,'user','\n\n'.join(dynamic),dynamic_bindings,False,tuple(dynamic_keys))
        # Appending after the whole pending result group never splits tool calls from returns.
        index=len(messages)
        if messages and message_role(messages[-1],format)=='user':index-=1
        messages.insert(index,_message(block,format,namespace_id,generation));blocks.append(block)
    value={**deepcopy(request),'messages':messages}
    mapping={key:target for block in blocks for key,target in block.bindings.items()}
    visible_stable=[model_visible_message(message) for message in messages if message_role(message,format)=='system' or isinstance(message.get('instructions'),str)]
    digest=sha256(canonical_json({'messages':visible_stable,'tools':value.get('tools',[])}).encode()).hexdigest()
    return HostInput(value,tuple(blocks),mapping,digest)
