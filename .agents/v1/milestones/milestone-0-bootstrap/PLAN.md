# Milestone 0: Repo Bootstrap and Guardrails

## Status: COMPLETE

**Completed:** 2026-01-07

---

## Overview

**Goal:** Create the foundational rails so that subsequent development is constrained to the correct patterns. This milestone establishes the crate structure, CI, feature flags, and a minimal "hello command" that exercises the engine lifecycle.

**Principle:** Follow the leader (SPEC.md + ARCHITECTURE.md), Simplicity, Reuse.

---

## Acceptance Gates - ALL PASSED

- [x] `cargo fmt --check` passes
- [x] `cargo clippy --all-targets --all-features` passes (warnings only for dead stub code)
- [x] `cargo test` passes (16 unit tests + 3 doctests)
- [x] `cargo doc --no-deps` succeeds
- [x] `cargo run -- hello` outputs "Hello from Lattice!"
- [x] `cargo run -- --debug hello` shows full engine lifecycle phases

---

## Implementation Summary

### Cargo.toml
- Fixed edition from `"2024"` to `"2021"`
- Added dependencies: clap, serde, serde_json, toml, anyhow, thiserror, git2, uuid, chrono
- Added dev-dependencies: assert_cmd, assert_fs, tempfile, insta, proptest, predicates
- Defined feature flags: `keychain`, `fault_injection`, `live_github_tests`
- Note: Used `git2` with `vendored-libgit2` feature to avoid OpenSSL dependency

### Directory Structure Created
```
src/
├── lib.rs              # Library root with module declarations
├── main.rs             # CLI entry point
├── cli/
│   ├── mod.rs          # CLI layer
│   ├── args.rs         # Clap derive structures
│   └── commands/
│       └── mod.rs      # Command dispatch
├── engine/
│   ├── mod.rs          # Engine orchestrator
│   ├── scan.rs         # Scanner (stub)
│   ├── gate.rs         # Gating (stub)
│   ├── plan.rs         # Planner (stub)
│   ├── exec.rs         # Executor (stub)
│   └── verify.rs       # Verification (stub)
├── core/
│   ├── mod.rs          # Core domain types
│   ├── types.rs        # BranchName, Oid, RefName, UtcTimestamp
│   ├── graph.rs        # Stack graph with cycle detection
│   ├── verify.rs       # Fast verify
│   ├── naming.rs       # Branch naming/slugify
│   ├── ops/
│   │   ├── mod.rs      # Operations
│   │   ├── journal.rs  # Journal types
│   │   └── lock.rs     # Repo lock
│   ├── metadata/
│   │   ├── mod.rs      # Metadata
│   │   ├── schema.rs   # BranchMetadataV1 with deny_unknown_fields
│   │   └── store.rs    # Metadata store
│   └── config/
│       ├── mod.rs      # Config loading
│       └── schema.rs   # GlobalConfig, RepoConfig
├── git/
│   ├── mod.rs          # Git interface
│   └── interface.rs    # Git operations via git2
├── forge/
│   ├── mod.rs          # Forge abstraction
│   └── traits.rs       # Forge trait + GitHubForge stub
├── secrets/
│   ├── mod.rs          # Secret storage
│   └── traits.rs       # SecretStore trait + FileSecretStore stub
├── doctor/
│   ├── mod.rs          # Doctor framework
│   └── issues.rs       # Issue catalog + HealthReport
└── ui/
    ├── mod.rs          # UI utilities
    ├── prompts.rs      # Interactive prompts (stub)
    └── output.rs       # Output formatting

tests/
├── integration/
│   └── mod.rs          # Integration test scaffold
└── fixtures/
    └── .gitkeep

docs/
├── commands/
│   └── .gitkeep
└── references.md       # External reference links

.github/
└── workflows/
    └── ci.yml          # CI configuration

.gitignore              # Rust ignore patterns
```

### Key Implementation Details

1. **Engine Lifecycle**: The `hello` command exercises the full Scan → Gate → Plan → Execute → Verify lifecycle with stub implementations.

2. **Strong Types**: `BranchName`, `Oid`, `RefName` validate at construction time with comprehensive error handling.

3. **Metadata Schema**: `BranchMetadataV1` uses `deny_unknown_fields` for strict parsing per ARCHITECTURE.md.

4. **Git Interface**: Single doorway to Git via `git2` crate, with typed error handling.

5. **Module Documentation**: Every `mod.rs` includes `//!` documentation per SPEC.md Section 10.1.

---

## Files Created/Modified

| Category | Count |
|----------|-------|
| Source files | 32 |
| Test files | 2 |
| Doc files | 2 |
| Config files | 3 |
| **Total** | **39** |

---

## Notes

- All module implementations are **stubs** that establish structure without real logic
- The `hello` command is for bootstrap validation only
- Feature flags declared but unused until later milestones
- Using `git2` with vendored libgit2 to avoid system OpenSSL dependency
- Dead code warnings are expected for stub implementations

---

## Next Steps (Milestone 1)

Per ROADMAP.md, proceed to **Milestone 1: Core domain types, schemas, and deterministic serialization**:
- Implement full strong type validation for BranchName
- Complete config schema with loading/writing
- Complete metadata schema with CAS operations
- Add comprehensive unit tests with doctests
