# Milestone 0.1 Implementation Notes

## Status: COMPLETE

Last updated: 2026-01-20

---

## Summary

Milestone 0.1 (Gating Integration + Scope Walking) has been completed. All commands now go through gating before execution, using one of two patterns:

1. **`run_gated()` wrapper** - For simple commands (13 files)
2. **`check_requirements()` pre-flight** - For complex commands with internal re-scans (18 files)

---

## Completed Work

### Phase 1: Foundation Types

1. **Command trait** (`src/engine/command.rs`) - COMPLETE
   - Defined `Command` trait with static `REQUIREMENTS`
   - Created `CommandOutput<T>` enum for result handling
   - Added `SimpleCommand` marker trait for no-output commands

2. **Mode types** (`src/engine/modes.rs`) - COMPLETE
   - Defined `SubmitMode`, `SyncMode`, `GetMode` enums
   - Each mode has `resolve()` method for flag/bare-repo handling
   - Each mode has `requirements()` method returning static `RequirementSet`
   - Added `ModeError` for bare-repo-without-flag errors

3. **Scope walking** (`src/engine/gate.rs`) - COMPLETE
   - Implemented `compute_stack_scope()` - returns branches from target to trunk
   - Implemented `compute_freeze_scope()` - with optional descendants
   - Implemented `check_frozen_policy()` - validates no frozen branches in scope
   - Added `Display` impl for `RepairBundle`
   - Added comprehensive tests

### Phase 2: Engine Entrypoint

4. **Engine runner** (`src/engine/runner.rs`) - COMPLETE
   - Created `run_command()` for full lifecycle execution
   - Created `run_command_with_scope()` for scope-aware commands
   - Created `run_command_with_requirements()` for mode-dependent commands
   - Created `run_gated()` for simpler read-only commands
   - Created `check_requirements()` for preflight checks
   - Added `RunError` enum with `NeedsRepair` variant

5. **Module visibility** (`src/engine/mod.rs`) - COMPLETE
   - Added new module declarations: `command`, `modes`, `runner`
   - Re-exported key types for command use

### Phase 3: Command Migration - COMPLETE

**Batch 1: Read-only commands** - COMPLETE (via `run_gated`)
- `log_cmd.rs` - Uses `requirements::READ_ONLY`
- `info.rs` - Uses `requirements::READ_ONLY`
- `relationships.rs` (parent, children) - Uses `requirements::READ_ONLY`
- `trunk.rs` - Uses `requirements::READ_ONLY`

**Batch 2: Metadata-only commands** - COMPLETE (via `run_gated`)
- `init.rs` - Uses `requirements::MINIMAL` (special case: sets up trunk)
- `track.rs` - Uses `requirements::MUTATING_METADATA_ONLY`
- `untrack.rs` - Uses `requirements::MUTATING_METADATA_ONLY`
- `freeze.rs` (freeze, unfreeze) - Uses `requirements::MUTATING_METADATA_ONLY`
- `unlink.rs` - Uses `requirements::MUTATING_METADATA_ONLY`

**Batch 3: Navigation commands** - COMPLETE (via `run_gated`)
- `navigation.rs` (up, down, top, bottom) - Uses `requirements::NAVIGATION`
- `checkout.rs` - Uses `requirements::NAVIGATION`

**Batch 4: Stack mutation commands** - COMPLETE (via `check_requirements`)
- `create.rs` - Uses `requirements::MUTATING` with `run_gated`
- `rename.rs` - Uses `requirements::MUTATING_METADATA_ONLY` with `run_gated`
- `modify.rs` - Uses `requirements::MUTATING` with pre-flight check
- `restack.rs` - Uses `requirements::MUTATING` with pre-flight check
- `move_cmd.rs` - Uses `requirements::MUTATING` with pre-flight check
- `reorder.rs` - Uses `requirements::MUTATING` with pre-flight check
- `split.rs` - Uses `requirements::MUTATING` with pre-flight check
- `squash.rs` - Uses `requirements::MUTATING` with pre-flight check
- `fold.rs` - Uses `requirements::MUTATING` with pre-flight check
- `pop.rs` - Uses `requirements::MUTATING` with pre-flight check
- `delete.rs` - Uses `requirements::MUTATING` with pre-flight check
- `revert.rs` - Uses `requirements::MUTATING` with pre-flight check

**Batch 5: Remote commands with modes** - COMPLETE (via `check_requirements`)
- `submit.rs` - Mode-dependent: `REMOTE` or `REMOTE_BARE_ALLOWED` based on `--no-restack`
- `sync.rs` - Mode-dependent: `REMOTE` or `REMOTE_BARE_ALLOWED` based on restack flag
- `get.rs` - Mode-dependent: `REMOTE` or `REMOTE_BARE_ALLOWED` based on `--no-checkout`
- `merge.rs` - Uses `requirements::REMOTE_BARE_ALLOWED`
- `pr.rs` - Uses `requirements::READ_ONLY`

**Batch 6: Recovery commands** - COMPLETE (via `check_requirements`)
- `recovery.rs` (continue, abort) - Uses `requirements::RECOVERY`
- `undo.rs` - Uses `requirements::RECOVERY`

**Config command** - COMPLETE
- `config_cmd.rs` - Uses `requirements::READ_ONLY` with pre-flight check

**Exempt commands** (no repo required):
- `auth.rs` - No gating needed (doesn't use repo)
- `changelog.rs` - No gating needed (prints static info)
- `completion.rs` - No gating needed (generates shell scripts)

### Phase 4: Testing Infrastructure - COMPLETE

- **Architecture lint** (`scripts/lint-arch.sh`) - COMPLETE
  - Checks for ungated scan imports (excluding pre-flight gated files)
  - Checks all command files use some form of gating
  - Validates pre-flight gating order (check_requirements before scan)
  - Reports summary of gating patterns used

- **Gating tests** - COMPLETE
  - Extensive unit tests in `src/engine/gate.rs`
  - Tests cover: requirement sets, gate function, repair bundle, ready context
  - Tests cover: scope walking, freeze scope, frozen policy checking

---

## Two Migration Patterns

### Pattern 1: `run_gated()` Wrapper (Simple Commands)

For commands that only need one scan and don't do complex operations:

```rust
pub fn my_command(ctx: &Context, ...) -> Result<()> {
    let cwd = ctx.cwd.clone().unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    run_gated(&git, ctx, &requirements::READ_ONLY, |ready| {
        let snapshot = &ready.snapshot;
        // Business logic here
        Ok(())
    })
    .map_err(|e| match e {
        RunError::NeedsRepair(bundle) => {
            anyhow::anyhow!("Repository needs repair: {}", bundle)
        }
        other => anyhow::anyhow!("{}", other),
    })
}
```

### Pattern 2: `check_requirements()` Pre-flight (Complex Commands)

For commands that need to re-scan during execution (after rebases, etc.):

```rust
pub fn my_complex_command(ctx: &Context, ...) -> Result<()> {
    let cwd = ctx.cwd.clone().unwrap_or_else(|| std::env::current_dir().unwrap());
    let git = Git::open(&cwd).context("Failed to open repository")?;

    // Pre-flight gating check
    crate::engine::runner::check_requirements(&git, &requirements::MUTATING)
        .map_err(|bundle| anyhow::anyhow!("Repository needs repair: {}", bundle))?;

    // Now safe to call scan() and do complex operations
    let snapshot = scan(&git).context("Failed to scan repository")?;
    
    // ... complex logic with potential re-scans ...
}
```

### Mode-Dependent Commands

For remote commands that have different requirements based on flags:

```rust
// In submit.rs
let reqs = if no_restack {
    &requirements::REMOTE_BARE_ALLOWED  // Works in bare repos
} else {
    &requirements::REMOTE  // Requires working directory
};
crate::engine::runner::check_requirements(&git, reqs)
    .map_err(|bundle| anyhow::anyhow!("Repository needs repair: {}", bundle))?;
```

---

## Key Decisions Made

1. **Two-pattern approach**: Rather than forcing all commands through a full `Command` trait implementation, we use `run_gated()` for simple commands and `check_requirements()` pre-flight for complex commands. This achieves the gating guarantee while being pragmatic about migration effort.

2. **Pre-flight gating for complex commands**: Commands that need to re-scan (after rebases, during conflict resolution, etc.) use `check_requirements()` as a pre-flight check. This ensures gating happens before any mutations, while still allowing internal re-scans.

3. **Mode-dependent requirements**: Remote commands (submit, sync, get) select between `REMOTE` and `REMOTE_BARE_ALLOWED` based on CLI flags. This allows bare-repo workflows with explicit flags.

4. **Architecture lint recognizes both patterns**: The lint script identifies files using either pattern and only flags truly ungated commands.

---

## Verification

All checks pass:

```bash
cargo test           # 778 tests pass
cargo clippy         # No warnings  
cargo fmt --check    # Clean
./scripts/lint-arch.sh  # All architecture checks passed!
```

Architecture lint summary:
- run_gated() wrapper: 13 files
- check_requirements() pre-flight: 18 files

---

## Acceptance Criteria Status

- [x] Every command goes through gating (via run_gated or check_requirements)
- [x] Mode types used for flag-dependent commands (submit/sync/get)
- [x] Commands receive validated context (ReadyContext or post-check_requirements)
- [x] Architecture lint prevents future bypass
- [x] Scope walking implemented (compute_stack_scope, compute_freeze_scope)
- [x] Frozen policy checking implemented (check_frozen_policy)
- [x] `cargo test` passes
- [x] `cargo clippy` passes

---

## Future Improvements (Out of Scope for 0.1)

1. **Make scan private**: Could change `pub mod scan` to `mod scan` once confident no external code needs it
2. **Full Command trait migration**: Could migrate complex commands to use full Command trait with plan/execute/finish lifecycle
3. **Gating matrix integration tests**: Could add integration tests that spin up real repos to test command × capability combinations
