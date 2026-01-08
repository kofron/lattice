//! Lattice CLI entry point.

use latticework::cli;

fn main() -> anyhow::Result<()> {
    cli::run()
}
