use fancy_regex::Regex;

use crate::LinkTarget;

// Derived from Ghostty's default URL/path matcher in
// vendor/ghostty-src/src/config/url.zig at the vendored ghostty-src revision.
// Ghostty uses Oniguruma; this version keeps the same branch structure while
// replacing the variable-length `$` lookbehind with an equivalent fixed-width
// guard accepted by fancy-regex.
const URL_SCHEMES: &str = r"https?://|mailto:|ftp://|file:|ssh:|git://|ssh://|tel:|magnet:|ipfs://|ipns://|gemini://|gopher://|news:";
const IPV6_URL_PATTERN: &str = r"(?:\[[:0-9a-fA-F]+(?:[:0-9a-fA-F]*)+\](?::[0-9]+)?)";
const SCHEME_URL_CHARS: &str = r"[\w\-.~:/?#@!$&*+,;=%]";
const PATH_CHARS: &str = r"[\w\-.~:/?#@!$&*+;=%]";
const OPTIONAL_BRACKETED_WORD_SUFFIX: &str = r"(?:[\(\[]\w*[\)\]])?";
const NO_TRAILING_PUNCTUATION: &str = r"(?<![,.])";
const NO_TRAILING_COLON: &str = r"(?<!:)";
const TRAILING_SPACES_AT_EOL: &str = r"(?: +(?= *$))?";
const DOTTED_PATH_LOOKAHEAD: &str = r"(?=[\w\-.~:/?#@!$&*+;=%]*\.)";
const NON_DOTTED_PATH_LOOKAHEAD: &str = r"(?![\w\-.~:/?#@!$&*+;=%]*\.)";
const DOTTED_PATH_SPACE_SEGMENTS: &str =
    r"(?:(?<!:) (?!\w+://)(?!\.{0,2}/)(?!~/)[\w\-.~:/?#@!$&*+;=%]*[/.])*";
const ANY_PATH_SPACE_SEGMENTS: &str =
    r"(?:(?<!:) (?!\w+://)(?!\.{0,2}/)(?!~/)[\w\-.~:/?#@!$&*+;=%]+)*";
const ROOTED_OR_RELATIVE_PATH_PREFIX: &str = r"(?:\.\./|\./|(?<!\w)~/|(?:[\w][\w\-.]*/)*(?<!\w)\$[A-Za-z_]\w*/|\.[\w][\w\-.]*/|(?<![\w~/])/(?!/))";
const BARE_RELATIVE_PATH_PREFIX: &str = r"(?<![\w$])[\w][\w\-.]*/";

pub(crate) fn compile(urls: bool, paths: bool) -> Result<Option<Regex>, fancy_regex::Error> {
    let Some(source) = regex_source(urls, paths) else {
        return Ok(None);
    };
    Regex::new(&source).map(Some)
}

fn regex_source(urls: bool, paths: bool) -> Option<String> {
    let mut branches = Vec::new();
    if urls {
        branches.push(scheme_url_branch());
    }
    if paths {
        branches.push(rooted_or_relative_path_branch());
        branches.push(bare_relative_path_branch());
    }
    if branches.is_empty() {
        None
    } else {
        Some(branches.join("|"))
    }
}

fn scheme_url_branch() -> String {
    format!(
        "(?:{URL_SCHEMES})(?:{IPV6_URL_PATTERN}|{SCHEME_URL_CHARS}+{OPTIONAL_BRACKETED_WORD_SUFFIX})+{NO_TRAILING_PUNCTUATION}"
    )
}

fn rooted_or_relative_path_branch() -> String {
    format!(
        "{ROOTED_OR_RELATIVE_PATH_PREFIX}(?:{DOTTED_PATH_LOOKAHEAD}{PATH_CHARS}+{DOTTED_PATH_SPACE_SEGMENTS}{NO_TRAILING_COLON}{TRAILING_SPACES_AT_EOL}|{NON_DOTTED_PATH_LOOKAHEAD}{PATH_CHARS}+{ANY_PATH_SPACE_SEGMENTS}{NO_TRAILING_COLON}{TRAILING_SPACES_AT_EOL})"
    )
}

fn bare_relative_path_branch() -> String {
    format!(
        "{DOTTED_PATH_LOOKAHEAD}{BARE_RELATIVE_PATH_PREFIX}{PATH_CHARS}+{NO_TRAILING_COLON}{TRAILING_SPACES_AT_EOL}"
    )
}

pub(crate) fn classify(text: &str) -> LinkTarget {
    if is_scheme_url(text) {
        LinkTarget::Url(text.to_owned())
    } else {
        LinkTarget::Path(text.to_owned())
    }
}

fn is_scheme_url(text: &str) -> bool {
    let Some(colon) = text.find(':') else {
        return false;
    };
    let scheme = &text[..colon].to_ascii_lowercase();
    matches!(
        scheme.as_str(),
        "http"
            | "https"
            | "mailto"
            | "ftp"
            | "file"
            | "ssh"
            | "git"
            | "tel"
            | "magnet"
            | "ipfs"
            | "ipns"
            | "gemini"
            | "gopher"
            | "news"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn first(input: &str) -> Option<String> {
        let re = compile(true, true).unwrap().unwrap();
        re.find(input)
            .unwrap()
            .map(|m| input[m.start()..m.end()].to_owned())
    }

    #[test]
    fn matches_representative_ghostty_cases() {
        for (input, want) in [
            ("hello https://example.com world", "https://example.com"),
            (
                "Link period https://example.com. More text.",
                "https://example.com",
            ),
            ("match file://example.com file links", "file://example.com"),
            ("/tmp/test.txt http://www.google.com", "/tmp/test.txt"),
            ("../example.py", "../example.py"),
            ("modified:   src/config/url.zig", "src/config/url.zig"),
            ("dotfiles/shell/.hushlogin", "dotfiles/shell/.hushlogin"),
            ("../test folder/file.txt", "../test folder/file.txt"),
            ("./.config/ghostty: Needs upstream", "./.config/ghostty"),
        ] {
            assert_eq!(first(input).as_deref(), Some(want), "input={input}");
        }
    }

    #[test]
    fn rejects_representative_ghostty_no_match_cases() {
        for input in [
            "input/output",
            "foo/bar",
            "$10/bar.txt",
            "foo/bar,baz.txt",
            ".hushlogin",
        ] {
            assert_eq!(first(input), None, "input={input}");
        }
    }

    #[test]
    fn classifies_urls_and_paths() {
        assert!(matches!(
            classify("https://example.com"),
            LinkTarget::Url(_)
        ));
        assert!(matches!(
            classify("dotfiles/shell/.hushlogin"),
            LinkTarget::Path(_)
        ));
    }
}
