# Milestone 1: Core Domain Types, Schemas, and Deterministic Serialization

## Status: COMPLETE

**Completed:** 2026-01-07

---

## Overview

**Goal:** Define the vocabulary of the system so everything else composes cleanly. This milestone transforms the stub types from Milestone 0 into fully validated, production-ready domain types with comprehensive testing.

**Principles:** Follow the leader (SPEC.md + ARCHITECTURE.md), Simplicity, Purity, Tests are everything.

**Dependency:** Milestone 0 (complete)

---

## Acceptance Gates - ALL PASSED

- [x] `cargo fmt --check` passes
- [x] `cargo clippy -- -D warnings` passes
- [x] `cargo test` passes (89 unit tests + 17 doctests + 12 property tests)
- [x] `cargo doc --no-deps` succeeds
- [x] `cargo run -- hello` outputs "Hello from Lattice!"

### Specific Acceptance Criteria - ALL MET

- [x] `BranchName::new(".lock")` fails (starts with `.`)
- [x] `BranchName::new("branch.lock")` fails (ends with `.lock`)
- [x] `BranchName::new("valid/branch")` succeeds
- [x] `Oid::zero()` creates 40-character zero OID
- [x] `Oid::short(7)` returns first 7 characters
- [x] `RefName::for_branch("main")` returns `refs/heads/main`
- [x] `RefName::for_metadata("main")` returns `refs/branch-metadata/main`
- [x] Config loads from `$LATTICE_CONFIG` when set
- [x] Config prefers repo over global
- [x] Config atomic write doesn't corrupt on crash
- [x] Metadata with unknown fields is rejected
- [x] Metadata with wrong schema_version is rejected
- [x] `StructuralMetadata` extracts only structural fields
- [x] Fingerprint is deterministic for same refs
- [x] 17 doctests exist and pass
- [x] 12 property tests exist and pass

---

## Implementation Summary

### Step 1-4: Strong Types (Complete)

**File:** `src/core/types.rs`

Implemented:
- `BranchName` - Full Git refname validation including:
  - Cannot start with `.` or `-`
  - Cannot end with `.lock` or `/`
  - Cannot contain `..`, `@{`, `//`, control characters
  - Cannot be exactly `@` (reserved)
  - Per-component validation (no `.` prefix, no `.lock` suffix)
- `Oid` - SHA-1/SHA-256 validation with:
  - `zero()` - Create null OID
  - `is_zero()` - Check for null OID
  - `short(len)` - Abbreviated form
  - Lowercase normalization
- `RefName` - Full refname validation with:
  - `for_branch(branch)` - Create `refs/heads/<branch>`
  - `for_metadata(branch)` - Create `refs/branch-metadata/<branch>`
  - `strip_prefix(prefix)` - Extract suffix
  - `is_branch_ref()`, `is_metadata_ref()` - Type checks
- `Fingerprint` - Repository state hash using SHA-256:
  - `compute(refs)` - Create from sorted ref/oid pairs
  - Order-independent (sorts internally)
  - Deterministic

### Step 5-6: Config Loading (Complete)

**Files:** `src/core/config/mod.rs`, `src/core/config/schema.rs`

Implemented:
- Full precedence resolution:
  1. `$LATTICE_CONFIG` environment variable
  2. `$XDG_CONFIG_HOME/lattice/config.toml`
  3. `~/.lattice/config.toml`
- Repo config with compatibility paths (warns on deprecated locations)
- Atomic writes (temp file + rename)
- Validation:
  - `trunk` must be valid branch name
  - `remote` must be non-empty
  - `secrets.provider` must be "file" or "keychain"
  - `default_forge` must be "github" (v1)
- `ConfigLoadResult` with warnings for deprecated paths

### Step 7-9: Metadata Schema (Complete)

**File:** `src/core/metadata/schema.rs`

Implemented:
- `BranchMetadataV1` with full validation
- `parse_metadata(json)` - Version-dispatched parsing with:
  - Kind validation
  - Schema version check
  - Field validation (branch names, OIDs)
- `BranchMetadataBuilder` - Fluent construction
- `StructuralMetadata` - Validated structural-only extraction
- `StructuralView` - Reference view without cloning
- Helper methods on `FreezeState`, `PrState`, `ParentInfo`
- `to_canonical_json()` - Deterministic serialization

### Step 10-12: Tests (Complete)

**Files:** Various + `tests/property_tests.rs`

Implemented:
- Serialization determinism tests
- 17 doctests across all modules
- 12 property-based tests using proptest:
  - `branch_name_serde_roundtrip`
  - `oid_serde_roundtrip`
  - `oid_normalized_to_lowercase`
  - `fingerprint_deterministic`
  - `fingerprint_order_independent`
  - `branch_name_to_refname`
  - `metadata_serde_roundtrip`
  - `oid_short_is_prefix`
  - `zero_oid_detection`
  - Plus 3 determinism tests

---

## Dependencies Added

```toml
sha2 = "0.10"
hex = "0.4"
dirs = "6.0"
```

---

## Files Changed

| File | Action | Description |
|------|--------|-------------|
| `src/core/types.rs` | Modified | Complete validation for BranchName, Oid, RefName; add Fingerprint |
| `src/core/config/mod.rs` | Modified | Full config loading with precedence |
| `src/core/config/schema.rs` | Modified | Add validation methods |
| `src/core/metadata/mod.rs` | Modified | Add re-exports |
| `src/core/metadata/schema.rs` | Modified | Add validation, migration, structural extraction, builder |
| `src/engine/gate.rs` | Modified | Add `#[allow(dead_code)]` for stub |
| `src/engine/plan.rs` | Modified | Add `#[allow(dead_code)]` for stub |
| `src/engine/scan.rs` | Modified | Add `#[allow(dead_code)]` for stub |
| `tests/property_tests.rs` | Created | Property-based tests |
| `Cargo.toml` | Modified | Add sha2, hex, dirs dependencies |

---

## Test Counts

| Category | Count |
|----------|-------|
| Unit tests | 89 |
| Doctests | 17 |
| Property tests | 12 |
| **Total** | **118** |

---

## Notes

- All types use `deny_unknown_fields` per ARCHITECTURE.md strict parsing
- Validation happens at construction (parse, don't validate pattern)
- Config loading is the only I/O in this milestone
- Fingerprint uses SHA-256 for stability
- Dead code warnings on engine stubs suppressed (intentional placeholders)

---

## Next Steps (Milestone 2)

Per ROADMAP.md, proceed to **Milestone 2: Single Git Interface**:
- Implement single Git interface via git2 crate
- Typed error categories
- Git queries for scanner
- Ref update primitives with CAS
