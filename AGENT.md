# Agent Guidelines

Welcome, coding agent! This document provides guidelines for contributing to Caelumora. It covers coding style, testing, and other relevant topics to ensure consistency and quality across the codebase.

## Architectural Overview
Before making any changes, please refer to [ARCHITECTURE.md](ARCHITECTURE.md) for a detailed explanation of the project's design, including:
*   Top-level module structure
*   Component architecture
*   The `AgentLoop` cognitive cycle
*   The Actor-based communication model

## Coding Style

### Comments and Documentation
*   **Doc Comments**: Use `///` for public APIs (structs, traits, methods). Document parameters, return values, and potential errors.
*   **Internal Comments**: Use `//` for implementation details. Explain *why* something is done if it's not obvious from the code.
*   **Module Docs**: Use `//!` at the top of new modules to describe their purpose.
*   **Consistency**: Mirror the existing comment frequency and style in the module you are editing.

### Error Handling
*   Use `anyhow::Result` for general application logic, especially in the `runtime` where diverse error types are aggregated.
*   Use `thiserror` for defining domain-specific error types (e.g., in `caelumora-model`).
*   Avoid `unwrap()` or `expect()` in production code. Use `?` or handle errors explicitly.

### Asynchronous Programming
*   Caelumora makes heavy use of `async/await`.
*   Prefer `tokio` for async primitives (e.g., `Mutex`, `RwLock`, `mpsc`).
*   Traits with async methods should use `#[async_trait]` if they need to be object-safe or if you are working in a part of the codebase already using it. Note: Many internal traits use `BoxStream` for streaming responses.

## Testing Strategy

### Unit Tests
*   **Placement**: Put unit tests in a `mod tests` block at the bottom of the file, guarded by `#[cfg(test)]`.
*   **Async Tests**: Use `#[tokio::test]` for tests involving async code.
*   **Naming**: Use descriptive names for test functions (e.g., `state_history_writer_is_passthrough_and_persists_history`).
*   **Assertions**: Use `assert!`, `assert_eq!`, and `expect()` with descriptive messages.

### Integration Tests
*   Place integration tests in the `tests/` directory of the relevant crate or in the `examples/` directory if they serve as documentation.

## Contribution Workflow
1.  **Analyze**: Understand the requirement and how it fits into the architecture.
2.  **Plan**: Identify the files that need modification and the tests required.
3.  **Implement**: Apply minimal, focused changes following the coding style.
4.  **Verify**: Run existing tests and add new ones to cover your changes.
5.  **Refactor**: Ensure the code is clean and well-documented.

## Agent-Specific Tips
*   When adding new components, remember to register them in the appropriate `ComponentRegistry`.
*   If you modify `caelumora-model`, ensure compatibility with `caelumora-runtime`.
*   Use the `log` crate for debugging (`debug!`, `info!`, `warn!`, `error!`).
