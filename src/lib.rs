//! Vimputti - Virtual Input Device Emulation Library
//!
//! This library provides a high-level API for creating and controlling
//! virtual input devices in isolated containers.

pub mod client;
pub mod manager;
pub mod protocol;
pub mod templates;

// Re-export commonly used types
pub use protocol::{
    Axis, AxisConfig, BusType, Button, DeviceConfig, DeviceId, DeviceInfo, InputEvent,
};

pub use client::{VimputtiClient, VirtualController};
pub use templates::{ControllerBuilder, ControllerTemplates};
