# Milestone 5.1: Degraded Log Mode

## Goal

Make `lattice log` useful before metadata exists by showing a clear "degraded mode" view with an actionable suggestion to run `lattice doctor` for bootstrap.

**Core principle:** Log should be helpful immediately after `init`, even when no branches have been tracked yet. Users should understand the state and know how to proceed.

---

## Background

Currently, `lattice log` works when metadata exists but provides limited value in fresh repositories:
- Shows "No tracked branches." when metadata is empty
- Doesn't show untracked branches by default
- Doesn't guide users toward bootstrap

The `log` command already only requires `RepoOpen` capability (not `MetadataReadable`), so it can run in degraded conditions. We need to improve the UX when running in this state.

**Current implementation:** `src/cli/commands/log_cmd.rs`
- Uses `scan(&git)` to get `RepoSnapshot`
- Displays branches from `snapshot.graph.branches()` (only tracked branches)
- Shows "No tracked branches." when empty

---

## Spec References

- **Proposed SPEC.md Section 8G.1 amendment** - Degraded log mode
- **ARCHITECTURE.md Section 5.3** - `log` requires only `RepoOpen`
- **SPEC.md Section 8G.1** - Current log behavior

---

## Design Decisions

### What constitutes "degraded mode"?

Degraded mode is detected when:
1. `MetadataReadable` capability is present (no parse errors), AND
2. `snapshot.metadata.is_empty()` (no tracked branches exist)

This distinguishes between:
- **Degraded:** Metadata system works, but nothing is tracked yet (bootstrap opportunity)
- **Corrupted:** Metadata parse errors exist (Doctor repair needed, different issue)

### What should degraded mode show?

1. **Banner:** Clear indication that the view is incomplete
2. **Trunk status:** Show configured trunk, or "unknown" if not configured
3. **Untracked branches:** List local branches that could be tracked
4. **Call to action:** Suggest `lattice doctor` for bootstrap

### What should degraded mode NOT do?

- Never write metadata
- Never attempt repair
- Never show PR annotations (would require auth/API calls)

---

## Implementation Steps

### Step 1: Add Degraded Mode Detection

**File:** `src/cli/commands/log_cmd.rs`

Add a helper function to detect degraded mode.

### Step 2: Add Degraded Mode Banner Rendering

**File:** `src/cli/commands/log_cmd.rs`

Add a function to render the degraded mode banner with trunk status and call to action.

### Step 3: Add Untracked Branches Display

**File:** `src/cli/commands/log_cmd.rs`

Add a function to list untracked branches (excluding trunk).

### Step 4: Modify Main Log Function

**File:** `src/cli/commands/log_cmd.rs`

Update the `log` function to check for degraded mode first and handle it.

### Step 5: Handle Mixed Mode

When `--all` flag is used and there are both tracked and untracked branches, show both clearly separated.

### Step 6: Add Required Imports

Add the capability import at the top of the file.

---

## Critical Files

| File | Action | Purpose |
|------|--------|---------|
| `src/cli/commands/log_cmd.rs` | MODIFY | Add degraded mode detection and display |
| `src/engine/capabilities.rs` | READ ONLY | Reference for Capability enum |
| `src/engine/scan.rs` | READ ONLY | Reference for RepoSnapshot structure |

---

## Acceptance Criteria

- [ ] `lt log` in fresh repo (post-init, no tracks) shows degraded banner
- [ ] Banner message clearly states "Degraded view"
- [ ] Banner includes trunk status (configured or not)
- [ ] Banner includes actionable doctor suggestion
- [ ] Untracked local branches are listed in degraded mode
- [ ] `lt log` with partial metadata shows mixed view (tracked normal, untracked grouped)
- [ ] `lt log --all` shows both tracked and untracked branches with clear separation
- [ ] No metadata writes occur in degraded mode
- [ ] `--quiet` flag suppresses the degraded banner
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

---

## Testing

### Manual Verification

1. Create a fresh repo: `git init test-repo && cd test-repo`
2. Initialize Lattice: `lattice init`
3. Run `lattice log` - should show degraded banner
4. Create a branch: `git checkout -b feature-1`
5. Run `lattice log` - should show degraded banner with feature-1 as untracked
6. Track the branch: `lattice track`
7. Run `lattice log` - should show normal stack view (no banner)

---

## Verification Commands

```bash
cargo check
cargo clippy -- -D warnings
cargo test
cargo fmt --check
```

---

## Notes

- **Simplicity principle:** This is a display-only change. No new data structures or mutation paths.
- **Purity principle:** Degraded mode is purely informational; it never modifies state.
- **Code is communication:** The banner text should be actionable and not alarming.
- **Follow the leader:** Uses existing RepoSnapshot and Capability infrastructure.

---

## Edge Cases

1. **Bare repository:** Should still work (log only requires RepoOpen)
2. **Detached HEAD:** Show degraded mode normally, current branch indicator not shown
3. **Only trunk exists:** Show trunk as configured, no untracked branches to display
4. **Metadata parse errors:** This is NOT degraded mode - it's a corruption case handled by existing Doctor issues

---

## Estimated Scope

- **Lines of code changed:** ~80-120 in `log_cmd.rs`
- **Risk:** Low - display-only change, no mutation paths affected
