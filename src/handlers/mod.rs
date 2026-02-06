//! LSP request handlers
//!
//! This module contains the logic for LSP requests, keeping main.rs focused
//! on the Backend struct and thin handler delegation.

pub mod lens;
pub mod linked;
pub mod navigation;
pub mod selection;
pub mod semantic;
pub mod signature;
pub mod symbols;
pub mod util;
