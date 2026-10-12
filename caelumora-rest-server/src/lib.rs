//! The REST server as a library (issue 323): what a handler needs - the
//! logged-in user (`auth`), the errors it answers with and the extractors
//! that answer like them (`error`), the state handlers share (`state`), the
//! event streams (`sse`) - and the core's routes (`routes::core`), which
//! an application nests where it serves the API, beside the routes of its
//! plugins. The application is `caelumora-app`.

pub mod auth;
pub mod error;
pub mod routes;
pub mod sse;
pub mod state;
