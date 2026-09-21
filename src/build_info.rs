//! Build identity helpers.

pub const BASE_VERSION: &str = env!("CARGO_PKG_VERSION");

// Modified by ke: ke's own release version. BASE_VERSION stays the upstream herdr version ke is
// based on. Release tags are `ke-v<KE_VERSION>`; the ke release workflow checks they match.
pub const KE_VERSION: &str = "0.6.1";
// Modified by ke: where ke releases and the installer live.
#[cfg(windows)]
pub const KE_INSTALL_SCRIPT_URL: &str =
    "https://github.com/Little-Z7/ke/releases/latest/download/ke-install.ps1";
#[cfg(not(windows))]
pub const KE_INSTALL_SCRIPT_URL: &str =
    "https://github.com/Little-Z7/ke/releases/latest/download/ke-install.sh";
#[cfg(windows)]
pub const KE_INSTALL_COMMAND: &str = "powershell -ExecutionPolicy Bypass -c \"irm https://github.com/Little-Z7/ke/releases/latest/download/ke-install.ps1 | iex\"";
#[cfg(not(windows))]
pub const KE_INSTALL_COMMAND: &str =
    "curl -fsSL https://github.com/Little-Z7/ke/releases/latest/download/ke-install.sh | sh";

pub fn channel() -> &'static str {
    non_empty(option_env!("HERDR_BUILD_CHANNEL")).unwrap_or("stable")
}

pub fn build_id() -> Option<&'static str> {
    non_empty(option_env!("HERDR_BUILD_ID"))
}

pub fn version() -> String {
    match channel() {
        "stable" => BASE_VERSION.to_string(),
        channel => match build_id() {
            Some(build_id) => format!("{BASE_VERSION}-{channel}.{build_id}"),
            None => format!("{BASE_VERSION}-{channel}"),
        },
    }
}

pub fn is_preview() -> bool {
    channel() == "preview"
}

fn non_empty(value: Option<&'static str>) -> Option<&'static str> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn stable_version_defaults_to_cargo_version() {
        assert!(!super::version().is_empty());
    }
}
