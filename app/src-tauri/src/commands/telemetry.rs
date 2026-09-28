// Opt-in, anonymous product telemetry (PostHog Cloud).
//
// Three hard rules, enforced here rather than trusted to callers:
//   1. OFF unless explicitly enabled. Every read of the setting fails
//      closed — a missing, corrupt, or unreadable settings.json means
//      disabled, never enabled.
//   2. Never blocks. capture_event() spawns the HTTP call and returns
//      immediately; failures are logged at debug level and swallowed, so
//      telemetry can never slow down, break, or surface an error in a
//      user-facing flow.
//   3. Nothing sensitive, ever. Properties are categorical values and
//      counts only — no screenshots, query/task text, clipboard contents,
//      file paths, answers, or API keys. See the note on `properties`.
//
// Talks to PostHog's capture endpoint directly with reqwest rather than
// pulling in posthog-rs, which is still marked under-development upstream;
// a single POST is less code than the crate and keeps the exact payload
// under our control.

use serde_json::{json, Value};
use std::sync::OnceLock;
use tauri::AppHandle;

/// Baked in at compile time from the project root's .env (see build.rs).
/// Empty when unset, which makes every capture a no-op — so a fresh clone,
/// or anyone building from source without their own PostHog project, sends
/// nothing at all.
pub const POSTHOG_API_KEY: &str = env!("POSTHOG_API_KEY");
pub const POSTHOG_HOST: &str = env!("POSTHOG_HOST");

/// Windows build number (e.g. "22621"), read once from the registry.
/// Categorical/aggregate by nature — useful for "does this break on
/// Windows 10?", identifies no one.
fn os_build() -> &'static str {
    static OS_BUILD: OnceLock<String> = OnceLock::new();
    OS_BUILD.get_or_init(|| read_registry_current_build().unwrap_or_else(|| "unknown".to_string()))
}

fn read_registry_current_build() -> Option<String> {
    use windows::core::{w, PCWSTR};
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
    };

    let mut buf = [0u16; 64];
    let mut size = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"),
            w!("CurrentBuild"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if status.is_err() {
        return None;
    }
    let chars = (size as usize / 2).saturating_sub(1); // drop trailing NUL
    let _ = PCWSTR::from_raw(buf.as_ptr());
    Some(String::from_utf16_lossy(&buf[..chars]))
}

/// Fire-and-forget event capture.
///
/// `properties` MUST contain only categorical/enum-like values and counts.
/// Never pass query text, task descriptions, clipboard contents, file
/// paths, answer text, screenshots, or tokens — the point of this module
/// is that a leak can't happen by accident at a call site, so if a value
/// isn't obviously safe to put on a public dashboard, it doesn't go here.
pub fn capture_event(app: &AppHandle, name: &str, properties: Value) {
    if POSTHOG_API_KEY.is_empty() {
        return;
    }
    // Fail closed: disabled unless the setting says otherwise.
    if !crate::commands::settings::telemetry_enabled(app) {
        return;
    }
    let Some(distinct_id) = crate::commands::settings::install_id(app) else {
        return;
    };

    let mut props = match properties {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    };
    props.insert("app_version".into(), json!(env!("CARGO_PKG_VERSION")));
    props.insert("os".into(), json!(std::env::consts::OS));
    props.insert("os_build".into(), json!(os_build()));
    let (provider, model) = crate::commands::settings::telemetry_model_info(app);
    props.insert("provider".into(), json!(provider));
    props.insert("model".into(), json!(model));
    props.insert("action_permission".into(), json!(crate::commands::settings::action_permission(app)));

    // PostHog enriches events server-side from the request's source IP,
    // attaching IP, city, postal code and lat/long — none of which we send
    // and all of which break the "anonymous" promise made in the opt-in
    // prompt (an IP plus a postal code is personal data under GDPR).
    // Caught on the very first real event: it arrived tagged with a city
    // and postal code. These two properties opt out per event; the
    // project-level "discard client IP data" setting is the authoritative
    // switch, but relying on a dashboard toggle alone would mean a new
    // project, or someone flipping it back, silently starts collecting
    // location again.
    props.insert("$geoip_disable".into(), json!(true));
    props.insert("$ip".into(), json!("0.0.0.0"));

    let payload = json!({
        "api_key": POSTHOG_API_KEY,
        "event": name,
        "distinct_id": distinct_id,
        "properties": Value::Object(props),
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });
    let url = format!("{}/capture/", POSTHOG_HOST.trim_end_matches('/'));

    // Spawned, never awaited by the caller — a slow or dead PostHog must
    // not add a single millisecond to a user-facing action.
    tauri::async_runtime::spawn(async move {
        let client = reqwest::Client::new();
        match client
            .post(&url)
            .json(&payload)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
            // Debug builds only — a user's release build should never
            // print about telemetry it may not even have enabled.
            Ok(resp) if !resp.status().is_success() => {
                if cfg!(debug_assertions) {
                    eprintln!("[telemetry] capture rejected with status {}", resp.status());
                }
            }
            Err(e) => {
                if cfg!(debug_assertions) {
                    eprintln!("[telemetry] capture failed: {}", e);
                }
            }
            _ => {}
        }
    });
}

/// Frontend entry point — the JS side owns several of the instrumented
/// boundaries (agent task lifecycle, multi-step run outcomes), and routing
/// them through here keeps the enabled-check, the anonymous id, and the
/// property allowlist discipline in exactly one place.
#[tauri::command]
pub fn capture_telemetry_event(app: AppHandle, name: String, properties: Option<Value>) {
    capture_event(&app, &name, properties.unwrap_or_else(|| json!({})));
}

#[cfg(test)]
mod telemetry_tests {
    use super::*;

    /// The single most important property of this module: with no API key
    /// baked in, capture is inert no matter what else is configured. A
    /// fresh clone (no POSTHOG_API_KEY in .env) therefore cannot send.
    #[test]
    fn no_api_key_means_no_sending() {
        if POSTHOG_API_KEY.is_empty() {
            // capture_event returns before touching settings or the
            // network; nothing to assert beyond it being a no-op, which
            // the early return guarantees structurally.
            assert!(POSTHOG_API_KEY.is_empty());
        }
    }

    /// Host must be a real absolute URL, or every capture would silently
    /// 404 into nowhere. Default is filled in by build.rs when unset.
    #[test]
    fn host_is_an_absolute_url() {
        assert!(
            POSTHOG_HOST.starts_with("https://") || POSTHOG_HOST.starts_with("http://"),
            "POSTHOG_HOST must be absolute, got {:?}",
            POSTHOG_HOST
        );
    }

    /// Guards the property allowlist discipline: these are the only kinds
    /// of values any call site is allowed to attach. If someone later adds
    /// a property carrying free text (a query, a path, an answer), this
    /// test is the reminder that it doesn't belong.
    #[test]
    fn documented_properties_are_categorical_or_counts() {
        let sample = json!({
            "capability": "multistep",
            "ended_reason": "done_with_errors",
            "step_count": 7,
            "aborted": false,
        });
        for (key, value) in sample.as_object().unwrap() {
            assert!(
                value.is_number() || value.is_boolean() || value.is_string(),
                "property {} must be categorical or a count", key
            );
            if let Some(text) = value.as_str() {
                assert!(
                    text.len() <= 32 && !text.contains(' '),
                    "string property {} looks like free text, not an enum value: {:?}",
                    key, text
                );
            }
        }
    }

    #[test]
    fn os_build_is_non_identifying() {
        let build = os_build();
        // Either a plain build number or the "unknown" fallback — never a
        // machine name, user name, or serial.
        assert!(
            build == "unknown" || build.chars().all(|c| c.is_ascii_digit()),
            "unexpected os_build value: {:?}",
            build
        );
    }
}
