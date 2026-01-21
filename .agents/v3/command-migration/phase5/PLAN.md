# Phase 5: Core Mutating Commands Migration

## Status: PLANNING

**Started:** 2026-01-21  
**Branch:** `jared-fix-ledger-bug`

---

## Overview

Phase 5 migrates the core mutating commands to implement the `Command` trait and flow through the unified `run_command()` lifecycle in `runner.rs`. These commands are the most complex in the codebase, involving multi-branch rebases, conflict handling, and journal-based recovery.

### Goals

1. All Phase 5 commands implement `Command` trait
2. All mutations expressed as `PlanStep` variants with CAS semantics
3. Conflict handling delegated to executor (via `PotentialConflictPause`)
4. No direct `scan()` calls in command code
5. Engine hooks fire for all commands (Milestone 0.12 requirement)

### Commands in Scope

| Command | File | Complexity | Current Pattern |
|---------|------|------------|-----------------|
| `restack` | `restack.rs` | **CRITICAL** | Legacy journal, direct scan |
| `create` | `create.rs` | MEDIUM | run_gated, direct metadata writes |
| `modify` | `modify.rs` | HIGH | Legacy journal, rebase helper |
| `delete` | `delete.rs` | HIGH | Legacy journal, orphan handling |
| `rename` | `rename.rs` | HIGH | Multiple metadata updates |
| `squash` | `squash.rs` | MEDIUM | Interactive rebase |
| `fold` | `fold.rs` | HIGH | Merge + delete |
| `move` | `move_cmd.rs` | HIGH | Re-parent + rebase |
| `pop` | `pop.rs` | MEDIUM | Delete + preserve changes |
| `reorder` | `reorder.rs` | HIGH | Interactive editor |
| `split` | `split.rs` | HIGH | Multiple branches from one |
| `revert` | `revert.rs` | MEDIUM | Create revert branch |

---

## Architecture Reference

### Command Lifecycle (ARCHITECTURE.md Section 12)

```
Scan → Gate → [Repair if needed] → Plan → Execute → Verify → Return
```

### Key Constraints

1. **Commands cannot call `scan()` directly** - Use `ctx.snapshot` from `ReadyContext`
2. **All mutations via PlanSteps** - No direct git calls or metadata writes
3. **CAS semantics required** - All ref/metadata updates include expected old values
4. **Conflict handling by executor** - Commands mark `PotentialConflictPause`, executor handles
5. **Pure `plan()` function** - No I/O, deterministic from snapshot

### Available PlanStep Variants

```rust
PlanStep::UpdateRefCas { refname, old_oid, new_oid, reason }
PlanStep::DeleteRefCas { refname, old_oid, reason }
PlanStep::WriteMetadataCas { branch, old_ref_oid, metadata }
PlanStep::DeleteMetadataCas { branch, old_ref_oid }
PlanStep::RunGit { args, description, expected_effects }
PlanStep::Checkpoint { name }
PlanStep::PotentialConflictPause { branch, git_operation }
PlanStep::Checkout { branch, reason }
```

### Runner Entry Points

- `run_command(&cmd, &git, ctx)` - Standard single-scope commands
- `run_command_with_scope(&cmd, &git, ctx, target)` - Multi-branch stack operations
- `run_command_with_requirements(&cmd, &git, ctx, reqs)` - Mode-dependent requirements

---

## Implementation Order

Commands are ordered by dependency and complexity to establish patterns early.

### Task 5.1: `restack` (REFERENCE IMPLEMENTATION) - CRITICAL

**Why first:** Most complex, establishes pattern for all others. All other multi-branch commands follow this template.

**File:** `src/cli/commands/restack.rs`

**Current Implementation:**
- Direct `scan()` call
- Manual `RepoLock::acquire()`
- Manual `Journal::new("restack")`
- Direct `git rebase` via `Command::new("git")`
- Inline conflict detection and pause

**Target Implementation:**

```rust
pub struct RestackCommand {
    branch: Option<String>,
    only: bool,
    downstack: bool,
}

impl Command for RestackCommand {
    const REQUIREMENTS: &'static RequirementSet = &requirements::MUTATING;
    type Output = RestackResult;

    fn plan(&self, ctx: &ReadyContext) -> Result<Plan, PlanError> {
        // Get scope from validated data (provided by run_command_with_scope)
        let scope = match &ctx.data {
            ValidatedData::StackScope { trunk, branches } => branches,
            _ => return Err(PlanError::InvalidState("Missing scope".into())),
        };
        
        let mut plan = Plan::new(OpId::new(), "restack");
        
        for branch in scope.iter() {
            // Skip if already aligned
            let meta = ctx.snapshot.metadata(branch)?;
            let parent_tip = ctx.snapshot.branch_tip(&meta.parent)?;
            
            if meta.base == parent_tip {
                continue; // Already aligned
            }
            
            // Check freeze
            if meta.frozen.is_frozen() {
                return Err(PlanError::FrozenBranch(branch.clone()));
            }
            
            let branch_tip = ctx.snapshot.branch_tip(branch)?;
            
            // Checkpoint before rebase
            plan = plan.with_step(PlanStep::Checkpoint {
                name: format!("before-restack-{}", branch),
            });
            
            // Git rebase operation
            plan = plan.with_step(PlanStep::RunGit {
                args: vec![
                    "rebase".into(),
                    "--onto".into(),
                    parent_tip.to_string(),
                    meta.base.to_string(),
                    branch.to_string(),
                ],
                description: format!("Rebase {} onto {}", branch, parent_tip),
                expected_effects: vec![format!("refs/heads/{}", branch)],
            });
            
            // Mark potential conflict point
            plan = plan.with_step(PlanStep::PotentialConflictPause {
                branch: branch.to_string(),
                git_operation: "rebase".to_string(),
            });
            
            // Update metadata with new base
            let updated_meta = meta.with_base(parent_tip.clone());
            plan = plan.with_step(PlanStep::WriteMetadataCas {
                branch: branch.to_string(),
                old_ref_oid: Some(ctx.snapshot.metadata_ref_oid(branch)?),
                metadata: Box::new(updated_meta),
            });
        }
        
        Ok(plan)
    }

    fn finish(&self, result: ExecuteResult) -> CommandOutput<RestackResult> {
        match result {
            ExecuteResult::Success { fingerprint } => {
                CommandOutput::Success(RestackResult { branches_restacked: ... })
            }
            ExecuteResult::Paused { branch, git_state, .. } => {
                CommandOutput::Paused {
                    message: format!(
                        "Conflict while restacking '{}'. Resolve and run 'lattice continue'.",
                        branch
                    ),
                }
            }
            ExecuteResult::Aborted { error, .. } => {
                CommandOutput::Failed { error }
            }
        }
    }
}

// Entry point
pub fn restack(ctx: &Context, branch: Option<&str>, only: bool, downstack: bool) -> Result<()> {
    let git = Git::open(&ctx.cwd()?)?;
    let target = branch.map(String::from);
    
    let cmd = RestackCommand {
        branch: target.clone(),
        only,
        downstack,
    };
    
    // Use run_command_with_scope for multi-branch operations
    let output = run_command_with_scope(&cmd, &git, ctx, target.as_deref())?;
    
    match output {
        CommandOutput::Success(result) => {
            // Print success message
            Ok(())
        }
        CommandOutput::Paused { message } => {
            println!("{}", message);
            Ok(())
        }
        CommandOutput::Failed { error } => Err(anyhow!("{}", error)),
    }
}
```

**Key Changes:**
1. Remove direct `scan()` call - use `ctx.snapshot`
2. Remove manual `RepoLock` - executor handles
3. Remove manual `Journal` - executor handles
4. Remove inline conflict detection - use `PotentialConflictPause`
5. All git rebases via `PlanStep::RunGit`
6. All metadata updates via `PlanStep::WriteMetadataCas`

**Acceptance Criteria:**
- [ ] `RestackCommand` struct implements `Command`
- [ ] Entry point calls `run_command_with_scope()`
- [ ] No direct `scan()` calls
- [ ] Plan includes Checkpoint + RunGit + PotentialConflictPause + WriteMetadataCas per branch
- [ ] Conflict pauses correctly with guidance message
- [ ] `lattice continue` resumes from pause
- [ ] All existing tests pass
- [ ] `cargo clippy` passes

---

### Task 5.2: `create`

**File:** `src/cli/commands/create.rs`

**Current Pattern:** Uses `run_gated()`, direct metadata writes

**Key Operations:**
1. Stage changes (optional)
2. Create branch from current HEAD
3. Create commit (optional, empty branch if no changes)
4. Write metadata
5. Reparent child (if `--insert`)

**Target Pattern:**

```rust
pub struct CreateCommand<'a> {
    name: Option<&'a str>,
    message: Option<&'a str>,
    insert: bool,
    // ... other flags
}

impl Command for CreateCommand<'_> {
    const REQUIREMENTS: &'static RequirementSet = &requirements::MUTATING;
    type Output = String; // New branch name

    fn plan(&self, ctx: &ReadyContext) -> Result<Plan, PlanError> {
        let mut plan = Plan::new(OpId::new(), "create");
        
        // Determine branch name
        let branch_name = self.resolve_branch_name(ctx)?;
        let current = ctx.snapshot.current_branch()?;
        let current_tip = ctx.snapshot.branch_tip(&current)?;
        
        // Create branch
        plan = plan.with_step(PlanStep::RunGit {
            args: vec!["checkout".into(), "-b".into(), branch_name.clone()],
            description: format!("Create branch {}", branch_name),
            expected_effects: vec![
                format!("refs/heads/{}", branch_name),
                "HEAD".to_string(),
            ],
        });
        
        // Create commit if staged changes exist
        if self.has_staged_changes(ctx) {
            plan = plan.with_step(PlanStep::RunGit {
                args: vec!["commit".into(), "-m".into(), message],
                description: "Create commit".into(),
                expected_effects: vec![format!("refs/heads/{}", branch_name)],
            });
        }
        
        // Write metadata for new branch
        let metadata = BranchMetadata::new(current.clone(), current_tip.clone());
        plan = plan.with_step(PlanStep::WriteMetadataCas {
            branch: branch_name.clone(),
            old_ref_oid: None, // New branch, no existing metadata
            metadata: Box::new(metadata),
        });
        
        // Handle --insert: reparent child to new branch
        if self.insert {
            if let Some(child) = self.determine_child_to_insert(ctx)? {
                let child_meta = ctx.snapshot.metadata(&child)?;
                let updated_meta = child_meta.with_parent(branch_name.clone());
                plan = plan.with_step(PlanStep::WriteMetadataCas {
                    branch: child.clone(),
                    old_ref_oid: Some(ctx.snapshot.metadata_ref_oid(&child)?),
                    metadata: Box::new(updated_meta),
                });
            }
        }
        
        Ok(plan)
    }
}
```

**Acceptance Criteria:**
- [ ] `CreateCommand` struct implements `Command`
- [ ] Empty branch creation works (no commit step)
- [ ] `--insert` reparenting works
- [ ] Metadata written with correct parent + base
- [ ] All existing tests pass

---

### Task 5.3: `modify`

**File:** `src/cli/commands/modify.rs`

**Current Pattern:** Legacy journal, uses `rebase_onto_with_journal()` helper

**Key Operations:**
1. Stage changes
2. Amend HEAD or create new commit
3. Auto-restack all descendants

**Target Pattern:**

```rust
pub struct ModifyCommand<'a> {
    create: bool,
    message: Option<&'a str>,
    edit: bool,
    // ... staging flags
}

impl Command for ModifyCommand<'_> {
    const REQUIREMENTS: &'static RequirementSet = &requirements::MUTATING;
    type Output = ();

    fn plan(&self, ctx: &ReadyContext) -> Result<Plan, PlanError> {
        let current = ctx.snapshot.current_branch()?;
        let current_meta = ctx.snapshot.metadata(&current)?;
        
        // Check freeze
        if current_meta.frozen.is_frozen() {
            return Err(PlanError::FrozenBranch(current.clone()));
        }
        
        let mut plan = Plan::new(OpId::new(), "modify");
        
        // Commit step (amend or create)
        let commit_args = if self.create {
            vec!["commit".into(), "-m".into(), self.message.unwrap_or("").into()]
        } else {
            vec!["commit".into(), "--amend".into(), "--no-edit".into()]
        };
        
        plan = plan.with_step(PlanStep::RunGit {
            args: commit_args,
            description: "Modify commit".into(),
            expected_effects: vec![format!("refs/heads/{}", current)],
        });
        
        // Get descendants and add restack steps for each
        let descendants = ctx.snapshot.descendants(&current)?;
        
        for descendant in descendants {
            let desc_meta = ctx.snapshot.metadata(&descendant)?;
            
            if desc_meta.frozen.is_frozen() {
                return Err(PlanError::FrozenBranch(descendant.clone()));
            }
            
            // Note: After modify, current branch tip changed
            // Executor will resolve actual OIDs during execution
            plan = plan.with_step(PlanStep::Checkpoint {
                name: format!("before-restack-{}", descendant),
            });
            
            plan = plan.with_step(PlanStep::RunGit {
                args: vec![
                    "rebase".into(),
                    "--onto".into(),
                    "HEAD".into(), // Will be resolved during execution
                    desc_meta.base.to_string(),
                    descendant.to_string(),
                ],
                description: format!("Restack {}", descendant),
                expected_effects: vec![format!("refs/heads/{}", descendant)],
            });
            
            plan = plan.with_step(PlanStep::PotentialConflictPause {
                branch: descendant.to_string(),
                git_operation: "rebase".to_string(),
            });
            
            // Metadata update for descendant
            // Note: new base will be the modified branch's new tip
            plan = plan.with_step(PlanStep::WriteMetadataCas {
                branch: descendant.to_string(),
                old_ref_oid: Some(ctx.snapshot.metadata_ref_oid(&descendant)?),
                metadata: Box::new(desc_meta.with_base_deferred()), // Special handling needed
            });
        }
        
        Ok(plan)
    }
}
```

**Note:** Modify has a complexity: the current branch's new tip isn't known until after the commit runs. The plan either needs:
1. A special `DeferredRef` value that executor resolves
2. Or split into pre-commit and post-commit phases

**Acceptance Criteria:**
- [ ] `ModifyCommand` struct implements `Command`
- [ ] Amend mode works
- [ ] Create mode (`-c`) works
- [ ] Descendant restack included in plan
- [ ] Conflict handling works
- [ ] All existing tests pass

---

### Task 5.4: `delete`

**File:** `src/cli/commands/delete.rs`

**Key Operations:**
1. Determine deletion scope (`--upstack`, `--downstack`)
2. Reparent children to deleted branch's parent
3. Checkout parent if currently on deleted branch
4. Delete git refs
5. Delete metadata refs

**Target Pattern:**

```rust
pub struct DeleteCommand<'a> {
    branch: Option<&'a str>,
    force: bool,
    upstack: bool,
    downstack: bool,
}

impl Command for DeleteCommand<'_> {
    const REQUIREMENTS: &'static RequirementSet = &requirements::MUTATING;
    type Output = Vec<String>; // Deleted branches

    fn plan(&self, ctx: &ReadyContext) -> Result<Plan, PlanError> {
        let target = self.resolve_target(ctx)?;
        let target_meta = ctx.snapshot.metadata(&target)?;
        
        let mut plan = Plan::new(OpId::new(), "delete");
        
        // Determine deletion set
        let to_delete = self.compute_deletion_set(ctx, &target)?;
        
        // Reparent children of deleted branches to their grandparent
        for branch in &to_delete {
            let children = ctx.snapshot.children(branch)?;
            let branch_meta = ctx.snapshot.metadata(branch)?;
            
            for child in children {
                if to_delete.contains(&child) {
                    continue; // Child also being deleted
                }
                
                let child_meta = ctx.snapshot.metadata(&child)?;
                let updated = child_meta.with_parent(branch_meta.parent.clone());
                
                plan = plan.with_step(PlanStep::WriteMetadataCas {
                    branch: child.clone(),
                    old_ref_oid: Some(ctx.snapshot.metadata_ref_oid(&child)?),
                    metadata: Box::new(updated),
                });
            }
        }
        
        // Checkout parent if on deleted branch
        let current = ctx.snapshot.current_branch()?;
        if to_delete.contains(&current) {
            plan = plan.with_step(PlanStep::Checkout {
                branch: target_meta.parent.clone(),
                reason: "Checking out parent before deletion".into(),
            });
        }
        
        // Delete branches (leaves first for safety)
        let ordered = topological_sort_reverse(&to_delete, ctx)?;
        for branch in ordered {
            let tip = ctx.snapshot.branch_tip(&branch)?;
            
            // Delete git ref
            plan = plan.with_step(PlanStep::DeleteRefCas {
                refname: format!("refs/heads/{}", branch),
                old_oid: Some(tip.to_string()),
                reason: format!("Delete branch {}", branch),
            });
            
            // Delete metadata ref
            plan = plan.with_step(PlanStep::DeleteMetadataCas {
                branch: branch.clone(),
                old_ref_oid: Some(ctx.snapshot.metadata_ref_oid(&branch)?),
            });
        }
        
        Ok(plan)
    }
}
```

**Acceptance Criteria:**
- [ ] `DeleteCommand` struct implements `Command`
- [ ] Children reparented correctly
- [ ] `--upstack` includes descendants
- [ ] `--downstack` includes ancestors (except trunk)
- [ ] Checkout before delete if on deleted branch
- [ ] All existing tests pass

---

### Task 5.5: `rename`

**File:** `src/cli/commands/rename.rs`

**Key Operations:**
1. Rename git branch ref
2. Rename metadata ref
3. Update all parent pointers that reference old name

**Target Pattern:**

```rust
impl Command for RenameCommand<'_> {
    fn plan(&self, ctx: &ReadyContext) -> Result<Plan, PlanError> {
        let mut plan = Plan::new(OpId::new(), "rename");
        
        let old_name = self.resolve_source(ctx)?;
        let new_name = self.new_name;
        
        // Rename git branch
        plan = plan.with_step(PlanStep::RunGit {
            args: vec!["branch".into(), "-m".into(), old_name.clone(), new_name.into()],
            description: format!("Rename {} to {}", old_name, new_name),
            expected_effects: vec![
                format!("refs/heads/{}", old_name),
                format!("refs/heads/{}", new_name),
            ],
        });
        
        // Create new metadata ref (copy of old with updated refs)
        let old_meta = ctx.snapshot.metadata(&old_name)?;
        plan = plan.with_step(PlanStep::WriteMetadataCas {
            branch: new_name.to_string(),
            old_ref_oid: None, // New ref
            metadata: Box::new(old_meta.clone()),
        });
        
        // Delete old metadata ref
        plan = plan.with_step(PlanStep::DeleteMetadataCas {
            branch: old_name.clone(),
            old_ref_oid: Some(ctx.snapshot.metadata_ref_oid(&old_name)?),
        });
        
        // Update all children's parent pointers
        let children = ctx.snapshot.children(&old_name)?;
        for child in children {
            let child_meta = ctx.snapshot.metadata(&child)?;
            let updated = child_meta.with_parent(new_name.to_string());
            
            plan = plan.with_step(PlanStep::WriteMetadataCas {
                branch: child.clone(),
                old_ref_oid: Some(ctx.snapshot.metadata_ref_oid(&child)?),
                metadata: Box::new(updated),
            });
        }
        
        Ok(plan)
    }
}
```

**Acceptance Criteria:**
- [ ] `RenameCommand` struct implements `Command`
- [ ] Git branch renamed
- [ ] Metadata moved to new name
- [ ] All children's parent pointers updated
- [ ] All existing tests pass

---

### Tasks 5.6-5.12: Remaining Commands

Apply the same pattern to:

| Task | Command | Key Considerations |
|------|---------|-------------------|
| 5.6 | `squash` | Interactive rebase to single commit |
| 5.7 | `fold` | Merge into parent, delete self, reparent children |
| 5.8 | `move` | Change parent, rebase onto new parent |
| 5.9 | `pop` | Delete branch, keep changes uncommitted |
| 5.10 | `reorder` | Editor-based, multiple rebases |
| 5.11 | `split` | Create multiple branches from one |
| 5.12 | `revert` | Create new branch with revert commit |

Each follows the pattern:
1. Struct with args
2. `const REQUIREMENTS = &requirements::MUTATING`
3. Pure `plan()` generating PlanSteps
4. `finish()` handling Success/Paused/Aborted

---

## Testing Strategy

### Unit Tests (Per Command)

For each migrated command:
1. **Plan generation test** - Given snapshot, verify plan contains expected steps
2. **CAS precondition test** - Verify all steps have old_oid/old_ref_oid
3. **Freeze check test** - Verify frozen branches cause PlanError

### Integration Tests (Existing)

All existing integration tests must pass unchanged. These validate user-facing behavior.

### New Architecture Tests

```rust
#[test]
fn phase5_commands_cannot_import_scan() {
    for file in ["restack.rs", "create.rs", "modify.rs", ...] {
        let content = std::fs::read_to_string(format!("src/cli/commands/{}", file))?;
        assert!(!content.contains("use crate::engine::scan"));
    }
}

#[test]
fn phase5_commands_implement_command_trait() {
    // Verify struct + impl blocks exist
}
```

---

## Verification Checklist

Before marking Phase 5 complete:

- [ ] All 12 commands implement `Command` trait
- [ ] All entry points use `run_command()` or `run_command_with_scope()`
- [ ] No direct `scan()` calls in any Phase 5 command
- [ ] No manual `Journal` creation in any Phase 5 command
- [ ] No manual `RepoLock` acquisition in any Phase 5 command
- [ ] All existing tests pass
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo fmt --check` passes
- [ ] Engine hooks fire for all commands (verified by OOB drift harness)

---

## Risk Mitigation

1. **Migrate one command at a time** - Don't batch
2. **Keep legacy code until new code proven** - Feature flag if needed
3. **Run full test suite after each command** - No regressions
4. **Commit after each successful migration** - Easy rollback

---

## References

- ARCHITECTURE.md Section 5-6, 12
- SPEC.md Section 4.6.5 (op-state), Section 8D (mutation commands)
- HANDOFF.md - Current patterns and progress
- `src/engine/command.rs` - Trait definitions
- `src/engine/runner.rs` - Entry points
- `src/engine/plan.rs` - PlanStep variants
- `src/engine/exec.rs` - Executor contract
