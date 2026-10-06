pub mod platform;

pub mod linux;
pub mod uefi;

pub use platform::*;

pub use linux::*;
pub use uefi::*;
