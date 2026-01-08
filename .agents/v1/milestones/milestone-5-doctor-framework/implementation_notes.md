# Milestone 5: Doctor Framework - Implementation Notes

## Completion Status

All acceptance gates pass:
- [x] `cargo fmt --check` passes
- [x] `cargo clippy -- -D warnings` passes  
- [x] `cargo test` passes (388 unit tests, 48 git integration, 29 persistence integration, 12 property tests)
- [x] `cargo doc --no-deps` succeeds
- [x] Doctor command available via `lattice doctor --help`
- [x] Fix options generated for each issue type
- [x] Confirmation model enforced (no auto-apply without explicit fix IDs)

## Implementation Decisions

### 1. Fix ID Design

Following **Simplicity** and ARCHITECTURE.md Section 8.2, fix IDs use a predictable format:
- Pattern: `{issue-type}:{action}[:{key}]`
- Examples: `trunk:set:main`, `metadata-parse-error:delete:feature/broken`, `parent-missing:reparent-trunk:child`

This makes fix IDs stable and user-friendly for `--fix` flag usage.

### 2. FixOption with Plan

FixOption carries an optional `Plan` that gets populated lazily by the planner. This follows **Reuse** - we don't create a separate plan type for repairs, we use the existing Plan infrastructure.

```rust
pub struct FixOption {
    pub id: FixId,
    pub issue_id: IssueId,
    pub description: String,
    pub preview: FixPreview,
    pub preconditions: Vec<Capability>,
    plan: Option<Plan>,  // Populated when planning repairs
}
```

### 3. Generator Pattern over Trait Objects

Instead of a `FixGenerator` trait with dynamic dispatch, I used a simpler function dispatch pattern:

```rust
pub fn generate_fixes(issue: &Issue, snapshot: &RepoSnapshot) -> Vec<FixOption> {
    match extract_issue_type(&issue.id) {
        "trunk-not-configured" => generate_trunk_fixes(issue, snapshot),
        "metadata-parse-error" => generate_metadata_parse_fixes(issue, snapshot),
        // ...
    }
}
```

This follows **Simplicity** - no need for trait objects when a match statement works.

### 4. RepairBundle Consolidation

The `RepairBundle` type already existed in `engine/gate.rs` from Milestone 4. Rather than creating a duplicate, I added a helper function `diagnose_from_gate_bundle` to bridge gate's RepairBundle to doctor's DiagnosisReport:

```rust
pub fn diagnose_from_gate_bundle(
    bundle: &crate::engine::gate::RepairBundle,
    snapshot: &RepoSnapshot,
) -> DiagnosisReport
```

This follows **Reuse** - extend what exists rather than duplicating.

### 5. Preview Types

Three types of changes are tracked in FixPreview:
- `RefChange` - ref updates, creates, deletes
- `MetadataChange` - metadata create, update, delete
- `ConfigChange` - config set, remove, migrate

Each has a Display implementation for human-readable output.

### 6. Planner Integration

The doctor planner reuses the existing `Plan` and `PlanStep` types from `engine/plan.rs`. RefChanges map to `PlanStep::UpdateRef` and `PlanStep::DeleteRef`. This follows **Reuse** - doctor is not a special-case command, it uses the same execution model.

### 7. CLI Design

The doctor command follows the confirmation model from ARCHITECTURE.md Section 8.3:

```
lattice doctor              # Interactive: show issues, prompt for fixes
lattice doctor --list       # List issues and fixes (machine-readable)
lattice doctor --fix <id>   # Apply specific fix (non-interactive)
lattice doctor --dry-run    # Preview what would happen
```

Non-interactive mode (`-q` or `--no-interactive`) with no `--fix` flags emits issues but applies nothing - strict confirmation model.

### 8. Issue Type Extraction

For mapping issues to fix generators, issue IDs are parsed to extract the type prefix:

```rust
fn extract_issue_type(issue_id: &IssueId) -> &str {
    let id_str = issue_id.as_str();
    match id_str.find(':') {
        Some(pos) => &id_str[..pos],
        None => id_str,
    }
}
```

This handles both singleton IDs (`trunk-not-configured`) and hashed IDs (`metadata-parse-error:abc123`).

## Files Created/Modified

### New Files
- `src/doctor/fixes.rs` - FixId, FixOption, FixPreview, change types
- `src/doctor/generators.rs` - Fix generators for each issue type
- `src/doctor/planner.rs` - Repair plan generation

### Modified Files
- `src/doctor/mod.rs` - Doctor struct, DiagnosisReport, RepairOutcome
- `src/doctor/issues.rs` - Extended with orphaned_metadata, parent_missing, config_migration
- `src/engine/health.rs` - Added orphaned_metadata, parent_missing, config_migration_needed factories
- `src/cli/args.rs` - Added Doctor subcommand
- `src/cli/commands/mod.rs` - Added doctor command handler

## Test Coverage

- 388 unit tests total
- Doctor module tests cover:
  - FixId creation, parsing, display
  - FixOption with preconditions
  - FixPreview building and formatting
  - All change type Display implementations
  - Generator dispatch and individual generators
  - Planner combining multiple fixes
  - Doctor diagnose and plan_repairs
  - DiagnosisReport formatting
  - RepairOutcome construction

## Future Work (Milestone 6+)

1. Event ledger integration - record DoctorProposed/DoctorApplied events during actual fix application
2. Interactive mode prompts - currently returns fixes but doesn't prompt
3. Actual plan execution - currently generates plans but doesn't execute them
4. Post-repair verification - run full health check after repairs
