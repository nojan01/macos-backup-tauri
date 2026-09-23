//! AppleArchive puts detailed errno/path errors in Unified Logging, while aa's
//! stderr can contain only "Archive encoding failed". Preserve the real cause so
//! the existing proven-permission-denial / screen-unlock retry can work.
use super::*;
const MAX_DETAILS: usize = 16 * 1024;

pub(crate) fn failure_details(pid: u32, started: chrono::DateTime<chrono::Local>) -> String {
    if BACKUP_CANCELLED.load(Ordering::SeqCst) || VERIFY_CANCELLED.load(Ordering::SeqCst) {
        return String::new();
    }
    let mut command = Command::new("/usr/bin/log");
    command.args(["show", "--style", "json", "--start"])
        .arg((started - chrono::Duration::seconds(1)).format("%Y-%m-%d %H:%M:%S").to_string())
        .arg("--end")
        .arg((chrono::Local::now() + chrono::Duration::seconds(1)).format("%Y-%m-%d %H:%M:%S").to_string())
        .arg("--predicate")
        .arg(format!("processIdentifier == {pid} AND senderImagePath ENDSWITH '/libAppleArchive.dylib' AND (logType == 'Error' OR logType == 'Fault')"))
        .env("LC_ALL", "C");
    match run_with_timeout(command, std::time::Duration::from_secs(10)) {
        Ok(output) if output.status.success() => {
            let details = parse(&output.stdout, pid);
            if details.is_empty() {
                String::new()
            } else {
                format!("\nAppleArchive-Systemdiagnose:\n{details}")
            }
        }
        _ => String::new(), // Keep the original error if log access is unavailable.
    }
}
fn parse(bytes: &[u8], pid: u32) -> String {
    let Ok(serde_json::Value::Array(events)) = serde_json::from_slice(bytes) else {
        return String::new();
    };
    let mut lines = Vec::new();
    let mut length = 0;
    for event in events {
        // Validate provenance again; never act on arbitrary neighboring processes.
        if event.get("processID").and_then(|v| v.as_u64()) != Some(pid as u64)
            || !event
                .get("senderImagePath")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s.ends_with("/libAppleArchive.dylib"))
            || !event
                .get("messageType")
                .and_then(|v| v.as_str())
                .is_some_and(|s| matches!(s, "Error" | "Fault"))
        {
            continue;
        }
        let Some(message) = event.get("eventMessage").and_then(|v| v.as_str()) else {
            continue;
        };
        if lines.iter().any(|s: &String| s == message) {
            continue;
        }
        if length + message.len() > MAX_DETAILS {
            break;
        }
        lines.push(message.to_string());
        length += message.len() + 1;
    }
    lines.join("\n")
}
#[cfg(test)]
mod tests {
    use super::*;
    fn event(pid: u32, message: &str) -> serde_json::Value {
        serde_json::json!({"processID":pid,"senderImagePath":"/usr/lib/libAppleArchive.dylib","messageType":"Error","eventMessage":message})
    }
    #[test]
    fn permission_denial_retains_exact_cause_and_path_from_the_failed_process_only() {
        let events = serde_json::json!([
            event(12, "Operation not permitted: Mail/recentSearches.plist"),
            event(12, "computing file hashes: Mail/recentSearches.plist"),
            event(13, "Permission denied: unrelated"),
            event(12, "Operation not permitted: Mail/recentSearches.plist")
        ]);
        let details = parse(&serde_json::to_vec(&events).unwrap(), 12);
        assert_eq!(details,"Operation not permitted: Mail/recentSearches.plist\ncomputing file hashes: Mail/recentSearches.plist");
    }
    #[test]
    fn malformed_unrelated_and_non_error_logs_cannot_trigger_a_permission_retry() {
        assert!(parse(b"unavailable", 12).is_empty());
        let mut wrong_sender = event(12, "Permission denied");
        wrong_sender["senderImagePath"] = "/other/library".into();
        let mut info = event(12, "Permission denied");
        info["messageType"] = "Info".into();
        assert!(parse(
            &serde_json::to_vec(&vec![wrong_sender, info, event(99, "Permission denied")]).unwrap(),
            12
        )
        .is_empty());
    }
    #[test]
    fn diagnostics_are_bounded_and_do_not_reclassify_io_errors_as_access_denials() {
        let events = vec![
            event(12, "Input/output error: file"),
            event(12, &"x".repeat(MAX_DETAILS)),
        ];
        assert_eq!(
            parse(&serde_json::to_vec(&events).unwrap(), 12),
            "Input/output error: file"
        );
    }
}
