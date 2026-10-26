//! Host process resources; OS service registration and software installation have separate lifecycles.
mod config;
mod io;
pub mod permissions;
mod process;
mod resources;
mod runner;
pub mod installation;
pub mod service;
pub use config::{ExecutableResource, PLATFORM, PlatformConfig, register_configuration};
pub use process::{
    ManagedProcess, Platform, ProcessExit, ProcessResult, ProcessSpec, ProcessWorkspace, StopReason,
};
