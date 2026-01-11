//! core
//!
//! Core domain types, schemas, and operations for Lattice.
//!
//! # Modules
//!
//! - [`types`] - Strong types: BranchName, Oid, RefName, etc.
//! - [`graph`] - Stack graph representation and operations
//! - [`verify`] - Fast verification of repository invariants
//! - [`naming`] - Branch naming rules and validation
//! - [`ops`] - Operation journaling and locking
//! - [`metadata`] - Branch metadata schema and storage
//! - [`config`] - Configuration schema and loading
//! - [`paths`] - Centralized path routing for Lattice storage
//!
//! # Design Principles
//!
//! - Strong typing prevents invalid states at compile time
//! - Schemas are strict and self-describing
//! - All verification is deterministic

pub mod config;
pub mod graph;
pub mod metadata;
pub mod naming;
pub mod ops;
pub mod paths;
pub mod types;
pub mod verify;
