from grpc.aio import Server

from .health import aio

def add_HealthServicer_to_server(servicer: aio.HealthServicer, server: Server) -> None: ...
