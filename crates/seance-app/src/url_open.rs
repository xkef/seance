//! Minimal launcher for OSC 8 hyperlink targets.
//!
//! Defends against malicious URI schemes (`javascript:`, `vbscript:`, custom
//! protocol handlers) by allowlisting the schemes a terminal should plausibly
//! open, then shelling out to the platform handler. The URL is passed as a
//! single argv argument — never through a shell — so embedded shell
//! metacharacters cannot escape.

use std::process::{Command, Stdio};

const ALLOWED_SCHEMES: &[&str] = &["http", "https", "ftp", "ftps", "file", "mailto"];

/// Launch `url` via the platform's default handler if its scheme is on the
/// allowlist. Returns `false` (and logs a warning) when the scheme is
/// rejected or the launcher fails to spawn.
pub(crate) fn open_hyperlink(url: &str) -> bool {
    if !is_allowed_scheme(url) {
        log::warn!("refusing to open hyperlink with disallowed scheme: {url}");
        return false;
    }
    let launcher = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    match Command::new(launcher)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(_) => true,
        Err(err) => {
            log::warn!("failed to spawn {launcher} for {url}: {err}");
            false
        }
    }
}

fn is_allowed_scheme(url: &str) -> bool {
    let Some(colon) = url.find(':') else {
        return false;
    };
    let scheme = &url[..colon];
    if scheme.is_empty() {
        return false;
    }
    let scheme_lower = scheme.to_ascii_lowercase();
    ALLOWED_SCHEMES
        .iter()
        .any(|allowed| *allowed == scheme_lower)
}

#[cfg(test)]
mod tests {
    use super::is_allowed_scheme;

    #[test]
    fn http_https_file_mailto_accepted() {
        assert!(is_allowed_scheme("http://example.com"));
        assert!(is_allowed_scheme("https://example.com/path?q=1"));
        assert!(is_allowed_scheme("HTTPS://EXAMPLE.com"));
        assert!(is_allowed_scheme("file:///etc/hosts"));
        assert!(is_allowed_scheme("mailto:user@example.com"));
        assert!(is_allowed_scheme("ftp://ftp.example.com"));
    }

    #[test]
    fn javascript_and_unscheme_rejected() {
        assert!(!is_allowed_scheme("javascript:alert(1)"));
        assert!(!is_allowed_scheme("vbscript:foo"));
        assert!(!is_allowed_scheme("data:text/html,foo"));
        assert!(!is_allowed_scheme("example.com"));
        assert!(!is_allowed_scheme(":empty-scheme"));
        assert!(!is_allowed_scheme(""));
    }
}
