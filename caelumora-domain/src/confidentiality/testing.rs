//! Classifications and compartments as the tests of this crate and of others write them.

use super::{Classification, Compartment, Compartments, Confidentiality};

/// The compartment `name`.
pub fn compartment(name: &str) -> Compartment {
    Compartment::new(name)
}

/// What is classified `level` for the compartments named `compartments`.
#[expect(clippy::expect_used, reason = "test code: a classification that is none fails the test")]
pub fn classified(level: Confidentiality, compartments: &[&str]) -> Classification {
    Classification::new(level, compartments.iter().map(|c| compartment(c)))
        .expect("a test's classification is one")
}

/// The compartments of an organisation: HR management reads HR, HR reads finance, and sales.
#[expect(clippy::expect_used, reason = "test code: compartments that are none fail the test")]
pub fn organisation() -> Compartments {
    Compartments::new([
        (compartment("hr_management"), vec![compartment("hr")]),
        (compartment("hr"), vec![compartment("finance")]),
        (compartment("finance"), vec![]),
        (compartment("sales"), vec![]),
    ])
    .expect("the organisation's compartments read only what is configured")
}
