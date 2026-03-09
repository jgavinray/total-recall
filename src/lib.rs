// Re-export modules so integration tests in tests/ can access them.
// main.rs remains the entry point; lib.rs exposes the library surface.
pub mod config;
pub mod error;
pub mod memory;
