//! Added by ke: process-environment isolation from an installed upstream herdr.
//!
//! When ke is started from inside an upstream herdr pane (`HERDR_ENV=1` but no `KE_ENV`), the
//! inherited `HERDR_*` locator variables are dropped so ke never talks to herdr's server or trips
//! the nested-session guard. Inside ke's own panes `KE_ENV=1` is set, so the variables are kept
//! and point at ke.

/// Marker ke injects into its own panes and plugin commands next to `HERDR_ENV`.
pub(crate) const KE_ENV_VAR: &str = "KE_ENV";

const INHERITED_HERDR_VARS: [&str; 9] = [
    crate::HERDR_ENV_VAR,
    "HERDR_SOCKET_PATH",
    "HERDR_CLIENT_SOCKET_PATH",
    "HERDR_SESSION",
    "HERDR_CONFIG_PATH",
    "HERDR_PANE_ID",
    "HERDR_TAB_ID",
    "HERDR_WORKSPACE_ID",
    "HERDR_BIN_PATH",
];

pub(crate) fn isolate_inherited_herdr_env() {
    if should_isolate(
        std::env::var_os(KE_ENV_VAR).is_some(),
        std::env::var_os(crate::HERDR_ENV_VAR).is_some(),
    ) {
        for key in INHERITED_HERDR_VARS {
            std::env::remove_var(key);
        }
    }
}

fn should_isolate(has_ke_env: bool, has_herdr_env: bool) -> bool {
    !has_ke_env && has_herdr_env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_foreign_herdr_pane_triggers_isolation() {
        assert!(should_isolate(false, true), "inside upstream herdr");
        assert!(!should_isolate(true, true), "inside ke's own pane");
        assert!(!should_isolate(false, false), "plain terminal");
        assert!(!should_isolate(true, false));
    }

    #[test]
    fn every_locator_variable_is_covered() {
        for key in ["HERDR_SOCKET_PATH", "HERDR_PANE_ID", "HERDR_BIN_PATH"] {
            assert!(INHERITED_HERDR_VARS.contains(&key));
        }
        assert_eq!(INHERITED_HERDR_VARS[0], crate::HERDR_ENV_VAR);
    }
}
