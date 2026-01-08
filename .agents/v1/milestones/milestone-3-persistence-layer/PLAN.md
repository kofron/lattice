# Milestone 3: Persistence Layer

## Status: COMPLETE

---

## Overview

**Goal:** Implement the persistence layer for Lattice - the metadata store, secret store, and repository lock. These components provide the stable foundation for all future operations by ensuring atomic, crash-safe storage of branch metadata and configuration.

**Principles:** Follow the leader (SPEC.md + ARCHITECTURE.md), Simplicity, Purity, No stubs, Tests are everything.

**Dependencies:** 
- Milestone 0 (complete) - Crate structure and basic types
- Milestone 1 (complete) - Core domain types with full validation
- Milestone 2 (complete) - Git interface with CAS operations

---

## Architecture Context

Per ROADMAP.md Section 3 and SPEC.md Section 4:

1. **Metadata Store**: Branch metadata lives in Git refs under `refs/branch-metadata/<branch>`. Each ref points to a blob containing JSON. All updates use CAS (compare-and-swap) semantics.

2. **Secret Store**: The `SecretStore` trait abstracts secret storage. V1 requires `FileSecretStore` with `~/.lattice/secrets.toml` and optional `KeychainSecretStore` behind feature flag.

3. **Repo Lock**: Exclusive lock at `.git/lattice/lock` prevents concurrent mutations. Uses OS file locking (`fs2` crate).

The Git interface from Milestone 2 provides the primitives (`write_blob`, `read_blob`, `update_ref_cas`, `delete_ref_cas`) that the metadata store will use.

---

## Acceptance Gates

### Functional Gates
- [x] `MetadataStore::read()` parses JSON from blob correctly
- [x] `MetadataStore::write_cas()` creates blob and updates ref atomically
- [x] `MetadataStore::write_cas()` fails with `CasFailed` when precondition violated
- [x] `MetadataStore::delete_cas()` removes ref with CAS semantics
- [x] `MetadataStore::list()` returns all tracked branches
- [x] `FileSecretStore` writes to `~/.lattice/secrets.toml`
- [x] `FileSecretStore` enforces 0600 permissions on Unix
- [x] `FileSecretStore` never prints secrets in any output
- [x] `KeychainSecretStore` works behind `keychain` feature flag
- [x] `RepoLock::acquire()` uses OS file locking via `fs2`
- [x] `RepoLock` prevents concurrent Lattice operations (integration test)
- [x] Lock contention returns proper error code

### Quality Gates
- [x] `cargo fmt --check` passes
- [x] `cargo clippy -- -D warnings` passes
- [x] `cargo test` passes (target: 200+ tests including new persistence tests)
- [x] `cargo doc --no-deps` succeeds
- [x] All public functions have doctests
- [x] Integration tests use real git repositories

### Architectural Gates
- [x] Metadata store uses Git interface exclusively (no direct git2 imports)
- [x] All metadata operations use CAS semantics
- [x] Secret values never appear in logs, errors, or debug output
- [x] Lock acquisition is RAII-based (automatic release on drop)

---

## Implementation Steps

### Step 1: Add Dependencies

Add required dependencies to `Cargo.toml`:

```toml
[dependencies]
fs2 = "0.4"           # OS file locking for RepoLock
keyring = { version = "3", optional = true }  # Keychain access

[features]
keychain = ["dep:keyring"]
```

**Files:** `Cargo.toml`

---

### Step 2: Implement RepoLock with fs2

Transform the stub `RepoLock` into a real implementation using OS file locking.

**File:** `src/core/ops/lock.rs`

**Key Implementation:**

```rust
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

pub struct RepoLock {
    path: PathBuf,
    file: Option<File>,
}

impl RepoLock {
    /// Attempt to acquire exclusive lock on the repository.
    ///
    /// Returns `LockError::AlreadyLocked` if another process holds the lock.
    pub fn acquire(git_dir: &Path) -> Result<Self, LockError> {
        let lattice_dir = git_dir.join("lattice");
        std::fs::create_dir_all(&lattice_dir)?;
        
        let path = lattice_dir.join("lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&path)?;
        
        // Try non-blocking exclusive lock
        file.try_lock_exclusive()
            .map_err(|_| LockError::AlreadyLocked)?;
        
        Ok(Self { path, file: Some(file) })
    }
    
    /// Try to acquire lock, returning None if already held.
    pub fn try_acquire(git_dir: &Path) -> Result<Option<Self>, LockError> {
        match Self::acquire(git_dir) {
            Ok(lock) => Ok(Some(lock)),
            Err(LockError::AlreadyLocked) => Ok(None),
            Err(e) => Err(e),
        }
    }
    
    /// Release the lock explicitly.
    pub fn release(&mut self) -> Result<(), LockError> {
        if let Some(file) = self.file.take() {
            file.unlock()?;
        }
        Ok(())
    }
}

impl Drop for RepoLock {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            let _ = file.unlock();
        }
    }
}
```

**Tests Required:**
- `lock_acquire_succeeds` - Basic lock acquisition works
- `lock_prevents_second_acquire` - Second acquire fails with `AlreadyLocked`
- `lock_released_on_drop` - Lock can be reacquired after drop
- `lock_released_explicitly` - `release()` method works
- `try_acquire_returns_none_when_locked` - Non-blocking variant works
- `lock_creates_lattice_directory` - Directory created if missing

**Integration Test:**
- Spawn two processes, second must fail with lock contention

---

### Step 3: Implement MetadataStore (Full Implementation)

Transform the stub `MetadataStore` to use the Git interface for real storage.

**File:** `src/core/metadata/store.rs`

**Key Implementation:**

```rust
use crate::core::metadata::schema::{parse_metadata, BranchMetadataV1};
use crate::core::types::{BranchName, Oid, RefName};
use crate::git::{Git, GitError};

/// Result of reading metadata - includes ref OID for CAS.
#[derive(Debug, Clone)]
pub struct MetadataEntry {
    /// The metadata ref's current OID (blob pointer) - used for CAS
    pub ref_oid: Oid,
    /// The parsed metadata
    pub metadata: BranchMetadataV1,
}

/// Metadata store backed by Git refs.
pub struct MetadataStore<'a> {
    git: &'a Git,
}

impl<'a> MetadataStore<'a> {
    pub fn new(git: &'a Git) -> Self {
        Self { git }
    }
    
    /// Get the ref name for a branch's metadata.
    pub fn ref_name(branch: &BranchName) -> RefName {
        RefName::for_metadata(branch.as_str())
            .expect("branch name should be valid for metadata ref")
    }
    
    /// Read metadata for a branch.
    pub fn read(&self, branch: &BranchName) -> Result<Option<MetadataEntry>, StoreError> {
        let refname = Self::ref_name(branch);
        
        // Try to resolve the metadata ref
        let ref_oid = match self.git.try_resolve_ref(refname.as_str())? {
            Some(oid) => oid,
            None => return Ok(None),
        };
        
        // Read the blob content
        let json = self.git.read_blob_as_string(&ref_oid)?;
        
        // Parse with strict validation
        let metadata = parse_metadata(&json)?;
        
        Ok(Some(MetadataEntry { ref_oid, metadata }))
    }
    
    /// Write metadata with CAS semantics.
    ///
    /// - `expected_old`: Expected current ref OID, or None for creation
    /// - Returns the new ref OID on success
    pub fn write_cas(
        &self,
        branch: &BranchName,
        expected_old: Option<&Oid>,
        metadata: &BranchMetadataV1,
    ) -> Result<Oid, StoreError> {
        let refname = Self::ref_name(branch);
        
        // Serialize to canonical JSON
        let json = metadata.to_canonical_json()?;
        
        // Write blob
        let blob_oid = self.git.write_blob(json.as_bytes())?;
        
        // Update ref with CAS
        self.git.update_ref_cas(
            refname.as_str(),
            &blob_oid,
            expected_old,
            &format!("lattice: update metadata for {}", branch),
        )?;
        
        Ok(blob_oid)
    }
    
    /// Delete metadata with CAS semantics.
    pub fn delete_cas(&self, branch: &BranchName, expected_old: &Oid) -> Result<(), StoreError> {
        let refname = Self::ref_name(branch);
        self.git.delete_ref_cas(refname.as_str(), expected_old)?;
        Ok(())
    }
    
    /// List all branches with metadata.
    pub fn list(&self) -> Result<Vec<BranchName>, StoreError> {
        let refs = self.git.list_metadata_refs()?;
        Ok(refs.into_iter().map(|(name, _)| name).collect())
    }
    
    /// List all metadata entries with their ref OIDs.
    pub fn list_all(&self) -> Result<Vec<(BranchName, Oid)>, StoreError> {
        Ok(self.git.list_metadata_refs()?)
    }
}
```

**Error Mapping:**

```rust
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("metadata not found for branch: {0}")]
    NotFound(String),
    
    #[error("CAS precondition failed: expected {expected}, found {actual}")]
    CasFailed { expected: String, actual: String },
    
    #[error("failed to parse metadata: {0}")]
    ParseError(String),
    
    #[error("failed to serialize metadata: {0}")]
    SerializeError(String),
    
    #[error("git error: {0}")]
    GitError(#[from] GitError),
}
```

**Tests Required:**
- `read_nonexistent_returns_none` - Missing metadata returns None
- `write_cas_create` - Create new metadata ref
- `write_cas_update` - Update existing with correct expected_old
- `write_cas_fails_on_mismatch` - CAS failure on wrong expected_old
- `write_cas_fails_on_unexpected_existence` - CAS failure creating when exists
- `delete_cas_success` - Delete with correct expected_old
- `delete_cas_fails_on_mismatch` - CAS failure on wrong expected_old
- `list_empty` - Empty list when no metadata
- `list_multiple` - Lists all tracked branches
- `roundtrip_metadata` - Write then read returns identical metadata
- `strict_parsing` - Unknown fields rejected

---

### Step 4: Implement FileSecretStore

Transform the stub `FileSecretStore` into a real implementation.

**File:** `src/secrets/traits.rs` (split into `src/secrets/file_store.rs`)

**Key Implementation:**

```rust
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// File-based secret storage.
///
/// Stores secrets in `~/.lattice/secrets.toml` with 0600 permissions.
/// Secrets are never printed, logged, or included in error messages.
pub struct FileSecretStore {
    path: PathBuf,
}

impl FileSecretStore {
    /// Create a new file secret store.
    ///
    /// Uses the default path `~/.lattice/secrets.toml`.
    pub fn new() -> Result<Self, SecretError> {
        let home = dirs::home_dir()
            .ok_or_else(|| SecretError::ReadError("cannot determine home directory".into()))?;
        let path = home.join(".lattice").join("secrets.toml");
        Ok(Self { path })
    }
    
    /// Create with a custom path (for testing).
    pub fn with_path(path: PathBuf) -> Self {
        Self { path }
    }
    
    /// Read secrets from file.
    fn read_secrets(&self) -> Result<HashMap<String, String>, SecretError> {
        if !self.path.exists() {
            return Ok(HashMap::new());
        }
        
        let content = fs::read_to_string(&self.path)
            .map_err(|e| SecretError::ReadError(e.to_string()))?;
        
        let secrets: HashMap<String, String> = toml::from_str(&content)
            .map_err(|e| SecretError::ReadError(e.to_string()))?;
        
        Ok(secrets)
    }
    
    /// Write secrets to file with proper permissions.
    fn write_secrets(&self, secrets: &HashMap<String, String>) -> Result<(), SecretError> {
        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| SecretError::WriteError(e.to_string()))?;
        }
        
        let content = toml::to_string_pretty(secrets)
            .map_err(|e| SecretError::WriteError(e.to_string()))?;
        
        // Write to temp file and rename for atomicity
        let temp_path = self.path.with_extension("tmp");
        
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&temp_path)
                .map_err(|e| SecretError::WriteError(e.to_string()))?;
            
            // Set restrictive permissions BEFORE writing content
            #[cfg(unix)]
            {
                let permissions = fs::Permissions::from_mode(0o600);
                file.set_permissions(permissions)
                    .map_err(|e| SecretError::WriteError(e.to_string()))?;
            }
            
            file.write_all(content.as_bytes())
                .map_err(|e| SecretError::WriteError(e.to_string()))?;
            
            file.sync_all()
                .map_err(|e| SecretError::WriteError(e.to_string()))?;
        }
        
        // Atomic rename
        fs::rename(&temp_path, &self.path)
            .map_err(|e| SecretError::WriteError(e.to_string()))?;
        
        Ok(())
    }
}

impl SecretStore for FileSecretStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        let secrets = self.read_secrets()?;
        Ok(secrets.get(key).cloned())
    }
    
    fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        let mut secrets = self.read_secrets()?;
        secrets.insert(key.to_string(), value.to_string());
        self.write_secrets(&secrets)
    }
    
    fn delete(&self, key: &str) -> Result<(), SecretError> {
        let mut secrets = self.read_secrets()?;
        secrets.remove(key);
        self.write_secrets(&secrets)
    }
}
```

**Tests Required:**
- `get_nonexistent_returns_none` - Missing key returns None
- `set_and_get` - Store and retrieve a secret
- `set_overwrites` - Overwriting existing key works
- `delete_existing` - Delete removes the key
- `delete_nonexistent_ok` - Delete missing key doesn't error
- `permissions_0600_on_unix` - File has restrictive permissions (Unix only)
- `atomic_write` - Interrupted write doesn't corrupt
- `secret_not_in_error_message` - Error messages don't contain secrets
- `roundtrip_multiple_secrets` - Multiple secrets work correctly
- `handles_missing_directory` - Creates .lattice directory if needed

---

### Step 5: Implement KeychainSecretStore

Implement the keychain-based secret store behind the `keychain` feature flag.

**File:** `src/secrets/keychain_store.rs`

**Key Implementation:**

```rust
#[cfg(feature = "keychain")]
use keyring::Entry;

/// Keychain-based secret storage.
///
/// Uses the OS keychain (macOS Keychain, Windows Credential Manager,
/// Linux Secret Service) via the `keyring` crate.
#[cfg(feature = "keychain")]
pub struct KeychainSecretStore {
    service: String,
}

#[cfg(feature = "keychain")]
impl KeychainSecretStore {
    /// Create a new keychain secret store.
    pub fn new() -> Result<Self, SecretError> {
        Ok(Self {
            service: "lattice".to_string(),
        })
    }
    
    /// Create an entry for the given key.
    fn entry(&self, key: &str) -> Result<Entry, SecretError> {
        Entry::new(&self.service, key)
            .map_err(|e| SecretError::ReadError(e.to_string()))
    }
}

#[cfg(feature = "keychain")]
impl SecretStore for KeychainSecretStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        let entry = self.entry(key)?;
        match entry.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(SecretError::ReadError(e.to_string())),
        }
    }
    
    fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        let entry = self.entry(key)?;
        entry.set_password(value)
            .map_err(|e| SecretError::WriteError(e.to_string()))
    }
    
    fn delete(&self, key: &str) -> Result<(), SecretError> {
        let entry = self.entry(key)?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()), // Already gone
            Err(e) => Err(SecretError::DeleteError(e.to_string())),
        }
    }
}
```

**Tests Required:**
- Basic tests similar to FileSecretStore (behind feature flag)
- Manual testing on each platform is recommended

---

### Step 6: Reorganize Secrets Module

Split `src/secrets/traits.rs` into proper module structure.

**Files:**
- `src/secrets/mod.rs` - Module root with re-exports
- `src/secrets/traits.rs` - Just the `SecretStore` trait and errors
- `src/secrets/file_store.rs` - FileSecretStore implementation
- `src/secrets/keychain_store.rs` - KeychainSecretStore (feature-gated)

**Module Structure:**

```rust
// src/secrets/mod.rs
//! secrets
//!
//! Secret storage abstraction for tokens and credentials.
//!
//! # Architecture
//!
//! Secrets are stored through the `SecretStore` trait, which has
//! multiple implementations:
//!
//! - `FileSecretStore`: Stores in `~/.lattice/secrets.toml` (default)
//! - `KeychainSecretStore`: Uses OS keychain (optional, feature-gated)
//!
//! # Security
//!
//! - Secrets are never logged or included in error messages
//! - File store uses 0600 permissions on Unix
//! - All writes are atomic (temp file + rename)

mod traits;
mod file_store;
#[cfg(feature = "keychain")]
mod keychain_store;

pub use traits::{SecretStore, SecretError};
pub use file_store::FileSecretStore;
#[cfg(feature = "keychain")]
pub use keychain_store::KeychainSecretStore;
```

---

### Step 7: Add SecretStore Factory

Add a factory function to create the appropriate secret store based on configuration.

**File:** `src/secrets/mod.rs`

```rust
/// Create a secret store based on the provider name.
///
/// Valid providers:
/// - "file" (default) - FileSecretStore
/// - "keychain" - KeychainSecretStore (requires feature)
pub fn create_store(provider: &str) -> Result<Box<dyn SecretStore>, SecretError> {
    match provider {
        "file" => Ok(Box::new(FileSecretStore::new()?)),
        #[cfg(feature = "keychain")]
        "keychain" => Ok(Box::new(KeychainSecretStore::new()?)),
        #[cfg(not(feature = "keychain"))]
        "keychain" => Err(SecretError::ReadError(
            "keychain support not enabled (compile with --features keychain)".into()
        )),
        other => Err(SecretError::ReadError(
            format!("unknown secret provider: {}", other)
        )),
    }
}
```

---

### Step 8: Update Journal Types

Enhance the journal types to include ref snapshots for proper undo support.

**File:** `src/core/ops/journal.rs`

**Additions:**

```rust
/// A snapshot of a ref's state before/after an operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefSnapshot {
    /// The ref name
    pub refname: String,
    /// OID before the operation (None if created)
    pub before: Option<String>,
    /// OID after the operation (None if deleted)
    pub after: Option<String>,
}

/// Enhanced step kind with ref snapshots.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StepKind {
    /// A ref update with before/after values
    RefUpdate {
        refname: String,
        old_oid: Option<String>,
        new_oid: String,
    },
    /// A metadata write with full snapshot
    MetadataWrite {
        branch: String,
        old_ref_oid: Option<String>,
        new_ref_oid: String,
    },
    /// A metadata delete
    MetadataDelete {
        branch: String,
        old_ref_oid: String,
    },
    /// A checkpoint marker
    Checkpoint { name: String },
    /// A git process was run
    GitProcess {
        args: Vec<String>,
        description: String,
    },
}

impl Journal {
    /// Record a ref update step.
    pub fn record_ref_update(
        &mut self,
        refname: impl Into<String>,
        old_oid: Option<String>,
        new_oid: impl Into<String>,
    ) {
        self.add_step(StepKind::RefUpdate {
            refname: refname.into(),
            old_oid,
            new_oid: new_oid.into(),
        });
    }
    
    /// Record a metadata write step.
    pub fn record_metadata_write(
        &mut self,
        branch: impl Into<String>,
        old_ref_oid: Option<String>,
        new_ref_oid: impl Into<String>,
    ) {
        self.add_step(StepKind::MetadataWrite {
            branch: branch.into(),
            old_ref_oid,
            new_ref_oid: new_ref_oid.into(),
        });
    }
    
    /// Record a metadata delete step.
    pub fn record_metadata_delete(
        &mut self,
        branch: impl Into<String>,
        old_ref_oid: impl Into<String>,
    ) {
        self.add_step(StepKind::MetadataDelete {
            branch: branch.into(),
            old_ref_oid: old_ref_oid.into(),
        });
    }
}
```

---

### Step 9: Implement Journal Persistence

Add methods to read/write journals to disk with fsync.

**File:** `src/core/ops/journal.rs`

```rust
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

impl Journal {
    /// Path to the journal file.
    pub fn path(git_dir: &Path) -> PathBuf {
        git_dir.join("lattice").join("ops")
    }
    
    /// Full path to this journal's file.
    pub fn file_path(&self, git_dir: &Path) -> PathBuf {
        Self::path(git_dir).join(format!("{}.json", self.op_id))
    }
    
    /// Write the journal to disk with fsync.
    pub fn write(&self, git_dir: &Path) -> Result<(), JournalError> {
        let dir = Self::path(git_dir);
        fs::create_dir_all(&dir)?;
        
        let path = self.file_path(git_dir);
        let content = serde_json::to_string_pretty(self)?;
        
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        
        Ok(())
    }
    
    /// Read a journal from disk.
    pub fn read(git_dir: &Path, op_id: &OpId) -> Result<Self, JournalError> {
        let path = Self::path(git_dir).join(format!("{}.json", op_id));
        let content = fs::read_to_string(&path)?;
        let journal = serde_json::from_str(&content)?;
        Ok(journal)
    }
    
    /// List all journal files.
    pub fn list(git_dir: &Path) -> Result<Vec<OpId>, JournalError> {
        let dir = Self::path(git_dir);
        if !dir.exists() {
            return Ok(vec![]);
        }
        
        let mut ids = vec![];
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            if let Some(name) = entry.file_name().to_str() {
                if let Some(id) = name.strip_suffix(".json") {
                    ids.push(OpId(id.to_string()));
                }
            }
        }
        Ok(ids)
    }
}

/// Errors from journal operations.
#[derive(Debug, Error)]
pub enum JournalError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    
    #[error("journal not found: {0}")]
    NotFound(String),
}
```

---

### Step 10: Implement OpState Persistence

Add methods to read/write the op-state marker.

**File:** `src/core/ops/journal.rs`

```rust
impl OpState {
    /// Path to the op-state file.
    pub fn path(git_dir: &Path) -> PathBuf {
        git_dir.join("lattice").join("op-state.json")
    }
    
    /// Write the op-state marker.
    pub fn write(&self, git_dir: &Path) -> Result<(), JournalError> {
        let dir = git_dir.join("lattice");
        fs::create_dir_all(&dir)?;
        
        let path = Self::path(git_dir);
        let content = serde_json::to_string_pretty(self)?;
        
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        
        Ok(())
    }
    
    /// Read the op-state marker, if it exists.
    pub fn read(git_dir: &Path) -> Result<Option<Self>, JournalError> {
        let path = Self::path(git_dir);
        if !path.exists() {
            return Ok(None);
        }
        
        let content = fs::read_to_string(&path)?;
        let state = serde_json::from_str(&content)?;
        Ok(Some(state))
    }
    
    /// Remove the op-state marker.
    pub fn remove(git_dir: &Path) -> Result<(), JournalError> {
        let path = Self::path(git_dir);
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(())
    }
    
    /// Check if an op-state marker exists.
    pub fn exists(git_dir: &Path) -> bool {
        Self::path(git_dir).exists()
    }
}
```

---

### Step 11: Create Integration Tests

Create comprehensive integration tests for the persistence layer.

**File:** `tests/persistence_integration.rs`

**Test Categories:**

1. **MetadataStore Tests:**
   - Create, read, update, delete with real git repos
   - CAS failure scenarios
   - Concurrent access handling
   - Large metadata handling

2. **FileSecretStore Tests:**
   - Store and retrieve secrets
   - Permission verification (Unix)
   - Atomic write testing
   - Directory creation

3. **RepoLock Tests:**
   - Basic locking
   - Contention detection
   - Drop releases lock
   - Cross-process locking (spawn subprocess)

4. **Journal Tests:**
   - Write and read roundtrip
   - List journals
   - Op-state marker lifecycle

---

### Step 12: Documentation and Doctests

Add comprehensive documentation with doctests.

**Files:** All modified module files

**Required doctests:**
- `MetadataStore::new` usage example
- `MetadataStore::read` with matching
- `MetadataStore::write_cas` creation and update
- `FileSecretStore::new` usage
- `SecretStore` trait usage
- `RepoLock::acquire` usage
- `Journal::new` and step recording
- `OpState` lifecycle

---

## Files to Create/Modify

| File | Action | Description |
|------|--------|-------------|
| `Cargo.toml` | Modify | Add fs2, keyring dependencies |
| `src/core/ops/lock.rs` | Modify | Full RepoLock implementation with fs2 |
| `src/core/ops/journal.rs` | Modify | Add persistence, enhanced step types |
| `src/core/metadata/store.rs` | Modify | Full MetadataStore using Git interface |
| `src/secrets/mod.rs` | Modify | Add re-exports and factory |
| `src/secrets/traits.rs` | Modify | Trait and errors only |
| `src/secrets/file_store.rs` | Create | FileSecretStore implementation |
| `src/secrets/keychain_store.rs` | Create | KeychainSecretStore (feature-gated) |
| `tests/persistence_integration.rs` | Create | Integration tests |

---

## Test Count Target

| Category | Count |
|----------|-------|
| Existing tests | 165 |
| RepoLock tests | ~10 |
| MetadataStore tests | ~15 |
| FileSecretStore tests | ~12 |
| Journal persistence tests | ~10 |
| Integration tests | ~15 |
| **Target Total** | **~225** |

---

## Implementation Notes

### CAS Semantics

The MetadataStore uses the Git interface's CAS operations to ensure correctness:

1. Read current ref OID before any update
2. Pass expected OID to `update_ref_cas`
3. Git interface verifies precondition atomically
4. Failure returns `CasFailed` with current value

This prevents race conditions when multiple processes or out-of-band changes occur.

### Secret Security

The FileSecretStore enforces multiple security measures:

1. File permissions set to 0600 BEFORE writing content
2. Atomic writes via temp file + rename
3. Secrets never included in error messages
4. Secrets never logged (even in debug mode)

### Lock Contention

The RepoLock uses `fs2::try_lock_exclusive()` for non-blocking lock acquisition:

1. Creates `.git/lattice/lock` file if needed
2. Attempts exclusive flock
3. Returns `AlreadyLocked` immediately if held
4. Lock released on Drop (RAII pattern)

This allows commands to fail fast when another Lattice instance is running.

---

## Next Steps (Milestone 4)

Per ROADMAP.md, proceed to **Milestone 4: Engine Lifecycle**:
- Scanner with capabilities
- Event ledger
- Gating with ReadyContext/RepairBundle
- Planner and Executor
- Fast verify
