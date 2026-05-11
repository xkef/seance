use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use seance_mux::LinkTarget;

const ALLOWED_SCHEMES: &[&str] = &["http", "https", "ftp", "ftps", "file", "mailto"];

pub(crate) fn open_link(target: &LinkTarget, pwd: Option<&str>) -> bool {
    let Some(target) = resolve_open_target(target, pwd) else {
        tracing::warn!("refusing to open unresolved link target: {target:?}");
        return false;
    };
    let launcher = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut command = Command::new(launcher);
    match &target {
        OpenTarget::Url(url) => {
            command.arg(url);
        }
        OpenTarget::Path(path) => {
            command.arg(path);
        }
    }
    match command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(_) => true,
        Err(err) => {
            tracing::warn!("failed to spawn {launcher}: {err}");
            false
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OpenTarget {
    Url(String),
    Path(PathBuf),
}

pub(crate) fn resolve_open_target(target: &LinkTarget, pwd: Option<&str>) -> Option<OpenTarget> {
    match target {
        LinkTarget::Url(url) => is_allowed_scheme(url).then(|| OpenTarget::Url(url.to_owned())),
        LinkTarget::Path(path) => resolve_path(path, pwd).map(OpenTarget::Path),
    }
}

fn resolve_path(path: &str, pwd: Option<&str>) -> Option<PathBuf> {
    let path = expand_path(path.trim_end(), pwd)?;
    path.exists().then_some(path)
}

fn expand_path(path: &str, pwd: Option<&str>) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return std::env::var_os("HOME").map(|home| PathBuf::from(home).join(rest));
    }
    if let Some(expanded) = expand_env_path(path) {
        return Some(expanded);
    }
    let path = Path::new(path);
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    pwd.map(|pwd| Path::new(pwd).join(path))
}

fn expand_env_path(path: &str) -> Option<PathBuf> {
    let rest = path.strip_prefix('$')?;
    let end = rest
        .char_indices()
        .find_map(|(idx, ch)| (!(ch == '_' || ch.is_ascii_alphanumeric())).then_some(idx))
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    let (var, suffix) = rest.split_at(end);
    let value = std::env::var_os(var)?;
    let suffix = suffix.strip_prefix('/').unwrap_or(suffix);
    Some(PathBuf::from(value).join(suffix))
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
    use super::*;
    use std::fs;

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

    #[test]
    fn relative_paths_resolve_against_pwd_when_they_exist() {
        let dir = std::env::temp_dir().join("seance-link-open-relative");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("dotfiles/shell")).unwrap();
        let file = dir.join("dotfiles/shell/.hushlogin");
        fs::write(&file, "").unwrap();

        let target = resolve_open_target(
            &LinkTarget::Path("dotfiles/shell/.hushlogin".to_string()),
            dir.to_str(),
        );

        assert_eq!(target, Some(OpenTarget::Path(file)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unresolved_relative_paths_are_rejected() {
        let target = resolve_open_target(
            &LinkTarget::Path("dotfiles/shell/.missing".to_string()),
            Some("/tmp"),
        );

        assert_eq!(target, None);
    }

    #[test]
    fn disallowed_url_schemes_are_rejected_as_targets() {
        assert_eq!(
            resolve_open_target(&LinkTarget::Url("javascript:alert(1)".to_string()), None),
            None
        );
    }
}
