# Phase 6 Implementation Checklist

## Status: COMMAND MIGRATIONS COMPLETE

**Last Updated:** 2026-01-21

All four async/remote commands have been migrated to implement `AsyncCommand`:
- `merge` - Simplest, no mode dispatch needed
- `get` - Mode dispatch with `GetMode` (WithCheckout/NoCheckout)
- `sync` - Mode dispatch with `SyncMode` (WithRestack/NoRestack)
- `submit` - Mode dispatch with `SubmitMode` (WithRestack/NoRestack)

---

## Infrastructure Tasks

### Task 6.0.1: AsyncCommand Trait
- [x] Define `PlanFut<'a>` type alias
- [x] Define `AsyncCommand` trait in `src/engine/command.rs`
- [x] Add `SimpleAsyncCommand` marker trait
- [x] Add documentation and examples

### Task 6.0.2: Async Runner Functions
- [x] Implement `run_async_command()` in `src/engine/runner.rs`
- [x] Implement `run_async_command_with_requirements()`
- [x] Implement `run_async_command_with_scope()`
- [x] Implement `run_async_command_with_requirements_and_scope()`
- [x] Add debug logging
- [x] Add engine hooks integration

### Task 6.0.3: Forge PlanStep Variants
- [x] Add `ForgePush` variant to `PlanStep`
- [x] Add `ForgeCreatePr` variant
- [x] Add `ForgeUpdatePr` variant
- [x] Add `ForgeDraftToggle` variant
- [x] Add `ForgeRequestReviewers` variant
- [x] Add `ForgeMergePr` variant
- [x] Add `ForgeFetch` variant
- [x] Implement `touched_refs()` for new variants
- [x] Implement `is_mutation()` for new variants
- [x] Implement `description()` for new variants

### Task 6.0.4: Executor Forge Step Handling
- [x] Handle `ForgeFetch` step (via git fetch)
- [x] Handle `ForgePush` step (via git push)
- [x] Placeholder for API steps (error on sync execution)
- [ ] Full async executor with forge client (deferred - API steps execute through async runner)

### Task 6.0.5: Mode Types
- [x] `src/engine/modes.rs` exists (already implemented)
- [x] Implement `SubmitMode` enum and `resolve()`
- [x] Implement `SyncMode` enum and `resolve()`
- [x] Implement `GetMode` enum and `resolve()`
- [x] Implement `ModeError` type
- [x] Unit tests for mode resolution (13 tests pass)
- [x] Export from `src/engine/mod.rs`

---

## Command Migration Tasks

### Task 6.1: Merge Command ✓ COMPLETE
- [x] Create `MergeCommand` struct
- [x] Implement `AsyncCommand` for `MergeCommand`
- [x] Update entry point
- [x] Preserve merge method selection
- [x] Preserve dry run mode
- [x] Preserve stack order merging
- [x] Remove direct `scan()` calls
- [x] All existing merge tests pass
- [x] `cargo clippy` passes

### Task 6.2: Get Command ✓ COMPLETE
- [x] Create `GetWithCheckoutCommand` struct
- [x] Implement `AsyncCommand` for `GetWithCheckoutCommand`
- [x] Create `GetNoCheckoutCommand` struct
- [x] Implement `AsyncCommand` for `GetNoCheckoutCommand`
- [x] Update entry point with mode dispatch
- [x] Preserve PR number resolution
- [x] Preserve parent inference
- [x] Preserve default frozen behavior
- [x] Print worktree guidance in bare repos
- [x] Remove direct `scan()` calls
- [x] All existing get tests pass
- [x] `cargo clippy` passes

### Task 6.3: Sync Command ✓ COMPLETE
- [x] Create `SyncWithRestackCommand` struct
- [x] Implement `AsyncCommand` for `SyncWithRestackCommand`
- [x] Create `SyncNoRestackCommand` struct
- [x] Implement `AsyncCommand` for `SyncNoRestackCommand`
- [x] Update entry point with mode dispatch
- [x] Preserve trunk fast-forward logic
- [x] Preserve force reset logic
- [x] Preserve PR state checking
- [x] Integrate restack via restack helpers
- [x] Remove direct `scan()` calls
- [x] All existing sync tests pass
- [x] `cargo clippy` passes

### Task 6.4: Submit Command ✓ COMPLETE
- [x] Create `SubmitWithRestackCommand` struct
- [x] Implement `AsyncCommand` for `SubmitWithRestackCommand`
- [x] Create `SubmitNoRestackCommand` struct
- [x] Implement `AsyncCommand` for `SubmitNoRestackCommand`
- [x] Update entry point with mode dispatch
- [x] Implement alignment check for --no-restack mode
- [x] Implement base metadata normalization
- [x] Preserve stack comment generation
- [x] Preserve reviewer assignment
- [x] Preserve draft toggle
- [x] Remove direct `scan()` calls
- [x] Remove manual `check_requirements()` calls
- [x] All existing submit tests pass
- [x] `cargo clippy` passes

### Task 6.5: Auth Command (Verification Only)
- [x] Verify auth doesn't need AsyncCommand - Auth is a special case that doesn't require repo state
- [x] Document decision in this checklist

---

## Testing Tasks

### Unit Tests
- [x] Mode resolution tests (all modes) - 13 tests pass
- [ ] AsyncCommand trait implementation tests
- [ ] Plan generation tests for each command
- [ ] Forge step serialization tests

### Integration Tests
- [ ] Submit: create stack, submit, verify PRs
- [ ] Submit: re-submit updates PRs
- [ ] Submit: --dry-run produces no changes
- [ ] Sync: fetch updates trunk
- [ ] Sync: merged PRs detected
- [ ] Sync: restack after sync
- [ ] Get: fetch by PR number
- [ ] Get: fetch by branch name
- [ ] Merge: stack order merging

### Bare Repo Tests
- [ ] Submit refuses without --no-restack
- [ ] Submit with --no-restack checks alignment
- [ ] Submit with --no-restack normalizes stale metadata
- [ ] Sync refuses with restack in bare repo
- [ ] Sync --no-restack works in bare repo
- [ ] Get refuses without --no-checkout
- [ ] Get --no-checkout works in bare repo

### Mock Forge Tests
- [ ] Submit creates PRs via mock
- [ ] Submit updates PRs via mock
- [ ] Sync checks PR state via mock
- [ ] Get resolves PR number via mock
- [ ] Merge calls merge API via mock

---

## Final Verification

- [ ] No direct `scan()` calls in Phase 6 commands
- [ ] All entry points use mode dispatch where applicable
- [ ] All commands implement appropriate trait
- [ ] `cargo test` - ALL PASS
- [ ] `cargo clippy -- -D warnings` - PASS
- [ ] `cargo fmt --check` - PASS
- [ ] Engine hooks fire for async commands (verify via OOB harness)

---

## Notes

### Implementation Notes

**2026-01-21:**
- Infrastructure tasks (6.0.1 - 6.0.5) completed
- AsyncCommand trait added with PlanFut type alias
- Async runner functions added to runner.rs
- Forge PlanStep variants added to plan.rs (7 variants)
- Executor updated to handle ForgeFetch and ForgePush via git
- API-based forge steps return error in sync executor (require async runner)
- Mode types already existed and are fully tested
- Recovery module updated to handle Forge steps

**2026-01-21 (continued):**
- All four command migrations completed:
  - `merge.rs`: MergeCommand implements AsyncCommand, uses run_async_command for gating
  - `get.rs`: GetWithCheckoutCommand/GetNoCheckoutCommand with GetMode dispatch
  - `sync.rs`: SyncWithRestackCommand/SyncNoRestackCommand with SyncMode dispatch
  - `submit.rs`: SubmitWithRestackCommand/SubmitNoRestackCommand with SubmitMode dispatch
- Pattern: Commands use AsyncCommand for gating, then call execute_* functions for actual work
- SubmitOptions moved quiet/verify from Context to options struct for cleaner API
- All existing tests pass, clippy clean

### Deferred Items
- Full async executor with forge client integration - API steps will execute through the async command runner path, not the sync executor
- Additional integration tests for bare repo scenarios (testing tasks in checklist)

### Issues Encountered
- Test needed updating for new SubmitOptions fields (quiet, verify) - fixed
