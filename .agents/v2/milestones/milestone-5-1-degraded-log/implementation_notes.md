# Milestone 5.1: Degraded Log Mode - Implementation Notes

## Summary

Implemented degraded mode for `lattice log` to improve usability in fresh repositories where no branches have been tracked yet.

## Changes Made

### File: `src/cli/commands/log_cmd.rs`

1. **Added `is_degraded_mode()` function** - Detects when log should display in degraded mode:
   - Returns true if no branches are tracked (`metadata.is_empty()`)
   - Returns true if trunk is not configured
   - Distinguishes from corruption (which is handled by Doctor)

2. **Added `print_degraded_banner()` function** - Renders the degraded mode banner:
   - Clear visual separator with dashes
   - Shows trunk status (configured or not)
   - Provides actionable suggestions: `lattice track` and `lattice doctor`

3. **Added `print_untracked_branches()` function** - Lists untracked branches:
   - Filters out trunk from the list
   - Sorts branches alphabetically
   - Shows current branch indicator (`*`)
   - Displays count of untracked branches

4. **Modified main `log()` function**:
   - Checks for degraded mode first, before normal display logic
   - Respects `--quiet` flag (suppresses all degraded mode output)
   - Added mixed mode: `--all` flag shows untracked branches section after tracked branches

## Design Decisions

### Degraded Mode Detection

Chose simple criteria: `metadata.is_empty() || trunk.is_none()`. This covers:
- Fresh repos after `lattice init`
- Repos where user hasn't configured trunk yet

Did NOT use `MetadataReadable` capability check because:
- If metadata has parse errors, that's a corruption case (Doctor repair)
- Degraded mode is about "nothing tracked yet", not "metadata broken"

### Output Style

- Used `eprintln!` for banner (stderr) so it can be suppressed separately from branch list
- Used `println!` for branch list (stdout) for consistent output
- Used simple ASCII dashes (`---`) instead of Unicode box-drawing for terminal compatibility

### Mixed Mode (`--all`)

When `--all` flag is used and there are both tracked and untracked branches:
- Tracked branches display first with normal stack format
- Blank line separator
- "Untracked branches:" header
- Untracked branches with `(untracked)` suffix

## Testing

All existing tests pass (633 tests). Manual verification:

```bash
# Fresh repo test
git init test-repo && cd test-repo
lattice init
lattice log  # Shows degraded banner + no untracked branches

# With branches
git checkout -b feature-1
git checkout -b feature-2  
lattice log  # Shows degraded banner + lists feature-1, feature-2

# After tracking
lattice track feature-1
lattice log  # Normal view (no banner), shows feature-1

# Mixed mode
lattice log --all  # Shows tracked + untracked sections
```

## Files Changed

- `src/cli/commands/log_cmd.rs` - Main implementation (~80 lines added)

## Files Created

- `.agents/v2/milestones/milestone-5-1-degraded-log/PLAN.md`
- `.agents/v2/milestones/milestone-5-1-degraded-log/implementation_notes.md`

## Acceptance Criteria Status

- [x] `lt log` in fresh repo shows degraded banner
- [x] Banner message clearly states "Degraded view"
- [x] Banner includes trunk status
- [x] Banner includes actionable doctor suggestion
- [x] Untracked local branches are listed
- [x] `--all` shows both tracked and untracked with separation
- [x] No metadata writes in degraded mode
- [x] `--quiet` suppresses degraded banner
- [x] `cargo test` passes
- [x] `cargo clippy` passes
