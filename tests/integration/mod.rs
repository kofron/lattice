//! Integration tests for Lattice.
//!
//! These tests exercise the full CLI and verify behavior against real Git repos.

use assert_cmd::Command;

/// Get a command for running lattice.
fn lattice() -> Command {
    Command::cargo_bin("lattice").unwrap()
}

#[test]
fn hello_command_succeeds() {
    lattice()
        .arg("hello")
        .assert()
        .success()
        .stdout(predicates::str::contains("Hello from Lattice!"));
}

#[test]
fn version_flag_works() {
    lattice()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicates::str::contains("lattice"));
}

#[test]
fn help_flag_works() {
    lattice()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("stacked branches"));
}

mod predicates {
    pub mod str {
        use std::fmt;

        pub struct ContainsPredicate {
            expected: String,
        }

        impl ContainsPredicate {
            pub fn new(expected: &str) -> Self {
                Self {
                    expected: expected.to_string(),
                }
            }
        }

        impl predicates_core::Predicate<str> for ContainsPredicate {
            fn eval(&self, variable: &str) -> bool {
                variable.contains(&self.expected)
            }

            fn find_case<'a>(
                &'a self,
                expected: bool,
                variable: &str,
            ) -> Option<predicates_core::reflection::Case<'a>> {
                if self.eval(variable) == expected {
                    Some(predicates_core::reflection::Case::new(None, expected))
                } else {
                    None
                }
            }
        }

        impl predicates_core::reflection::PredicateReflection for ContainsPredicate {}

        impl fmt::Display for ContainsPredicate {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "contains {:?}", self.expected)
            }
        }

        pub fn contains(expected: &str) -> ContainsPredicate {
            ContainsPredicate::new(expected)
        }
    }
}
