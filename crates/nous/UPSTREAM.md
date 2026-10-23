# Reused source

The HTTP/MCP startup, authentication middleware shape, cancellation wiring,
stdio logging and tool router are adapted from the official
[Model Context Protocol Rust SDK](https://github.com/modelcontextprotocol/rust-sdk)
at commit `2a401c4ca3eb5880744d2e70fad8073a7fcfb2bc`:

- `examples/servers/src/counter_streamhttp.rs`
- `examples/servers/src/simple_auth_streamhttp.rs`
- `examples/servers/src/counter_stdio.rs`
- `examples/servers/src/common/counter.rs`

The adapted code is in `src/main.rs`, `src/api.rs` and `src/mcp.rs`. The SDK's
Apache-2.0 license is retained in [licenses/rmcp-LICENSE](licenses/rmcp-LICENSE).
Nous replaces the example's demo token and in-memory counter with protected,
scoped instance credentials and shared application operations.
