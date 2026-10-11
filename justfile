set windows-shell := ["powershell", "-NoProfile", "-Command"]
export POSTGRESQL_VERSION := "18.6.0"

default:
    @just --list

fmt:
    cargo fmt --all -- --config-path .config/rustfmt.toml
    corepack pnpm format

check-fast:
    cargo fmt --all -- --config-path .config/rustfmt.toml --check
    corepack pnpm check:fast
    cargo clippy --workspace --all-targets --all-features -- -D warnings

check: check-fast
    corepack pnpm test
    cargo test --workspace --all-features -- --test-threads=1

audit:
    cargo deny --config .config/deny.toml check
    cargo shear --deny-warnings
    corepack pnpm run audit
    cargo dupes report
    osv-scanner scan source -r .

dev-prepare:
    corepack pnpm dev:prepare

smoke:
    cargo build -p nous-kernel
    corepack pnpm smoke

release-prepare:
    powershell -NoProfile -File scripts/release/build-kernel.ps1
    corepack pnpm release:notices

release:
    corepack pnpm assemble:portable

release-verify:
    corepack pnpm release:verify --bundle data/releases/windows-x64/current.zip
    corepack pnpm release:verify --bundle data/releases/windows-x64/current.zip --layout colocated
    corepack pnpm release:verify --bundle data/releases/windows-x64/current.zip --layout locator --relocate

clean-test-temp:
    powershell -NoProfile -File scripts/maintenance/cleanup_embedded_postgres.ps1
