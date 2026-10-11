# Protobuf Contracts

[返回目录](../INDEX.md)

`proto/` is the canonical cross-language wire-contract source for Nous Core, Nous Kernel, and the official Client. Rust and TypeScript bindings are generated from these definitions.

- [`nous/wave/v1alpha1/`](nous/wave/v1alpha1/): public Subject, Cognition, Memory, Material, Identity, Resource, Topology, System, and Model contracts.
- [`nous/wave/kernel/v1alpha1/`](nous/wave/kernel/v1alpha1/): private Core-to-Kernel contracts.
- [`ErrorDetail`](nous/wave/v1alpha1/errors.proto): stable business codes, owner context and recovery guidance; independent of display messages and generic transport categories.
- `proto/buf.yaml` / `.config/buf.gen.yaml` define module and generation inputs.
