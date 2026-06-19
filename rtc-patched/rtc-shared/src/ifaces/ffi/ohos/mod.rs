//! Network interfaces module - OpenHarmony stub implementation
//!
//! OpenHarmony does not support nix crate's getifaddrs.
//! This module provides a stub implementation.
//! In sans-I/O architecture, network addresses are provided by the application layer.

use crate::ifaces::Interface;
use std::io::Error;

/// Query the local system for all interface addresses.
///
/// On OpenHarmony, this returns an empty list.
/// The caller should provide network addresses manually via the sans-I/O API.
pub fn ifaces() -> Result<Vec<Interface>, Error> {
    // OpenHarmony stub: return empty list
    Ok(Vec::new())
}
