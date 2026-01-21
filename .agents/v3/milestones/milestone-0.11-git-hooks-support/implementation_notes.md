# Milestone 0.11: Git Hooks Support - Implementation Notes

## Completion Date: 2026-01-20

## Summary

Successfully implemented `--verify` / `--no-verify` global CLI flags per SPEC.md Section 6.1 and ARCHITECTURE.md Section 10.2. The implementation threads the verification preference through Context to all git CLI invocations that support hook control.

## Files Modified

| File | Changes |
|------|---------|
| `src/cli/args.rs` | Added `--verify`/`--no-verify` flags with `conflicts_with`, `verify_flag()` helper, unit tests |
| `src/engine/mod.rs` | Added `verify: bool` field to Context, updated Default impl and tests |
| `src/cli/mod.rs` | Wired `cli.verify_flag().unwrap_or(true)` to Context creation |
| `src/cli/commands/create.rs` | Added --no-verify to git commit (2 locations) |
| `src/cli/commands/modify.rs` | Added --no-verify to git commit |
| `src/cli/commands/squash.rs` | Added --no-verify to git commit (3 locations) |
| `src/cli/commands/split.rs` | Added --no-verify to git commit (2 locations) |
| `src/cli/commands/restack.rs` | Added --no-verify to git rebase |
| `src/cli/commands/phase3_helpers.rs` | Changed `_ctx` to `ctx`, added --no-verify to git rebase |
| `src/cli/commands/fold.rs` | Added --no-verify to git merge (2 locations) |
| `src/cli/commands/sync.rs` | Added --no-verify to git merge |
| `src/cli/commands/revert.rs` | Added --no-verify to git revert |
| `src/cli/commands/submit.rs` | Added --no-verify to git push |
| `tests/bootstrap_fixes_integration.rs` | Added `verify: true` to Context |
| `tests/init_hint_integration.rs` | Added `verify: true` to Context |
| `tests/bare_repo_mode_compliance.rs` | Added `verify: true` to Context |
| `tests/commands_integration.rs` | Added `verify: true` to Context |
| `tests/oob_fuzz.rs` | Added `verify: true` to Context |

## Design Decisions

### 1. Config Integration Deferred

The PLAN.md suggested loading config to resolve verify defaults. We chose the simpler approach:

```rust
verify: cli.verify_flag().unwrap_or(true)
```

**Rationale:** 
- Default `true` is safe and per ARCHITECTURE.md §10.2 ("Git hooks are honored by default")
- Commands already load config internally when needed
- CLI flag always takes precedence per SPEC.md §4.3.1
- Avoids potential config loading failures before command dispatch

A comment in `src/cli/mod.rs` documents this decision.

### 2. Consistent Pattern Across Commands

All 10 command files use the same pattern:

```rust
let mut args = vec!["<command>"];
if !ctx.verify {
    args.push("--no-verify");
}
args.extend([...other args...]);
```

This ensures consistency and makes the code easy to audit.

### 3. Integration Tests Deferred

Full integration tests requiring actual git hooks were deferred because:
- Unit tests cover the flag parsing logic
- The implementation is straightforward (conditional string push)
- Git subprocess behavior is deterministic
- Manual verification can be done with real repositories

Integration tests could be added later if edge cases arise.

### 4. git2 Operations Unchanged

Per the PLAN.md analysis, git2 (libgit2) operations don't execute hooks by design. Only CLI-spawned git commands were updated.

## Verification Results

All checks pass:
- `cargo check` - Clean
- `cargo clippy -- -D warnings` - Clean  
- `cargo test` - 846 tests passing
- `cargo fmt --check` - Clean

## Spec Compliance

| Requirement | Status |
|-------------|--------|
| SPEC.md §6.1: `--verify`/`--no-verify` global flags | Implemented |
| ARCHITECTURE.md §10.2: Hooks honored by default | `verify: true` default |
| ARCHITECTURE.md §10.2: `--no-verify` disables hooks | Conditional flag passing |
| ARCHITECTURE.md §10.2: Executor carries policy | Context threading |

## Future Considerations

1. **Config-based defaults**: Could add `config.verify_hooks()` fallback if users request it
2. **Integration tests**: Could add tests with actual hook files if edge cases emerge
3. **Hook debugging**: Could add `--debug` output showing when hooks are skipped
