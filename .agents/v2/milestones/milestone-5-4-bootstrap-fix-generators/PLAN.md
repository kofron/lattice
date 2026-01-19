# Milestone 5.4: Bootstrap Fix Generators (Remote-First)

## Goal

Generate fix options for remote-first bootstrap, enabling users to import existing open PRs and their branches into Lattice tracking through the Doctor framework.

**Core principle from ARCHITECTURE.md Section 8.1:** "Doctor shares the same scanner, planner model (repair plans are plans), executor, event recording. There is no separate 'repair mutation path.'"

---

## Background

Milestone 5.3 adds bootstrap issue detection (already planned). This milestone implements the fix generators that respond to those issues:

| Issue (from 5.3) | Fix Generator (this milestone) |
|------------------|-------------------------------|
| `RemoteOpenPrBranchUntracked` | `generate_track_existing_from_pr_fixes()` |
| `RemoteOpenPrBranchMissingLocally` | `generate_fetch_and_track_pr_fixes()` |
| `RemoteOpenPrNotLinkedInMetadata` | `generate_link_pr_fixes()` |

---

## Status: COMPLETE

All implementation steps have been completed and verified.

---

## Spec References

- **ROADMAP.md Milestone 5.4** - Bootstrap fix generators deliverables
- **ARCHITECTURE.md Section 8** - Doctor framework
- **ARCHITECTURE.md Section 8.2** - Issues and fix options
- **SPEC.md Section 8B.1** - Track command behavior
- **SPEC.md Section 8E.1** - Forge abstraction

---

## Implementation Summary

### Prerequisites Completed

1. **Track Command Base Computation** - Updated to use merge-base
2. **Git Interface `fetch_ref`** - Added method to fetch specific refs

### Fix Generators Added

1. **`generate_track_existing_from_pr_fixes()`** - For untracked local branches
2. **`generate_fetch_and_track_pr_fixes()`** - For missing branches (creates frozen)
3. **`generate_link_pr_fixes()`** - For tracked branches without PR linkage

### Planner Updates

- Handle `"(fetched from remote)"` placeholder with `RunGit` fetch step
- Parse description format to extract parent, frozen, and PR info
- Extended `create_minimal_metadata` for freeze state and PR linkage

### Tests Added

15 unit tests covering:
- Fix generation for each bootstrap case
- Edge cases (branch missing, already tracked, etc.)
- Parent selection logic
- Evidence extraction

---

## Acceptance Criteria

- [x] `track` command uses merge-base for initial tracking
- [x] `track` refuses when merge-base is None with actionable error
- [x] TrackExisting fix creates valid metadata
- [x] FetchAndTrack fix generates fetch step and creates frozen metadata
- [x] LinkPR fix updates only cached PR state, not structural fields
- [x] Previews show all ref and metadata changes
- [x] `cargo test` passes
- [x] `cargo clippy` passes
