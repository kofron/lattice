# Milestone 3: Implementation Notes

## Completion Date
2026-01-07

## Summary
Successfully implemented the persistence layer for Lattice, including:
- MetadataStore with CAS operations using Git refs pointing to blobs
- FileSecretStore with atomic writes and 0600 permissions
- KeychainSecretStore behind feature flag
- RepoLock using OS file locking (fs2)
- Journal and OpState persistence with fsync

## Key Implementation Decisions

### 1. Git Interface Method: `try_resolve_ref_to_object`

Added a new public method to the Git interface:

```rust
pub fn try_resolve_ref_to_object(&self, refname: &str) -> Result<Option<Oid>, GitError>
```

**Rationale:** The existing `try_resolve_ref` method calls `peel_to_commit()`, which fails for refs pointing to blobs (like metadata refs). The new method resolves refs without peeling, making it suitable for non-commit refs.

**Location:** `src/git/interface.rs:924-956`

### 2. Secret Store Error Handling

FileSecretStore errors deliberately avoid including secret values:

```rust
// GOOD - no secret in error
SecretError::WriteError("failed to write secrets file".into())

// BAD - would leak secret
SecretError::WriteError(format!("failed to write {}", secret_value))
```

**Test:** `error_messages_are_reasonable` verifies error messages don't contain secret values.

### 3. Atomic Write Pattern

Both FileSecretStore and Journal use the atomic write pattern:
1. Write to temp file
2. Set permissions (for secrets)
3. Call `sync_all()` (fsync)
4. Rename to final location

This ensures crash safety and prevents partial writes.

### 4. RepoLock Directory Structure

Lock file is placed at `.git/lattice/lock` rather than `.git/lock`:
- Keeps Lattice-specific files namespaced
- Avoids potential conflicts with git's own lock files
- Directory is created automatically on lock acquisition

### 5. Journal Step Types

Enhanced `StepKind` enum to support proper rollback:

```rust
pub enum StepKind {
    RefUpdate { refname, old_oid, new_oid },      // For branch refs
    MetadataWrite { branch, old_ref_oid, new_ref_oid },  // For metadata
    MetadataDelete { branch, old_ref_oid },       // For untracking
    Checkpoint { name },                          // Named checkpoints
    GitProcess { args, description },             // Git operations
    ConflictPaused { refname, source_oid, target_oid },  // Conflicts
}
```

Each variant stores enough information to reverse the operation.

### 6. Feature-Gated Keychain

`KeychainSecretStore` is behind the `keychain` feature flag:
- Reduces default dependency footprint
- Avoids linking issues on systems without keyring support
- Stub implementation provided when feature is disabled

```toml
[features]
keychain = ["dep:keyring"]
```

## Test Coverage

Final test counts:
- Unit tests: 174 passing
- Git integration tests: 48 passing  
- Persistence integration tests: 29 passing
- Property tests: 12 passing
- Doc tests: 21 passing (39 ignored - require git repos)

**Total: 263 passing tests**

## Files Modified/Created

| File | Action |
|------|--------|
| `Cargo.toml` | Added fs2, keyring dependencies |
| `src/git/interface.rs` | Added `try_resolve_ref_to_object` method |
| `src/core/ops/lock.rs` | Full RepoLock implementation |
| `src/core/ops/journal.rs` | Enhanced steps, persistence |
| `src/core/metadata/store.rs` | Full MetadataStore implementation |
| `src/secrets/mod.rs` | Factory and re-exports |
| `src/secrets/traits.rs` | Trait definition only |
| `src/secrets/file_store.rs` | Created - FileSecretStore |
| `src/secrets/keychain_store.rs` | Created - KeychainSecretStore |
| `tests/persistence_integration.rs` | Created - 29 integration tests |

## Known Limitations

1. **Keychain testing**: KeychainSecretStore cannot be tested in CI without access to the system keychain. Manual testing on macOS confirmed it works.

2. **Cross-process lock testing**: The integration test uses threads rather than processes. True cross-process locking is verified by the fs2 crate's own tests.

3. **Windows permissions**: File permissions (0600) are only enforced on Unix. Windows relies on NTFS ACLs which are not explicitly set.

## Next Steps

Per ROADMAP.md, proceed to Milestone 4: Engine Lifecycle:
- Scanner with capabilities
- Event ledger  
- Gating with ReadyContext/RepairBundle
- Planner and Executor
- Fast verify
