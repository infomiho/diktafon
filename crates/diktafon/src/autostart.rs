//! Login-item registration via SMAppService (macOS 13+). Deliberately not a
//! LaunchAgent plist: those show up in System Settings attributed to the
//! codesigning developer instead of the app. Only works when running from the
//! app bundle, since the service is the main bundle itself.

use anyhow::{Context, Result, bail};
use objc2_service_management::{SMAppService, SMAppServiceStatus};

pub fn is_enabled() -> bool {
    let status = unsafe { SMAppService::mainAppService().status() };
    status == SMAppServiceStatus::Enabled
}

pub fn set(enabled: bool) -> Result<()> {
    let service = unsafe { SMAppService::mainAppService() };
    if enabled {
        unsafe { service.registerAndReturnError() }
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("registering the login item (run from diktafon.app, not a bare binary)")
    } else {
        unsafe { service.unregisterAndReturnError() }
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("unregistering the login item")
    }
}

pub fn run(mode: &str) -> Result<()> {
    match mode {
        "on" => {
            set(true)?;
            println!("autostart enabled");
        }
        "off" => {
            set(false)?;
            println!("autostart disabled");
        }
        "status" => {
            let status = match unsafe { SMAppService::mainAppService().status() } {
                SMAppServiceStatus::Enabled => "enabled",
                SMAppServiceStatus::NotRegistered => "not registered",
                SMAppServiceStatus::RequiresApproval => {
                    "requires approval in System Settings > General > Login Items"
                }
                SMAppServiceStatus::NotFound => {
                    "not found (never registered from this location, or not running from diktafon.app)"
                }
                _ => "unknown",
            };
            println!("autostart: {status}");
        }
        other => bail!("unknown autostart mode {other:?}; use on, off, or status"),
    }
    Ok(())
}
