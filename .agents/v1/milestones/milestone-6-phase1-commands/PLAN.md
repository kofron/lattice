# Milestone 6: Phase 1 Commands - Implementation Plan

## Summary

Implement the core local stack engine commands as specified in ROADMAP.md Section 6. These commands follow the validated execution model (Scan -> Gate -> Plan -> Execute -> Verify) and use the infrastructure built in Milestones 0-5.

**Core principle from ARCHITECTURE.md:** All commands flow through the engine lifecycle. Mutations only occur via the Executor with CAS semantics and journaling.

---

## Architecture Conformance

Per ARCHITECTURE.md Section 12 (Command Lifecycle):

1. **SCAN** - Compute repo health report, detect in-progress ops
2. **GATE** - Evaluate command requirement set, produce ReadyContext or RepairBundle
3. **REPAIR** (if gated) - Hand off to Doctor
4. **PLAN** - Planner produces plan from validated context
5. **EXECUTE** - Executor applies plan with CAS
6. **VERIFY** - Post-scan invariant validation
7. **RETURN** - Produce output and exit

---

## Command Catalog

### Phase A: Read-Only Commands

| Command | Requirements | Description |
|---------|-------------|-------------|
| `log` | RepoOpen | Display tracked branches in stack layout |
| `info [branch]` | RepoOpen | Show tracking status, parent, freeze state |
| `parent` | RepoOpen | Print parent branch name |
| `children` | RepoOpen | Print child branch names |
| `trunk` (print) | RepoOpen | Display configured trunk |

### Phase B: Setup Commands

| Command | Requirements | Description |
|---------|-------------|-------------|
| `init [--trunk] [--reset]` | RepoOpen | Initialize repo config |
| `config get/set/list` | RepoOpen | Configuration management |
| `completion` | None | Shell completion scripts |
| `changelog` | None | Version info |

### Phase C: Tracking Commands

| Command | Requirements | Description |
|---------|-------------|-------------|
| `track [branch]` | RepoOpen, GraphValid | Start tracking a branch |
| `untrack [branch]` | RepoOpen | Stop tracking a branch |
| `freeze [branch]` | RepoOpen, MetadataReadable | Mark branch frozen |
| `unfreeze [branch]` | RepoOpen, MetadataReadable | Unmark branch frozen |

### Phase D: Navigation Commands

| Command | Requirements | Description |
|---------|-------------|-------------|
| `checkout [branch]` | RepoOpen | Check out a branch |
| `up [steps]` | RepoOpen, GraphValid | Move to child branch |
| `down [steps]` | RepoOpen, GraphValid | Move to parent branch |
| `top` | RepoOpen, GraphValid | Move to stack leaf |
| `bottom` | RepoOpen, GraphValid | Move to stack root |

### Phase E: Core Mutating Commands

| Command | Requirements | Description |
|---------|-------------|-------------|
| `restack` | NoOpInProgress, GraphValid, FrozenPolicySatisfied | Rebase tracked branches |
| `continue` | LatticeOpInProgress | Resume paused operation |
| `abort` | LatticeOpInProgress | Cancel paused operation |
| `undo` | NoOpInProgress | Rollback last operation |
| `create [name]` | NoOpInProgress, GraphValid | Create tracked branch |

---

## Implementation Steps

### Step 1: Command Module Organization

Create file structure:
```
src/cli/commands/
  mod.rs          (update dispatch)
  log.rs
  info.rs
  relationships.rs  (parent, children)
  trunk.rs
  init.rs
  config_cmd.rs
  completion.rs
  changelog.rs
  track.rs
  untrack.rs
  freeze.rs
  checkout.rs
  navigation.rs   (up, down, top, bottom)
  restack.rs
  recovery.rs     (continue, abort)
  undo.rs
  create.rs
```

### Step 2: Phase A - Read-Only Commands

**log:**
- Add `Log` to Command enum with flags: `--short`, `--long`, `--stack`, `--all`, `--reverse`
- Scan repository, display graph using ASCII art
- Format: branch name, commit count, PR status indicator

**info:**
- Add `Info` to Command enum with optional `branch` arg
- Display: name, parent, base OID, freeze state, PR linkage
- Support `--diff`, `--stat`, `--patch` via git

**parent/children:**
- Simple queries returning branch names
- Exit 0 with no output if none

**trunk:**
- Print configured trunk name
- Error if not configured

### Step 3: Phase B - Setup Commands

**init:**
- Create `.git/lattice/config.toml` with trunk
- `--trunk <branch>`: specify trunk
- `--reset`: clear all `refs/branch-metadata/*`
- Interactive: prompt for trunk selection

**config:**
- Subcommands: `get <key>`, `set <key> <value>`, `list`
- Validate values on set
- Show merged global + repo config on list

**completion:**
- Use `clap_complete` crate
- Support bash, zsh, fish, powershell

**changelog:**
- Print version from Cargo.toml
- Include brief release notes

### Step 4: Phase C - Tracking Commands

**track:**
- Flags: `--parent <branch>`, `--force`, `--as-frozen`
- Compute base = parent tip at track time
- Create metadata via WriteMetadataCas
- `--force`: auto-select nearest tracked ancestor

**untrack:**
- Flags: `--force`
- Check for descendants, require confirmation
- Delete metadata refs for branch and descendants

**freeze/unfreeze:**
- Update metadata freeze state
- Default: apply to downstack ancestors
- `--only`: just this branch

### Step 5: Phase D - Navigation Commands

**checkout:**
- Flags: `--trunk`, `--stack`
- Shell out to `git checkout`
- `--stack`: filter selector to current stack

**up/down:**
- Optional `steps` argument (default 1)
- Follow graph edges
- Multi-child: prompt in interactive, error in non-interactive

**top/bottom:**
- Navigate to extremes
- Multi-path: prompt/error

### Step 6: Graph Traversal Helpers

Add to `src/core/graph.rs`:

```rust
/// Get all ancestors up to trunk
pub fn ancestors(&self, branch: &BranchName) -> Vec<BranchName>;

/// Get all descendants recursively  
pub fn descendants(&self, branch: &BranchName) -> Vec<BranchName>;

/// Topological sort for bottom-up traversal
pub fn topological_order(&self, branches: &[BranchName]) -> Vec<BranchName>;
```

### Step 7: restack Command

**Planner:**
1. Determine scope (target + direction)
2. Get branches in topological order (bottom-up)
3. For each branch:
   - Check if `base == parent.tip` (skip if aligned)
   - Check freeze state (skip if frozen, warn)
   - Generate: Checkpoint, RunGit rebase, PotentialConflictPause, WriteMetadataCas

**Executor handling:**
- RunGit shells out to `git rebase --onto parent.tip base branch`
- Check `git.state()` after each rebase
- If conflict: write op-state as Paused, record in journal, return Paused

### Step 8: continue/abort Commands

**continue:**
- Read OpState from `.git/lattice/op-state.json`
- Validate phase is Paused
- Read corresponding Journal
- `--all`: run `git add -A`
- Run `git rebase --continue`
- Resume remaining plan steps
- On success: mark committed, clear op-state

**abort:**
- Read OpState and Journal
- Run `git rebase --abort`
- Rollback ref changes from journal
- Mark rolled_back, clear op-state

### Step 9: undo Command

- Find most recent committed Journal
- Validate undoable (local refs only)
- Generate rollback plan from journal ref updates
- Execute via Executor

### Step 10: create Command

**Flags:**
- `-m, --message <msg>`
- `-a, --all` (stage all)
- `-u, --update` (stage modified)
- `-p, --patch` (interactive add)
- `-i, --insert` (insert between current and child)

**Planner:**
1. Determine branch name (arg, message slug, or prompt)
2. Check for staged changes
3. Generate: RunGit checkout -b + commit (if changes), WriteMetadataCas
4. Insert mode: re-parent child with additional WriteMetadataCas

### Step 11: Integration Tests

- Each command with real git repos
- Conflict scenarios for restack
- continue/abort recovery
- Undo correctness
- Freeze enforcement
- Snapshot tests for log/info output

---

## File Modifications Summary

| File | Changes |
|------|---------|
| `src/cli/args.rs` | Add 18 new Command variants with flags |
| `src/cli/commands/mod.rs` | Update dispatch, add module declarations |
| `src/cli/commands/*.rs` | 15 new command files |
| `src/core/graph.rs` | Add ancestors, descendants, topological_order |
| `src/engine/plan.rs` | Add planner functions for each command |

---

## Acceptance Gates

- [ ] `cargo fmt --check` passes
- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo test` passes
- [ ] `cargo doc --no-deps` succeeds
- [ ] All Phase 1 commands have integration tests
- [ ] Fault injection tests for executor step boundaries
- [ ] Conflict pausing works (restack pause + continue/abort)
- [ ] Freeze enforcement blocks rewriting commands

---

## Notes

- **Follow the leader**: SPEC.md and ARCHITECTURE.md are authoritative
- **Simplicity**: Implement exactly what's needed, no more
- **No stubs**: All commands fully functional
- **Purity**: Business logic in pure planner functions
- **Tests are everything**: Comprehensive test coverage required
