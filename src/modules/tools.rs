pub mod application;
pub mod domain;
pub mod infrastructure;

/// Characterization snapshot of the tool surface, co-located with the tools it freezes (test-only).
#[cfg(test)]
mod characterization;
