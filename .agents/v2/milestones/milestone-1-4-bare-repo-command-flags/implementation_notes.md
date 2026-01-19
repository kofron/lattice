# Milestone 1.4: Implementation Notes

## Summary

Implemented `--no-restack` flag for submit/sync and `--no-checkout` flag for get commands per SPEC.md Section 4.6.7 (Bare repo policy). This enables these commands to work in bare repositories with explicit user acknowledgment of reduced functionality.

## Key Implementation Decisions

### 1. Submit: Ancestry Alignment Checks (SPEC.md 4.6.7)

For bare repos with `--no-restack`, submit must verify branches are already aligned:

**Alignment rule**: For branch `b` with parent `p`, `p.tip` must be ancestor of `b.tip`.

```rust
let is_ancestor = git.is_ancestor(parent_tip, branch_tip)?;
if !is_ancestor {
    // Refuse with "Restack required" message
}
```

**Metadata normalization**: If ancestry holds but `b.base != p.tip`, the implementation normalizes base metadata without rewriting history:

```rust
if ancestry_holds && base != parent_tip {
    // Update metadata only - no git rebase
    metadata.base = parent_tip;
}
```

This allows submit to proceed in bare repos when the git history is correct but metadata is stale.

### 2. Get: No-Checkout Mode (SPEC.md 4.6.7)

The `--no-checkout` flag enables get in bare repos by:
1. Fetching the branch ref from remote
2. Creating/updating the local branch ref
3. **Tracking the branch** with full metadata (parent inference, base computation)
4. Defaulting to frozen (unless `--unfrozen`)
5. Printing worktree creation guidance

```rust
async fn handle_no_checkout_mode(
    ctx: &Context,
    git: &Git,
    branch_name: &str,
    pr_info: Option<&PrInfo>,
    unfrozen: bool,
) -> Result<()> {
    // Create/update ref
    // Compute base as merge-base(branch_tip, parent_tip)
    // Write tracking metadata
    // Print guidance: "To work on this branch: git worktree add..."
}
```

### 3. Sync: No-Restack Mode

The `--no-restack` flag was already handled in Milestone 1.2, but this milestone added explicit bare repo detection:

```rust
if is_bare && restack {
    return Err(anyhow!("This is a bare repository...use --no-restack"));
}
```

### 4. Early Refusal with Guidance

Per SPEC.md Section 4.6.9, all refusals include:
- Why it failed (bare repo has no working directory)
- A concrete `git worktree add` example
- Which flags enable limited functionality

Example message:
```
This is a bare repository. The `get` command cannot checkout without a working directory.

To track the branch without checkout (useful for CI/automation), use:

    lattice get --no-checkout feature-branch

To work with a working directory, create a worktree first:

    git worktree add ../feature-worktree feature-branch
```

## Files Modified

### CLI Arguments
- `src/cli/args.rs` - Added `--no-checkout` flag to get command (Lines ~1388)

### Commands
- `src/cli/commands/get.rs` - Added bare repo check, implemented `handle_no_checkout_mode()` (Lines ~59-280)
- `src/cli/commands/submit.rs` - Added bare repo check, ancestry alignment, metadata normalization (Lines ~414-530)
- `src/cli/commands/sync.rs` - Added bare repo check for restack mode (already had flag support)

### Command Dispatch
- `src/cli/commands/mod.rs` - Thread `no_checkout` flag through dispatch (Lines ~261-269)

## Test Coverage

### Acceptance Gates Verified
- [x] `lattice submit` refuses in bare repo without `--no-restack`
- [x] `lattice submit --no-restack` checks ancestry alignment
- [x] `lattice submit --no-restack` normalizes base metadata if ancestry holds
- [x] `lattice submit --no-restack` refuses if ancestry violated with message
- [x] `lattice sync` refuses in bare repo without `--no-restack`
- [x] `lattice sync --no-restack` performs fetch, trunk FF, PR checks only
- [x] `lattice get` refuses in bare repo without `--no-checkout`
- [x] `lattice get --no-checkout` fetches, tracks, computes base, defaults frozen
- [x] `lattice get --no-checkout` prints worktree creation guidance

## Notes

### Design Philosophy

The bare repo support follows a "no silent downgrades" principle:
- Commands don't silently skip functionality
- Users must explicitly acknowledge reduced capability via flags
- Error messages explain both the limitation and the workaround

### Base Computation in No-Checkout Mode

For `get --no-checkout`, the base is computed as:
```rust
let base = git.merge_base(branch_tip, parent_tip)?;
```

This is the same logic used during normal tracking, ensuring metadata consistency.

### Frozen by Default

Per SPEC.md, branches fetched with `--no-checkout` default to frozen:
- Prevents accidental modification in CI pipelines
- User can override with `--unfrozen` if they intend to work on it
