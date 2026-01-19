# Milestone 5.4: Bootstrap Fix Generators - Implementation Notes

## Summary

This milestone implements fix generators for remote-first bootstrap, enabling users to import existing open PRs and their branches into Lattice tracking through the Doctor framework.

## Changes Made

### 1. Track Command Base Computation (Prerequisite)

**File:** `src/cli/commands/track.rs`

Updated the track command to compute merge-base between the branch tip and parent tip, rather than using the parent's tip directly as the base. This ensures the base OID correctly represents the divergence point.

Key changes:
- Get branch tip OID from snapshot
- Compute `merge_base(branch_tip, parent_tip)`
- Refuse with actionable error if no common ancestor exists

### 2. Git Interface: `fetch_ref` Method (Prerequisite)

**File:** `src/git/interface.rs`

Added `fetch_ref` method to fetch a specific ref from a remote:
- Runs `git fetch <remote> <refspec>`
- Handles both simple refspecs and full `source:destination` format
- Returns the OID of the fetched ref
- Added helper `read_fetch_head()` for extracting OID from FETCH_HEAD

### 3. Bootstrap Fix Generators

**File:** `src/doctor/generators.rs`

Added three new fix generators:

#### `generate_track_existing_from_pr_fixes()`
- For `RemoteOpenPrBranchUntracked` issues
- Creates unfrozen tracking metadata for existing local branches
- Parent determined from PR's base_ref
- Preconditions: RepoOpen, TrunkKnown, GraphValid

#### `generate_fetch_and_track_pr_fixes()`
- For `RemoteOpenPrBranchMissingLocally` issues  
- Fetches branch from remote and creates frozen metadata
- Default freeze reason: "teammate_branch"
- Preconditions: RepoOpen, TrunkKnown, AuthAvailable, RemoteResolved

#### `generate_link_pr_fixes()`
- For `RemoteOpenPrNotLinkedInMetadata` issues
- Updates only cached PR state, not structural fields
- Per ARCHITECTURE.md Section 11.2 (PR linkage is cached)
- Precondition: RepoOpen

#### Helper Functions
- `extract_pr_evidence()`: Extracts branch, PR number, base_ref, URL from issue evidence
- `determine_parent_from_base_ref()`: Uses 4-rule priority for parent selection:
  1. If base_ref is trunk → parent = trunk
  2. If base_ref is tracked locally → parent = base_ref
  3. If base_ref is another PR's head (chain) → parent = base_ref
  4. Fallback to trunk

### 4. Doctor Planner Updates

**File:** `src/doctor/planner.rs`

Updated to handle bootstrap fixes:

#### `ref_change_to_step()` 
- Detects placeholder OID `"(fetched from remote)"`
- Generates `RunGit` fetch step instead of `UpdateRefCas`

#### `metadata_change_to_step()`
- Added handling for "pr" field updates (for LinkPR fix)
- Parses `linked(#42)` format to create `PrState::linked()`

#### `parse_create_description()`
- Parses generator description format: `"parent=<name>, pr=#<num>, frozen|unfrozen"`
- Extracts parent, frozen state, and PR info

#### `create_minimal_metadata()`
- Extended to accept frozen and pr_info parameters
- Uses `BranchMetadataBuilder` for proper construction
- Sets appropriate `FreezeState` and `PrState`

### 5. Issue Schema Updates

**Files:** `src/engine/health.rs`, `src/doctor/issues.rs`

Added `base_ref` parameter to `remote_pr_branch_untracked` issue constructor:
- Enables fix generators to determine parent from PR base
- Updated all callers (`scan.rs`, `issues.rs`)
- Updated tests

## Design Decisions

### Base Computation in Planner
The planner doesn't have access to Git for computing merge-base. Instead of breaking the "functional core" principle:
- `TrackExisting` uses parent tip as base (acceptable for initial tracking)
- The true merge-base is computed during sync/rebase operations
- This is documented in the code

### FetchAndTrack Uses RunGit
Rather than adding a new `PlanStep::FetchRef` variant, we use `RunGit` with fetch arguments. The placeholder OID `"(fetched from remote)"` triggers this special handling.

### PR Info Encoding
PR metadata is encoded in the fix description string for parsing by the planner. This avoids adding new fields to `FixOption` while maintaining the separation between generators and planner.

## Tests Added

**File:** `src/doctor/generators.rs`

15 new tests covering:
- Fix generation for each bootstrap case
- Edge cases (branch missing, already tracked, not tracked)
- Parent selection logic (trunk, tracked branch, fallback)
- Evidence extraction from issues

## Acceptance Criteria Status

- [x] `track` command uses merge-base for initial tracking
- [x] `track` refuses when merge-base is None with actionable error
- [x] TrackExisting fix creates valid metadata
- [x] FetchAndTrack fix generates fetch step and creates frozen metadata
- [x] LinkPR fix updates only cached PR state, not structural fields
- [x] Previews show all ref and metadata changes
- [x] `cargo test` passes
- [x] `cargo clippy` passes

## Files Modified

| File | Changes |
|------|---------|
| `src/cli/commands/track.rs` | Merge-base computation |
| `src/git/interface.rs` | Added `fetch_ref()`, `read_fetch_head()` |
| `src/doctor/generators.rs` | 3 generators + 2 helpers + 15 tests |
| `src/doctor/planner.rs` | Handle bootstrap fixes, parse descriptions |
| `src/engine/health.rs` | Added `base_ref` to issue constructor |
| `src/doctor/issues.rs` | Added `base_ref` to `KnownIssue` variant |
| `src/engine/scan.rs` | Updated issue constructor call |
