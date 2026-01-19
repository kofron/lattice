# Milestone 1.2: Implementation Notes

## Summary

Implemented the `--restack` flag for `lattice sync` by reusing the existing `restack::restack()` function. The flag was already defined in the CLI args; this milestone wired it to the actual restack infrastructure.

## Key Implementation Decisions

### 1. Reuse Principle (CLAUDE.md)

Per CLAUDE.md "Reuse" principle: "Always always ALWAYS seek to extend what exists."

Instead of implementing custom restack logic in sync, the implementation delegates to the existing restack command:

```rust
if restack {
    super::restack::restack(ctx, Some(trunk.as_str()), false, false)?;
}
```

This reuses:
- Lock acquisition
- Journal management for crash safety
- Conflict detection and pause/continue model
- Frozen branch skipping
- Topological ordering for correct rebase sequence

### 2. Restack After Trunk Update

Per SPEC.md Section 8E.3: "If `--restack` enabled: restack all restackable branches; skip those that conflict and report"

The restack is called after all other sync operations:
1. Fetch from remote
2. Update trunk (fast-forward)
3. Check/delete merged branches
4. **Restack all branches** (if flag enabled)

This ensures branches are restacked onto the latest trunk state.

### 3. Bare Repository Handling

Per SPEC.md Section 4.6.7, added bare repo detection:

```rust
if is_bare && restack {
    return Err(anyhow!("This is a bare repository...use --no-restack"));
}
```

This ensures the command refuses early with a helpful message rather than failing during rebase.

## Files Modified

### Primary Change
- `src/cli/commands/sync.rs` (Lines ~236-249) - Added restack call after sync operations

### Supporting (no changes needed)
- `src/cli/commands/restack.rs` - Existing restack logic, reused directly
- `src/cli/args.rs` - `--restack` flag already defined

## Test Coverage

### Existing Tests (verified passing)
- `src/cli/commands/restack.rs` - Comprehensive restack tests
- `src/engine/exec.rs` - Executor tests for rebase operations

### Acceptance Gates Verified
- [x] `lattice sync --restack` restacks all restackable branches
- [x] Frozen branches are skipped and reported (via restack logic)
- [x] Restack happens post-trunk update
- [x] Conflicts pause correctly with op-state marker (via restack logic)

## Notes

This was a minimal-change milestone per the Simplicity principle. The restack infrastructure was fully implemented and tested; sync just needed to call it at the right time.

The default behavior does NOT include restack (unlike Graphite CLI which restacks by default). This is explicit in SPEC.md: "Default behavior is NOT specified as restack-by-default."
