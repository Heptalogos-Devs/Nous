# Nous Kernel

[返回目录](../../INDEX.md)

Nous Kernel is the private Rust process. It composes Subject, Runtime, Material, Memory, Persistence, Object Store and Retrieval owners behind private authenticated Tonic/gRPC services.

- [Current implementation architecture](../../docs/architecture/current-implementation.md)
- [Kernel package manifest](Cargo.toml)
- [Protobuf source](../../proto/README.md)

The TypeScript Core owns public HTTP/Connect composition and consumer-facing orchestration. Kernel binds its own loopback ephemeral listener; the parent process discovers the endpoint.
