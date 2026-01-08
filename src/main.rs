//! Lattice CLI entry point.

use lattice::cli;

fn main() -> anyhow::Result<()> {
    cli::run()
}
