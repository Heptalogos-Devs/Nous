import grpc
from google.protobuf.message import Message

def to_status(status: Message) -> grpc.Status: ...
