//! The backends of the secrets (issue 324): the kernel's `SecretProvider`s - the environment of the
//! process (`env`) and a file of secrets (`file`) -, which the `SecretManager` asks in the order
//! configured.

pub mod env;
pub mod file;

use crate::backend::Backends;

/// Registers the secret providers the kernel brings with `backends`: `file` and `env`.
pub(crate) fn register_backends(backends: &mut Backends) {
    file::register_backend(backends);
    env::register_backend(backends);
}
