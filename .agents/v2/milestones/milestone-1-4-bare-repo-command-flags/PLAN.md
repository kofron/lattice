# Milestone 1.4: Bare Repo Command Flags

## Goal

**Wire the `--no-restack` and `--no-checkout` flags per SPEC.md Section 4.6.7 to enable safe operation in bare repositories.**

Bare repositories cannot perform rebases, checkouts, or any operation that uses index/worktree state. This milestone ensures that `submit`, `sync`, and `get` commands properly gate on `WorkingDirectoryAvailable` and provide explicit flag-based overrides for bare repo scenarios.

**Governing Principle:** Per CLAUDE.md "Reuse" - the infrastructure is complete (capabilities, requirement sets, error messages). We wire behavioral logic into existing commands.

---

## Background

### What Already Exists (Infrastructure Complete)

| Component | Location | Status |
|-----------|----------|--------|
| `WorkingDirectoryAvailable` capability | `src/engine/capabilities.rs` | Complete |
| `REMOTE_BARE_ALLOWED` requirement set | `src/engine/gate.rs` | Defined but unused |
| `no_working_directory()` issue | `src/engine/health.rs` | Complete with guidance |
| `is_ancestor()` function | `src/git/interface.rs` | Complete |
| `--no-restack` flag (submit) | `src/cli/args.rs` | Defined but not wired |
| `--restack` flag (sync) | `src/cli/args.rs` | Wired |
| Restack infrastructure | `src/cli/commands/restack.rs` | Fully tested |
| Bare repo detection | `src/engine/scan.rs` | Complete |

### What's Missing (Behavioral Logic)

Per SPEC.md Section 4.6.7:

1. **submit.rs**: No conditional logic based on `--no-restack`, no ancestry alignment check, no base metadata normalization
2. **sync.rs**: Needs to refuse in bare repos without `--no-restack`
3. **get.rs**: Missing `--no-checkout` flag entirely, no bare repo mode implementation
4. **args.rs**: Missing `--no-checkout` flag for `get` command

---

## Spec References

### SPEC.md Section 4.6.7 - Bare repo policy for submit/sync/get

**submit in bare repos:**
> - MUST refuse unless `--no-restack` is provided
> - Even with `--no-restack`, MUST refuse if submit set is not aligned
> - **Alignment is ancestry-based:** `p.tip` must be ancestor of `b.tip`
> - **Metadata normalization:** If ancestry holds but `b.base != p.tip`: normalize base to `p.tip` (metadata-only)

**sync in bare repos:**
> - MUST refuse unless `--no-restack` is provided
> - With `--no-restack`: may fetch, trunk FF, PR checks, branch deletion prompts

**get in bare repos:**
> - MUST refuse unless `--no-checkout` is provided
> - With `--no-checkout`: fetch, track branch with parent inference, compute base, default frozen
> - Print explicit guidance on how to create a worktree

### ARCHITECTURE.md Section 5.3 - Command requirement sets

> Commands MUST declare whether they require `WorkingDirectoryAvailable`.

### Existing Error Message (health.rs lines 631-649)

```
This command requires a working directory, but this is a bare repository.

To proceed, either:
• Create a worktree: git worktree add <path> <branch>
• Run from an existing worktree linked to this repository
• Use --no-checkout or --no-restack flags if available for this command
```

---

## Implementation Steps

### Step 1: Add `--no-checkout` Flag to Get Command

**File:** `src/cli/args.rs`

**Location:** In the `Get` command struct (approximately lines 750-780)

Add the flag definition:

```rust
/// Fetch and track branch without checking out (required for bare repos).
/// Creates tracking metadata and computes base, but does not modify working directory.
#[arg(long)]
no_checkout: bool,
```

**Location:** In the command dispatch match arm for `Get`

Pass the new parameter to the `get()` function call.

---

### Step 2: Update Get Command Signature and Implementation

**File:** `src/cli/commands/get.rs`

**Update function signature:**

```rust
pub fn get(
    ctx: &Context,
    target: &str,
    downstack: bool,
    force: bool,
    restack: bool,
    unfrozen: bool,
    no_checkout: bool,  // NEW parameter
) -> Result<()>
```

**Add bare repo gating logic** at the start of the function:

```rust
// Per SPEC.md §4.6.7: get MUST refuse in bare repos unless --no-checkout
let is_bare = ctx.repo_info.work_dir.is_none();
if is_bare && !no_checkout {
    anyhow::bail!(
        "This is a bare repository. The `get` command requires a working directory.\n\n\
         To fetch and track the branch without checkout, use:\n\
         \n\
         lattice get --no-checkout {}\n\
         \n\
         After tracking, you can create a worktree to work on it:\n\
         \n\
         git worktree add <path> {}",
        target, target
    );
}
```

**Implement no-checkout mode:**

```rust
if no_checkout {
    // Per SPEC.md §4.6.7: fetch, track, compute base, default frozen
    
    // 1. Fetch the branch ref from remote
    git.fetch_branch(remote, branch_name)?;
    
    // 2. Create/update the local branch ref
    let remote_ref = format!("refs/remotes/{}/{}", remote, branch_name);
    let branch_tip = git.resolve_ref(&remote_ref)?;
    git.update_ref(&format!("refs/heads/{}", branch_name), &branch_tip, None)?;
    
    // 3. Determine parent (from PR base or trunk)
    let parent = determine_parent_from_pr_or_trunk(ctx, branch_name)?;
    let parent_tip = git.resolve_branch_tip(&parent)?;
    
    // 4. Compute base as merge-base(branch_tip, parent_tip)
    let base = git.merge_base(&branch_tip, &parent_tip)?;
    
    // 5. Track with metadata, default frozen unless --unfrozen
    let freeze_state = if unfrozen {
        FreezeState::Unfrozen
    } else {
        FreezeState::Frozen {
            scope: FreezeScope::Single,
            reason: "fetched in no-checkout mode".to_string(),
            frozen_at: Utc::now(),
        }
    };
    
    write_branch_metadata(ctx, branch_name, &parent, &base, freeze_state)?;
    
    // 6. Print worktree guidance
    println!("Tracked branch '{}' with parent '{}'", branch_name, parent);
    println!("Branch is {} by default.", if unfrozen { "unfrozen" } else { "frozen" });
    println!();
    println!("To work on this branch, create a worktree:");
    println!("  git worktree add <path> {}", branch_name);
    
    return Ok(());
}
```

---

### Step 3: Wire `--no-restack` in Submit Command

**File:** `src/cli/commands/submit.rs`

**Add bare repo gating logic** at the start of the function:

```rust
// Per SPEC.md §4.6.7: submit MUST refuse in bare repos unless --no-restack
let is_bare = ctx.repo_info.work_dir.is_none();
if is_bare && !no_restack {
    anyhow::bail!(
        "This is a bare repository. The `submit` command requires a working directory for restacking.\n\n\
         To submit without restacking (branches must be properly aligned), use:\n\
         \n\
         lattice submit --no-restack\n\
         \n\
         Note: Branches must satisfy ancestry alignment (parent tip is ancestor of branch tip).\n\
         If alignment fails, you'll need to restack from a worktree first."
    );
}
```

**Add ancestry alignment check** (when `--no-restack` is provided in a bare repo):

```rust
// Per SPEC.md §4.6.7: Even with --no-restack, submit MUST check alignment in bare repos
if is_bare && no_restack {
    let alignment_result = check_submit_alignment(ctx, &branches_to_submit)?;
    
    match alignment_result {
        AlignmentResult::Aligned => {
            // All good, proceed with submit
        }
        AlignmentResult::NeedsNormalization(branches) => {
            // Ancestry holds but base != parent.tip - normalize metadata
            normalize_base_metadata(ctx, &branches)?;
            if !ctx.quiet {
                println!(
                    "Updated base metadata for {} branches (no history changes).",
                    branches.len()
                );
            }
        }
        AlignmentResult::NotAligned(branch, parent) => {
            anyhow::bail!(
                "Branch '{}' is not aligned with parent '{}'.\n\n\
                 The parent's tip is not an ancestor of the branch tip, which means\n\
                 the branch needs to be rebased.\n\n\
                 Restack required. Run from a worktree and re-run `lattice submit`.",
                branch, parent
            );
        }
    }
}
```

**Implement alignment check helper:**

```rust
/// Result of checking submit alignment for bare repo mode.
enum AlignmentResult {
    /// All branches are aligned (parent.tip is ancestor of branch.tip, base matches)
    Aligned,
    /// Ancestry holds but base needs normalization (metadata-only update)
    NeedsNormalization(Vec<BranchNormalization>),
    /// Ancestry violated - restack required
    NotAligned(String, String), // (branch_name, parent_name)
}

struct BranchNormalization {
    branch: String,
    new_base: Oid,
}

/// Check if all branches in submit set are aligned for bare repo submission.
/// Per SPEC.md §4.6.7: p.tip must be ancestor of b.tip.
fn check_submit_alignment(
    ctx: &Context,
    branches: &[String],
) -> Result<AlignmentResult> {
    let git = &ctx.git;
    let mut needs_normalization = Vec::new();
    
    for branch in branches {
        let metadata = read_branch_metadata(ctx, branch)?;
        let parent = &metadata.parent;
        
        // Skip trunk (it has no parent to align with)
        if parent == ctx.trunk {
            continue;
        }
        
        let branch_tip = git.resolve_branch_tip(branch)?;
        let parent_tip = git.resolve_branch_tip(parent)?;
        
        // Check ancestry: parent.tip must be ancestor of branch.tip
        if !git.is_ancestor(&parent_tip, &branch_tip)? {
            return Ok(AlignmentResult::NotAligned(
                branch.clone(),
                parent.clone(),
            ));
        }
        
        // If ancestry holds but base differs, needs normalization
        if metadata.base != parent_tip {
            needs_normalization.push(BranchNormalization {
                branch: branch.clone(),
                new_base: parent_tip,
            });
        }
    }
    
    if needs_normalization.is_empty() {
        Ok(AlignmentResult::Aligned)
    } else {
        Ok(AlignmentResult::NeedsNormalization(needs_normalization))
    }
}

/// Normalize base metadata for branches where ancestry holds but base differs.
/// This is a metadata-only operation - no history rewrite.
fn normalize_base_metadata(
    ctx: &Context,
    normalizations: &[BranchNormalization],
) -> Result<()> {
    for norm in normalizations {
        let mut metadata = read_branch_metadata(ctx, &norm.branch)?;
        metadata.base = norm.new_base.clone();
        write_branch_metadata_update(ctx, &norm.branch, &metadata)?;
    }
    Ok(())
}
```

---

### Step 4: Wire `--no-restack` in Sync Command

**File:** `src/cli/commands/sync.rs`

**Add bare repo gating logic** near the start of the function:

```rust
// Per SPEC.md §4.6.7: sync MUST refuse in bare repos unless --no-restack
// Note: sync uses positive `restack` flag, so we check the inverse
let is_bare = ctx.repo_info.work_dir.is_none();
if is_bare && restack {
    anyhow::bail!(
        "This is a bare repository. The `sync` command cannot restack without a working directory.\n\n\
         To sync without restacking (fetch, trunk FF, PR checks only), use:\n\
         \n\
         lattice sync --no-restack"
    );
}
```

**Note:** Sync already has the `restack` parameter wired (from milestone 1.2). The additional check ensures bare repos cannot use `--restack`.

---

### Step 5: Update Command Gating (Conditional Requirement Sets)

**File:** `src/cli/commands/mod.rs` (or wherever command dispatch happens)

The commands need to conditionally gate based on whether bare-repo-compatible flags are provided.

**For submit:**
```rust
// Use REMOTE_BARE_ALLOWED when --no-restack is provided, otherwise REMOTE
let requirement_set = if no_restack {
    gate::REMOTE_BARE_ALLOWED
} else {
    gate::REMOTE
};
```

**For sync:**
```rust
// Use REMOTE_BARE_ALLOWED when --no-restack mode (restack=false), otherwise REMOTE
let requirement_set = if !restack {
    gate::REMOTE_BARE_ALLOWED
} else {
    gate::REMOTE
};
```

**For get:**
```rust
// Use REMOTE_BARE_ALLOWED when --no-checkout is provided, otherwise REMOTE
let requirement_set = if no_checkout {
    gate::REMOTE_BARE_ALLOWED
} else {
    gate::REMOTE
};
```

---

### Step 6: Add Helper Functions for Parent Inference

**File:** `src/cli/commands/get.rs` (or utility module)

```rust
/// Determine parent branch for a fetched branch.
/// Per SPEC.md §4.6.7: use PR base branch or fall back to trunk.
fn determine_parent_from_pr_or_trunk(
    ctx: &Context,
    branch_name: &str,
) -> Result<String> {
    // Try to get PR info from GitHub API
    if let Some(pr) = find_pr_by_head(ctx, branch_name)? {
        // Use PR base branch as parent
        return Ok(pr.base.ref_name);
    }
    
    // Fall back to trunk
    Ok(ctx.trunk.clone())
}
```

---

### Step 7: Update ROADMAP.md

**File:** `.agents/v2/ROADMAP.md`

**Update milestone 1.4 status from "Not wired" to "Complete"**

**Mark all acceptance gates:**

```markdown
### Milestone 1.4: Bare Repo Command Flags

**Status:** Complete

...

**Acceptance gates (per SPEC.md Section 4.6.7):**

- [x] `lattice submit` refuses in bare repo without `--no-restack`
- [x] `lattice submit --no-restack` checks ancestry alignment
- [x] `lattice submit --no-restack` normalizes base metadata if ancestry holds
- [x] `lattice submit --no-restack` refuses if ancestry violated with message
- [x] `lattice sync` refuses in bare repo without `--no-restack`
- [x] `lattice sync --no-restack` performs fetch, trunk FF, PR checks only
- [x] `lattice get` refuses in bare repo without `--no-checkout`
- [x] `lattice get --no-checkout` fetches, tracks, computes base, defaults frozen
- [x] `lattice get --no-checkout` prints worktree creation guidance
- [x] `cargo test` passes
- [x] `cargo clippy` passes
```

---

## Critical Files Summary

| File | Action | Purpose |
|------|--------|---------|
| `src/cli/args.rs` | MODIFY | Add `--no-checkout` flag to Get command |
| `src/cli/commands/get.rs` | MODIFY | Add no-checkout mode implementation |
| `src/cli/commands/submit.rs` | MODIFY | Wire `--no-restack` with alignment check |
| `src/cli/commands/sync.rs` | MODIFY | Add bare repo refusal without `--no-restack` |
| `src/cli/commands/mod.rs` | MODIFY | Conditional requirement set selection |
| `.agents/v2/ROADMAP.md` | MODIFY | Update status to Complete |

---

## Acceptance Gates

Per SPEC.md Section 4.6.7 and ROADMAP.md:

- [ ] `lattice submit` refuses in bare repo without `--no-restack`
- [ ] `lattice submit --no-restack` checks ancestry alignment
- [ ] `lattice submit --no-restack` normalizes base metadata if ancestry holds
- [ ] `lattice submit --no-restack` refuses if ancestry violated with message
- [ ] `lattice sync` refuses in bare repo without `--no-restack`
- [ ] `lattice sync --no-restack` performs fetch, trunk FF, PR checks only
- [ ] `lattice get` refuses in bare repo without `--no-checkout`
- [ ] `lattice get --no-checkout` fetches, tracks, computes base, defaults frozen
- [ ] `lattice get --no-checkout` prints worktree creation guidance
- [ ] `cargo test` passes
- [ ] `cargo clippy` passes

---

## Testing Rubric

### Unit Tests

| Test | File | Purpose |
|------|------|---------|
| `test_alignment_check_aligned` | `src/cli/commands/submit.rs` | Verify aligned branches pass |
| `test_alignment_check_needs_normalization` | `src/cli/commands/submit.rs` | Verify metadata normalization triggered |
| `test_alignment_check_not_aligned` | `src/cli/commands/submit.rs` | Verify ancestry violation detected |
| `test_normalize_base_metadata` | `src/cli/commands/submit.rs` | Verify metadata-only update works |

### Integration Tests

| Test | File | Purpose |
|------|------|---------|
| `test_submit_refuses_bare_without_flag` | `tests/integration/bare_repo.rs` | Verify submit refuses in bare repo |
| `test_submit_no_restack_checks_alignment` | `tests/integration/bare_repo.rs` | Verify alignment check runs |
| `test_submit_no_restack_normalizes_metadata` | `tests/integration/bare_repo.rs` | Verify normalization works |
| `test_sync_refuses_bare_without_flag` | `tests/integration/bare_repo.rs` | Verify sync refuses in bare repo |
| `test_get_refuses_bare_without_flag` | `tests/integration/bare_repo.rs` | Verify get refuses in bare repo |
| `test_get_no_checkout_tracks_branch` | `tests/integration/bare_repo.rs` | Verify no-checkout mode works |
| `test_get_no_checkout_defaults_frozen` | `tests/integration/bare_repo.rs` | Verify frozen default |
| `test_get_no_checkout_prints_guidance` | `tests/integration/bare_repo.rs` | Verify worktree guidance printed |

### Existing Pitfall Tests (from SPEC.md Section 9.2)

| Test | Expected |
|------|----------|
| Bare repo: read-only commands succeed | `log`, `info`, `parent` work |
| Bare repo: workdir-required commands refuse with guidance | Clear error message |
| Bare repo: `submit --no-restack` alignment check and metadata normalization | Per §4.6.7 |
| Bare repo: `get --no-checkout` tracking behavior | Fetch, track, compute base |

---

## Verification Commands

```bash
# Build and type check
cargo check

# Lint
cargo clippy -- -D warnings

# Run all tests
cargo test

# Run specific bare repo tests
cargo test bare_repo::

# Run submit tests
cargo test submit::

# Run sync tests  
cargo test sync::

# Run get tests
cargo test get::

# Format check
cargo fmt --check
```

---

## Risk Assessment

**Low-Medium risk** - This milestone:
- Builds entirely on existing infrastructure
- No new async code or caching layers
- Clear spec requirements with deterministic behavior
- Well-defined error messages already exist

**Potential Issues:**
- Metadata CAS updates during normalization need to handle concurrent access
- PR parent inference may fail if GitHub API unavailable (fall back to trunk)

**Mitigations:**
- Use existing metadata update patterns with CAS semantics
- Always have trunk as fallback parent

---

## Dependencies

No new dependencies required. Uses existing:
- `git.is_ancestor()` for alignment check
- `git.merge_base()` for base computation
- Metadata read/write infrastructure
- PR lookup infrastructure

---

## Notes

**Principles Applied:**

- **Reuse:** Leverages existing `WorkingDirectoryAvailable` capability, `REMOTE_BARE_ALLOWED` requirement set, error messages, and git interface methods
- **Follow the Leader:** Implements exactly what SPEC.md Section 4.6.7 specifies
- **Simplicity:** Behavioral logic only - no new abstractions or infrastructure
- **Purity:** Alignment check is pure function over repository state
- **Tests are Everything:** Comprehensive coverage per SPEC.md Section 9.2 pitfall tests

**Key Insight:**

The infrastructure built in milestones 1.1-1.3 (capabilities, gating, scanner) makes this milestone straightforward. We're essentially "filling in the blanks" with behavioral logic that the architecture was designed to support.

**Sequence Matters:**

1. First add `--no-checkout` flag definition (args.rs)
2. Then implement get no-checkout mode
3. Then wire submit alignment check
4. Then add sync bare repo check
5. Finally update command gating and tests
