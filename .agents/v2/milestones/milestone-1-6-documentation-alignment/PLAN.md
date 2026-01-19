# Milestone 1.6: Documentation Alignment

## Goal

Align documentation with implemented code. v2 milestones 1.1-1.5 are complete, but documentation artifacts are missing or inconsistent.

**Governing Principle:** Per CLAUDE.md "Code is communication" - documentation must accurately reflect the codebase state so new developers can understand the system and implementation history.

---

## Background

### Issues Identified in ROADMAP.md

1. **Missing implementation_notes.md files**
   - Milestone 1.1 (Doctor Fix Execution): No implementation_notes.md
   - Milestone 1.2 (Sync Restack): No implementation_notes.md
   - Milestone 1.3 (OAuth RepoAuthorized): No implementation_notes.md
   - Milestone 1.4 (Bare Repo Command Flags): No implementation_notes.md
   - Milestone 1.5 (Event Ledger Completion): No implementation_notes.md
   - Milestone 1-github-app-oauth (v2 root): No implementation_notes.md

2. **v1 ROADMAP.md OAuth reference**
   - v1 ROADMAP.md ends at Milestone 10 (Multi-Forge Scaffolding)
   - Does not reference that OAuth was completed in v2
   - No clear handoff to v2 for OAuth work

3. **PLAN.md acceptance gates not updated**
   - Several PLAN.md files have unchecked acceptance gates despite milestones being complete

### What Exists vs What's Missing

| Milestone | PLAN.md | implementation_notes.md |
|-----------|---------|------------------------|
| v2/milestone-1-github-app-oauth | ✓ | ✗ |
| v2/milestone-2-bare-repo-support | ✓ | ✓ |
| v2/milestone-1-1-doctor-fix | ✓ | ✗ |
| v2/milestone-1-2-sync-restack | ✓ | ✗ |
| v2/milestone-1-3-oauth-repoauthorized | ✓ | ✗ |
| v2/milestone-1-4-bare-repo-command-flags | ✓ | ✗ |
| v2/milestone-1-5-event-ledger-completion | ✓ | ✗ |

---

## Implementation Steps

### Step 1: Create implementation_notes.md for Milestone 1.1 (Doctor Fix Execution)

**File:** `.agents/v2/milestones/milestone-1-1-doctor-fix/implementation_notes.md`

**Content to document:**
- Summary of the implementation
- Key implementation decisions (executor reuse, event recording approach)
- Files modified with brief descriptions
- Notable code patterns used
- Test coverage added
- Any deviations from original PLAN.md

**Research needed:**
- Review `src/cli/commands/mod.rs` for doctor --fix implementation
- Check event recording calls (DoctorProposed, DoctorApplied)
- Identify test files added

---

### Step 2: Create implementation_notes.md for Milestone 1.2 (Sync Restack)

**File:** `.agents/v2/milestones/milestone-1-2-sync-restack/implementation_notes.md`

**Content to document:**
- Summary: Wired `--restack` flag to existing restack infrastructure
- Key decision: Reuse existing `restack::restack()` function
- Files modified
- Test coverage

**Research needed:**
- Review `src/cli/commands/sync.rs` for restack integration

---

### Step 3: Create implementation_notes.md for Milestone 1.3 (OAuth RepoAuthorized)

**File:** `.agents/v2/milestones/milestone-1-3-oauth-repoauthorized/implementation_notes.md`

**Content to document:**
- Summary: Added GitHub installations API client, authorization cache, scanner integration
- Key decisions:
  - Cache TTL (10 minutes)
  - Pagination handling
  - Integration into scanner/gating
- Files created/modified
- Test coverage

**Research needed:**
- Review `src/auth/installations.rs`
- Review `src/auth/cache.rs`
- Review scanner integration in `src/engine/scan.rs`

---

### Step 4: Create implementation_notes.md for Milestone 1.4 (Bare Repo Command Flags)

**File:** `.agents/v2/milestones/milestone-1-4-bare-repo-command-flags/implementation_notes.md`

**Content to document:**
- Summary: Added `--no-restack` and `--no-checkout` flags per SPEC.md
- Key decisions:
  - Ancestry alignment checks for submit
  - Base metadata normalization
  - Get no-checkout mode with tracking
- Files modified
- Test coverage

**Research needed:**
- Review `src/cli/commands/get.rs`
- Review `src/cli/commands/submit.rs`
- Review `src/cli/commands/sync.rs`

---

### Step 5: Create implementation_notes.md for Milestone 1.5 (Event Ledger Completion)

**File:** `.agents/v2/milestones/milestone-1-5-event-ledger-completion/implementation_notes.md`

**Content to document:**
- Summary: Wired DivergenceObserved event recording into scan flow
- Key decisions:
  - Best-effort recording (don't fail scan on ledger errors)
  - Added divergence field to RepoHealthReport
  - Debug output for divergence info
- Files modified
- Test coverage

**Research needed:**
- Review `src/engine/scan.rs` for detect_and_record_divergence
- Review `src/engine/health.rs` for divergence field

---

### Step 6: Create implementation_notes.md for GitHub App OAuth (v2 root milestone)

**File:** `.agents/v2/milestones/milestone-1-github-app-oauth/implementation_notes.md`

**Content to document:**
- Summary: Complete OAuth implementation via GitHub App device flow
- Key decisions:
  - Client ID: `Iv23liIqb9vJ8kaRyZaU`
  - Token bundle storage format
  - Auth lock mechanism for refresh safety
  - Removal of PAT support
- Files created/modified
- Test coverage

**Research needed:**
- Review `src/auth/` module structure
- Review `src/cli/commands/auth.rs`

---

### Step 7: Add v2 Reference to v1 ROADMAP.md

**File:** `.agents/v1/ROADMAP.md`

**Change:** Add a section at the end noting that GitHub App OAuth and subsequent compliance work was completed in v2.

**Add:**
```markdown
---

## v2 Continuation

GitHub App OAuth authentication and additional compliance work are tracked in `.agents/v2/ROADMAP.md`. The v1 roadmap covers the foundational implementation; v2 covers OAuth, bare repository support, and documentation alignment.

See: `.agents/v2/ROADMAP.md` for continued development.
```

---

### Step 8: Update PLAN.md Acceptance Gates

**Files to update:**
- `.agents/v2/milestones/milestone-1-1-doctor-fix/PLAN.md` - Mark all gates as checked
- `.agents/v2/milestones/milestone-1-2-sync-restack/PLAN.md` - Mark all gates as checked (if exists, create minimal PLAN.md if not)
- `.agents/v2/milestones/milestone-1-3-oauth-repoauthorized/PLAN.md` - Mark all gates as checked (if exists)
- `.agents/v2/milestones/milestone-1-4-bare-repo-command-flags/PLAN.md` - Mark all gates as checked (if exists)

Note: ROADMAP.md already has checkmarks for completed milestones. PLAN.md files should be consistent.

---

### Step 9: Update ROADMAP.md Status

**File:** `.agents/v2/ROADMAP.md`

**Change:** Update Milestone 1.6 status from "Needed" to "Complete"

**Update:**
```markdown
| **Compliance** | Documentation Alignment | Complete |
```

And mark acceptance gates:
```markdown
**Acceptance gates:**

- [x] OAuth `implementation_notes.md` exists
- [x] v1 ROADMAP.md correctly references v2 for OAuth
- [x] All PLAN.md acceptance gates reflect actual status
- [x] All completed milestones have implementation_notes.md
```

---

## Critical Files Summary

| File | Action | Purpose |
|------|--------|---------|
| `.agents/v2/milestones/milestone-1-1-doctor-fix/implementation_notes.md` | CREATE | Document doctor fix implementation |
| `.agents/v2/milestones/milestone-1-2-sync-restack/implementation_notes.md` | CREATE | Document sync restack implementation |
| `.agents/v2/milestones/milestone-1-3-oauth-repoauthorized/implementation_notes.md` | CREATE | Document RepoAuthorized implementation |
| `.agents/v2/milestones/milestone-1-4-bare-repo-command-flags/implementation_notes.md` | CREATE | Document bare repo flags implementation |
| `.agents/v2/milestones/milestone-1-5-event-ledger-completion/implementation_notes.md` | CREATE | Document event ledger implementation |
| `.agents/v2/milestones/milestone-1-github-app-oauth/implementation_notes.md` | CREATE | Document OAuth implementation |
| `.agents/v1/ROADMAP.md` | MODIFY | Add v2 reference |
| `.agents/v2/ROADMAP.md` | MODIFY | Update 1.6 status to Complete |

---

## implementation_notes.md Template

Each implementation_notes.md should follow this structure (derived from the existing `milestone-2-bare-repo-support/implementation_notes.md`):

```markdown
# Milestone X.X: Implementation Notes

## Summary

[1-2 sentence summary of what was implemented]

## Key Implementation Decisions

### 1. [Decision Topic]

[Explanation of the decision and rationale]

### 2. [Decision Topic]

[Explanation of the decision and rationale]

## Files Modified

### [Category]
- `path/to/file.rs` - [Brief description of changes]

## Test Coverage

[Description of tests added]

## Future Work

[Optional: items identified but not addressed in this milestone]
```

---

## Acceptance Gates

Per ROADMAP.md:

- [ ] OAuth `implementation_notes.md` exists
- [ ] v1 ROADMAP.md correctly references v2 for OAuth  
- [ ] All PLAN.md acceptance gates reflect actual status
- [ ] All completed milestones (1.1-1.5, OAuth, bare repo) have implementation_notes.md
- [ ] `cargo test` passes (no code changes, but verify)
- [ ] `cargo clippy` passes (no code changes, but verify)

---

## Verification Commands

```bash
# Verify all implementation_notes.md files exist
ls -la .agents/v2/milestones/*/implementation_notes.md

# Verify no code changes break the build
cargo check
cargo clippy -- -D warnings
cargo test

# Verify documentation files are valid markdown
# (visual inspection)
```

---

## Risk Assessment

**Very Low Risk** - This milestone:
- Creates documentation only
- No code changes
- No functional impact
- Cannot break tests or builds

**Potential Issues:**
- May need to research code to accurately document implementation decisions
- Historical context may be incomplete if not documented during implementation

**Mitigations:**
- Read code carefully to reconstruct implementation decisions
- Note any uncertainty in implementation_notes.md

---

## Research Phase

Before writing each implementation_notes.md, investigate:

1. **For each milestone:**
   - Read the PLAN.md to understand original intent
   - Read relevant source files to understand what was actually implemented
   - Check git history for the milestone (if commits are tagged/named)
   - Identify test files added

2. **Key questions to answer:**
   - What was the core change?
   - What design decisions were made?
   - Were there any deviations from the plan?
   - What test coverage exists?

---

## Execution Order

1. Research all completed milestones (read code, understand implementation)
2. Create implementation_notes.md for milestone-1-github-app-oauth
3. Create implementation_notes.md for milestone-1-1-doctor-fix
4. Create implementation_notes.md for milestone-1-2-sync-restack
5. Create implementation_notes.md for milestone-1-3-oauth-repoauthorized
6. Create implementation_notes.md for milestone-1-4-bare-repo-command-flags
7. Create implementation_notes.md for milestone-1-5-event-ledger-completion
8. Update v1 ROADMAP.md with v2 reference
9. Update PLAN.md acceptance gates (if needed)
10. Update v2 ROADMAP.md status to Complete
11. Verify with cargo check/test/clippy

---

## Notes

**Principles Applied:**

- **Code is Communication:** Documentation allows future developers to understand the codebase history
- **Simplicity:** Focus on documenting key decisions, not exhaustive detail
- **Follow the Leader:** Use the existing implementation_notes.md template (from milestone-2-bare-repo-support)

**Why This Matters:**

The CLAUDE.md file states:
> "Your plans (as markdown checklists) should be kept there, and should always be up-to-date with your progress. In addition, when you're finished with a task, you should make any notes about your particular implementation choices in a file called implementation_notes.md within the milestone folder under .agents."

This milestone brings the documentation into compliance with the project's own documentation standards.
