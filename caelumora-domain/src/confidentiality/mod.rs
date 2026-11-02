//! Confidentiality (issue 133): who may know what - how sensitive a piece of information is and for
//! whom it is meant, who is cleared for it, how far a reader reads it, and whether it leaves its
//! owner at all.
//!
//! A `Classification` is a level from a fixed scale and an audience of compartments - areas such as
//! a topic, a group or a customer, which the administrator configures (`Compartments`). What may
//! flow where is decided by the one flow rule, `Classification::flows_to`, from the classifications
//! involved alone - not by who takes part in a channel at the moment, so that a decision stays the
//! same when someone joins or leaves. The decisions are a `ConfidentialityPolicy`'s, apart from who
//! may do what (`AccessPolicy`) and asked beside it (issue 304); `ConfidentialityRules` is the one
//! Caelumora applies - both `caelumora-security`'s (issue 517).
//!
//! Whether anyone but an actor's owner sees something at all is a separate axis, `Visibility`:
//! ownership, not sensitivity.
//!
//! Its parts: the classification and the flow rule over it (`classification`), what set the label
//! of a message (`label`), who is cleared for what (`clearance`), how far a reader reads a place
//! (`reading`), and the second axis (`visibility`).

mod classification;
mod clearance;
mod label;
mod reading;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
mod visibility;

pub use classification::{
    Classification, ClassificationError, Compartment, Compartments, Confidentiality,
};
pub use clearance::{Clearance, Clearances};
pub use label::LabelCauses;
pub use reading::{Place, Reading};
pub use visibility::Visibility;
