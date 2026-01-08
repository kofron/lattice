//! changelog command - Show version and changelog

use anyhow::Result;

/// Show version and changelog.
pub fn changelog() -> Result<()> {
    println!("lattice {}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("A Rust-native CLI for stacked branches and PRs.");
    println!();
    println!("Repository: https://github.com/lattice-cli/lattice");
    println!();
    println!("## Recent Changes");
    println!();
    println!("- Phase 1: Core local stack engine");
    println!("  - Stack graph and metadata management");
    println!("  - Branch tracking with parent relationships");
    println!("  - Restack with conflict handling");
    println!("  - Operation journaling for crash safety");
    println!();
    println!(
        "For full changelog, see: https://github.com/lattice-cli/lattice/blob/main/CHANGELOG.md"
    );

    Ok(())
}
