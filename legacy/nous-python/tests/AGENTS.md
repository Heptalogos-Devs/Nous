# Tests Organization

Core principles and values:

- **Follow AntiRLHF**: DO NOT formalize verification, and DO NOT rely on TDD
- **Watch for structural ossification, or retreat from refactoring**, that tests introduce
- Prefer tests designed from the **user's perspective**, along **long business-path flows**
- Prohibit shallow field tests, existence tests, and existence or consistency tests of text and documents
- When necessary, a dedicated test may protect fragile logic and complex behavior
- Test files are independent and do not depend on one another
- Organize by functional module, not by file type
- Do not reimplement shared, general logic; reuse is allowed
- During development, keep scaffolding, temporary probes, tests, and the like in `temp/`

Tests are organized into the following directories:

- `tests/python/`
- `tests/rust/`
- `tests/integration/`
- `tests/performance/`
- `tests/e2e/`
- `tests/cli/`
- `tests/platform/`
- `tests/*/shared/`

`native/` has its own tests. They stay with the crates and are not placed here.
