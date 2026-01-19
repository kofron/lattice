# Milestone 5.8: Synthetic Stack Detection - Implementation Notes

## Summary

Implemented two-tiered synthetic stack detection as specified in the ROADMAP.md. The feature identifies open PRs targeting trunk that may have accumulated commits from merged sub-PRs (synthetic remote stacks).

## Implementation Approach

### Two-Tiered Design

Following the principle of **Simplicity**, the implementation uses a two-tiered approach to balance API cost vs. information:

**Tier 1 (Default - Cheap):**
- Uses existing `list_open_prs` capability (no additional API calls)
- Identifies trunk-bound PRs as "potential synthetic stack heads"
- Emits `Severity::Info` issue for each detected head
- Runs automatically during `lt doctor`

**Tier 2 (Explicit - Deep Remote):**
- Enabled via `--deep-remote` flag or `doctor.bootstrap.deep_remote` config
- Queries closed PRs targeting each synthetic head branch
- Enforces configurable budgets:
  - `max_synthetic_heads`: 3 (default)
  - `max_closed_prs_per_head`: 20 (default)
- Adds `Evidence::SyntheticStackChildren` to issues when children found

### Key Design Decisions

1. **Informational Only**: Per ROADMAP.md, synthetic stacks are not automatically repairable. The commits have already been squash-merged or rebased, so reconstruction isn't possible. Issues are `Severity::Info` to communicate context without blocking operations.

2. **Budget Enforcement**: Deep analysis respects configurable budgets and reports truncation explicitly. This prevents runaway API calls on repositories with many potential synthetic heads.

3. **Detection in Scanner, Analysis in Generator**: Following **Purity** principle:
   - Tier 1 detection (`detect_potential_synthetic_heads`) is a pure function of snapshot + open PRs
   - Tier 2 analysis (`analyze_synthetic_stack_deep`) is async but side-effect free, called only when explicitly requested

4. **Reuse of Existing Infrastructure**: Following **Reuse** principle:
   - Extends `Forge` trait with `list_closed_prs_targeting` method
   - Uses existing `ListPullsResult` return type
   - Integrates with existing `MockForge` for testing
   - Uses existing `Evidence` enum pattern

## Files Modified

| File | Changes |
|------|---------|
| `src/doctor/issues.rs` | Added `PotentialSyntheticStackHead` variant to `KnownIssue` |
| `src/engine/health.rs` | Added `ClosedPrInfo`, `Evidence::PrReference`, `Evidence::SyntheticStackChildren`, and `potential_synthetic_stack_head()` constructor |
| `src/core/config/schema.rs` | Added `DoctorConfig` and `DoctorBootstrapConfig` |
| `src/cli/args.rs` | Added `--deep-remote` flag to doctor command |
| `src/forge/traits.rs` | Added `ListClosedPrsOpts` and `list_closed_prs_targeting` method |
| `src/forge/github.rs` | Implemented `list_closed_prs_targeting` with pagination |
| `src/forge/mock.rs` | Implemented `list_closed_prs_targeting` for MockForge |
| `src/engine/scan.rs` | Added `detect_potential_synthetic_heads()` function |
| `src/doctor/generators.rs` | Added `analyze_synthetic_stack_deep()` function |
| `src/cli/commands/mod.rs` | Wired deep analysis into doctor command |

## New Functions

1. `detect_potential_synthetic_heads(snapshot, open_prs)` - Tier 1 detection
2. `analyze_synthetic_stack_deep(issue, forge, config)` - Tier 2 deep analysis
3. `perform_deep_synthetic_analysis(ctx, git, diagnosis)` - Doctor command integration
4. `create_forge_for_deep_analysis(git)` - Helper to create forge for analysis

## New Types

1. `KnownIssue::PotentialSyntheticStackHead` - Issue variant for synthetic heads
2. `ClosedPrInfo` - Information about a closed PR child
3. `Evidence::PrReference` - PR reference evidence
4. `Evidence::SyntheticStackChildren` - Tier 2 evidence with closed PR children
5. `DoctorConfig` - Doctor command configuration
6. `DoctorBootstrapConfig` - Bootstrap analysis configuration
7. `ListClosedPrsOpts` - Options for closed PR queries

## Tests Added

### Unit Tests

- `potential_synthetic_stack_head_issue_id` - Issue ID generation
- `potential_synthetic_stack_head_to_issue` - Issue conversion
- `potential_synthetic_stack_head_not_blocking` - Severity check
- `detects_trunk_targeting_pr_as_potential_head` - Tier 1 detection
- `ignores_non_trunk_targeting_prs` - Filtering
- `returns_empty_when_no_trunk` - Edge case
- `detects_multiple_potential_heads` - Multiple detection
- `returns_empty_for_empty_prs` - Empty input handling
- `filters_by_base` - MockForge closed PR filtering
- `respects_limit` - Budget enforcement
- `empty_when_no_closed_prs` - Empty result handling
- `not_truncated_when_under_limit` - Truncation flag

### Config Tests

- `defaults` - Default configuration values
- `doctor_config_defaults` - DoctorConfig defaults
- `roundtrip` - Serialization/deserialization
- `global_config_with_doctor` - Integration with GlobalConfig

## Verification

All acceptance criteria from ROADMAP.md Milestone 5.8 are satisfied:

- [x] Tier 1: Trunk-bound open PRs flagged as potential synthetic heads
- [x] Tier 2: `--deep-remote` enables closed PR enumeration
- [x] Budgets enforced with explicit truncation reporting
- [x] Config options `doctor.bootstrap.*` respected
- [x] API failures do not block other diagnosis
- [x] `cargo test` passes
- [x] `cargo clippy` passes

## Usage

```bash
# Tier 1 (default) - detect potential synthetic heads
lt doctor

# Tier 2 (explicit) - deep analysis with closed PR enumeration
lt doctor --deep-remote

# Configure via config file
# ~/.lattice/config.toml
[doctor.bootstrap]
deep_remote = true
max_synthetic_heads = 5
max_closed_prs_per_head = 30
```
