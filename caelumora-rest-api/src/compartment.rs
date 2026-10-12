//! The compartments a classification may name (issue 133).

use caelumora_model::confidentiality::Compartment;
use serde::Serialize;

/// A compartment a classification may name (issue 133), as configured.
#[derive(Debug, Serialize)]
pub struct CompartmentResponse {
    pub name: String,
}

impl From<&Compartment> for CompartmentResponse {
    fn from(compartment: &Compartment) -> Self {
        Self { name: compartment.name().to_string() }
    }
}
