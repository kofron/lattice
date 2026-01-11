# Lattice v2 Roadmap

This roadmap covers v2 features for Lattice, building on the foundation established in v1.

## Overview

v2 focuses on enhanced authentication, additional forge support, and advanced repository workflows.

---

## Milestones

### Milestone 1: GitHub App OAuth Authentication

**Status:** Not started

**Goal:** Replace PAT-based authentication with GitHub App device flow OAuth as the only authentication method.

**Key deliverables:**

- Device Authorization Grant flow implementation
- Token refresh with single-use rotating refresh tokens
- Auth-scoped file locking for concurrent refresh safety
- `lattice auth login|status|logout` commands
- Installation discovery and repository authorization checks
- TokenProvider trait for host adapters

**Details:** See [milestone-1-github-app-oauth/PLAN.md](milestones/milestone-1-github-app-oauth/PLAN.md)

---

### Milestone 2: Bare Repository Support

**Status:** Not started

**Goal:** Enable Lattice to run safely in bare repositories and linked worktrees.

**Key deliverables:**

- `RepoContext` enum (Normal/Bare/Worktree) and `common_dir` path handling
- `WorkingDirectoryAvailable` capability and command gating
- Repo-scoped lock, op-state, and journals (shared across worktrees)
- Worktree occupancy detection ("checked out elsewhere" blocker)
- `--no-restack` for submit/sync in bare repos (ancestry-based alignment)
- `--no-checkout` for get in bare repos (still tracks with metadata)
- Continue/abort worktree ownership enforcement

**Details:** See [milestone-2-bare-repo-support/PLAN.md](milestones/milestone-2-bare-repo-support/PLAN.md)

---

## Dependencies

v2 milestones build on v1 infrastructure:

- v1 Milestones 0-8: Core CLI, stack operations, GitHub integration
- v1 Milestones 9-10: PR workflow commands

## Conventions

- Each milestone has a `PLAN.md` with phases and acceptance gates
- Implementation notes are recorded in `implementation_notes.md` after completion
- All changes must pass `cargo test`, `cargo clippy`, and type checks
