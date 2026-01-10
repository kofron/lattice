//! Lattice - A Rust-native CLI for stacked branches and PRs
//!
//! Lattice is a single-binary tool that mirrors Graphite CLI semantics for stacked
//! development: creating, navigating, restacking, submitting, syncing, and merging
//! stacked branches and pull requests.
//!
//! # Architecture
//!
//! The codebase follows a strict layered architecture:
//!
//! - [`cli`] - Command-line interface layer (parses args, delegates to engine)
//! - [`engine`] - Orchestrates Scan → Gate → Plan → Execute → Verify lifecycle
//! - [`core`] - Domain types, schemas, verification, and operations
//! - [`git`] - Single interface for all Git operations
//! - [`forge`] - Abstraction for remote forges (GitHub v1)
//! - [`auth`] - GitHub App OAuth authentication
//! - [`secrets`] - Secret storage abstraction
//! - [`doctor`] - Explicit repair framework
//! - [`ui`] - User interaction utilities
//!
//! # Correctness Invariants
//!
//! Lattice maintains the following invariants:
//!
//! 1. Commands execute only against validated execution models
//! 2. All mutations flow through a single transactional executor
//! 3. Repository state is never silently corrupted
//! 4. Repairs are explicit and require user confirmation

pub mod auth;
pub mod cli;
pub mod core;
pub mod doctor;
pub mod engine;
pub mod forge;
pub mod git;
pub mod secrets;
pub mod ui;
