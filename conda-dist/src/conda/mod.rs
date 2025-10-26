pub mod channels;
pub mod gateway;
pub mod lockfile;
pub mod platforms;
pub mod solver;
pub mod virtual_packages;

pub use channels::{DEFAULT_CHANNEL, parse_channels};
pub use gateway::build_gateway;
pub use lockfile::{LOCKFILE_NAME, build_lockfile, load_locked_packages};
pub use platforms::{augment_with_noarch, resolve_target_platforms};
pub use solver::solve_environment;
pub use virtual_packages::detect_virtual_packages_for_platform;
