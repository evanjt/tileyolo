//! Testing utilities for TileYolo.
//!
//! This module provides utilities for generating test COGs and helper functions
//! for integration tests.

pub mod cog_generator;
pub mod helpers;

pub use cog_generator::{TestCogGenerator, TestPattern};
