//! Application settings services and persistence boundaries.

mod service;

pub use service::{
    list_wsl_distros_blocking, load_settings_blocking, save_settings_internal,
    update_settings_internal, SaveSettingsError,
};
