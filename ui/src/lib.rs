//! Portable presentation layer: no filesystem, registry, service or VPN access.
pub use slint;
slint::include_modules!();
pub mod state;
pub mod visuals;
