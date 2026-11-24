pub fn format_timestamp(seconds: f64) -> String {
    let total_seconds = seconds as u64;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let secs = total_seconds % 60;
    format!("{:02}:{:02}:{:02}", hours, minutes, secs)
}

/// Scheme, host and port of a URL for log lines. Drops userinfo, path, query
/// and fragment, which can carry API keys or tokens (e.g. a custom
/// OpenAI-compatible endpoint with `?api-key=`). Ported from upstream beec9b8.
pub(crate) fn url_origin_for_log(raw: &str) -> String {
    let Ok(url) = url::Url::parse(raw) else {
        return "<invalid-url>".to_string();
    };
    let Some(host) = url.host_str() else {
        return "<invalid-url>".to_string();
    };

    match url.port() {
        Some(port) => format!("{}://{}:{}", url.scheme(), host, port),
        None => format!("{}://{}", url.scheme(), host),
    }
}

#[cfg(test)]
mod log_redaction_tests {
    use super::url_origin_for_log;

    #[test]
    fn url_origin_for_log_strips_credentials_path_and_query() {
        assert_eq!(
            url_origin_for_log("https://user:secret@example.com:8443/path?token=secret#fragment"),
            "https://example.com:8443"
        );
        assert_eq!(url_origin_for_log("http://localhost:11434/v1"), "http://localhost:11434");
        assert_eq!(url_origin_for_log("not a URL"), "<invalid-url>");
    }
}

/// Opens macOS System Settings to a specific privacy preference pane
#[cfg(target_os = "macos")]
#[tauri::command]
pub async fn open_system_settings(preference_pane: String) -> Result<(), String> {
    use std::process::Command;

    // Construct the URL for System Settings
    let url = format!("x-apple.systempreferences:com.apple.preference.security?{}", preference_pane);

    // Use the 'open' command on macOS to open the URL
    Command::new("open")
        .arg(&url)
        .spawn()
        .map_err(|e| format!("Failed to open system settings: {}", e))?;

    Ok(())
} 