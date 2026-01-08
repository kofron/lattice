//! completion command - Generate shell completion scripts

use crate::cli::args::{Cli, Shell};
use anyhow::Result;
use clap::CommandFactory;
use clap_complete::{generate, shells};

/// Generate shell completion scripts.
pub fn completion(shell: Shell) -> Result<()> {
    let mut cmd = Cli::command();
    let name = cmd.get_name().to_string();

    match shell {
        Shell::Bash => {
            generate(shells::Bash, &mut cmd, &name, &mut std::io::stdout());
        }
        Shell::Zsh => {
            generate(shells::Zsh, &mut cmd, &name, &mut std::io::stdout());
        }
        Shell::Fish => {
            generate(shells::Fish, &mut cmd, &name, &mut std::io::stdout());
        }
        Shell::PowerShell => {
            generate(shells::PowerShell, &mut cmd, &name, &mut std::io::stdout());
        }
    }

    Ok(())
}
