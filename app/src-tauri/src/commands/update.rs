use serde::Serialize;

const REPO_LATEST_RELEASE_API: &str = "https://api.github.com/repos/satvikydv/pointr/releases/latest";

#[derive(Debug, Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub url: String,
}

/// (major, minor, patch), missing/unparseable parts default to 0 — good
/// enough for our own controlled `vX.Y.Z` release tags, not a general
/// semver parser (no pre-release/build-metadata handling).
fn parse_version(v: &str) -> (u32, u32, u32) {
    let v = v.trim().trim_start_matches('v');
    let mut parts = v.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

/// Lightweight check-and-prompt — no signing keys, no manifest, just a
/// single GitHub API call compared against the version this binary was
/// built with. Returns None when already current (including if the check
/// itself fails, e.g. offline — never surfaces network errors to the user
/// for something this non-essential).
#[tauri::command]
pub async fn check_for_update() -> Result<Option<UpdateInfo>, String> {
    let client = reqwest::Client::new();
    let res = client
        .get(REPO_LATEST_RELEASE_API)
        // GitHub's API 403s anonymous requests with no User-Agent.
        .header("User-Agent", "pointr-app")
        .send()
        .await;

    let Ok(res) = res else { return Ok(None) };
    if !res.status().is_success() {
        return Ok(None);
    }
    let Ok(json) = res.json::<serde_json::Value>().await else { return Ok(None) };
    let Some(tag_name) = json.get("tag_name").and_then(|v| v.as_str()) else { return Ok(None) };
    let Some(html_url) = json.get("html_url").and_then(|v| v.as_str()) else { return Ok(None) };

    let current = parse_version(env!("CARGO_PKG_VERSION"));
    let latest = parse_version(tag_name);

    if latest > current {
        Ok(Some(UpdateInfo {
            version: tag_name.trim_start_matches('v').to_string(),
            url: html_url.to_string(),
        }))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod version_tests {
    use super::*;

    #[test]
    fn parses_a_v_prefixed_tag() {
        assert_eq!(parse_version("v0.1.0"), (0, 1, 0));
    }

    #[test]
    fn parses_without_v_prefix() {
        assert_eq!(parse_version("1.2.3"), (1, 2, 3));
    }

    #[test]
    fn detects_a_newer_patch() {
        assert!(parse_version("v0.1.1") > parse_version("0.1.0"));
    }

    #[test]
    fn detects_a_newer_minor_over_a_higher_patch() {
        // 0.2.0 must beat 0.1.9 — tuple compare, not naive string compare.
        assert!(parse_version("v0.2.0") > parse_version("v0.1.9"));
    }

    #[test]
    fn equal_versions_are_not_newer() {
        assert!(!(parse_version("v0.1.0") > parse_version("0.1.0")));
    }

    #[test]
    fn malformed_tag_falls_back_to_zeros_not_a_panic() {
        assert_eq!(parse_version("not-a-version"), (0, 0, 0));
    }
}

#[tauri::command]
pub fn open_release_page(url: String) -> Result<(), String> {
    std::process::Command::new("cmd")
        .args(["/c", "start", "", &url])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Failed to open browser: {}", e))
}
