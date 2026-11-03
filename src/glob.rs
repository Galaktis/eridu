//! A small glob matcher for path and author filters, in the spirit of `.gitignore`:
//! `*` and `?` stay within one path segment, `**` crosses segments, and a pattern without
//! a `/` matches any segment (`*.lock`, `node_modules`) while one with a `/` is anchored
//! at the repository root (`src/**/*.rs`, `vendor/`).

pub struct Glob {
    pattern: Vec<u8>,
    /// No `/` in the pattern: it is tried on each path segment instead of the whole path.
    segment: bool,
}

impl Glob {
    pub fn new(pattern: &str) -> Self {
        let mut pattern = pattern.trim().trim_start_matches('/').to_string();
        // `vendor/` means "everything under vendor".
        if pattern.ends_with('/') {
            pattern.push_str("**");
        }
        let segment = !pattern.contains('/');
        Self {
            pattern: pattern.into_bytes(),
            segment,
        }
    }

    pub fn matches(&self, path: &str) -> bool {
        if self.segment {
            path.split('/')
                .any(|part| matches(&self.pattern, part.as_bytes()))
        } else {
            matches(&self.pattern, path.as_bytes())
        }
    }
}

fn matches(pattern: &[u8], text: &[u8]) -> bool {
    match pattern {
        [] => text.is_empty(),
        [b'*', b'*', rest @ ..] => match rest.strip_prefix(b"/") {
            // `**/` matches zero or more whole directories.
            Some(rest) => (0..=text.len())
                .any(|i| (i == 0 || text[i - 1] == b'/') && matches(rest, &text[i..])),
            None => (0..=text.len()).any(|i| matches(rest, &text[i..])),
        },
        [b'*', rest @ ..] => (0..=text.len())
            .take_while(|&i| i == 0 || text[i - 1] != b'/')
            .any(|i| matches(rest, &text[i..])),
        [b'?', rest @ ..] => matches!(text, [c, tail @ ..] if *c != b'/' && matches(rest, tail)),
        [c, rest @ ..] => matches!(text, [d, tail @ ..] if d == c && matches(rest, tail)),
    }
}

#[cfg(test)]
mod tests {
    use super::Glob;

    fn check(pattern: &str, path: &str) -> bool {
        Glob::new(pattern).matches(path)
    }

    #[test]
    fn segment_patterns_match_at_any_depth() {
        assert!(check("*.lock", "Cargo.lock"));
        assert!(check("*.lock", "web/yarn.lock"));
        assert!(check("node_modules", "web/node_modules/react/index.js"));
        assert!(!check("*.lock", "Cargo.locks"));
        assert!(check("dependabot*", "dependabot[bot]"));
    }

    #[test]
    fn slash_patterns_are_anchored() {
        assert!(check("vendor/", "vendor/a/b.go"));
        assert!(!check("vendor/", "src/vendor/b.go"));
        assert!(check("src/**/*.rs", "src/main.rs"));
        assert!(check("src/**/*.rs", "src/a/b/c.rs"));
        assert!(!check("src/*.rs", "src/a/c.rs"));
        assert!(!check("src/**/b", "src/ab"));
        assert!(check("/docs/*.md", "docs/readme.md"));
        assert!(check("a/?.txt", "a/x.txt"));
        assert!(!check("a/?.txt", "a/xy.txt"));
    }

    #[test]
    fn segment_patterns_match_whole_segments() {
        assert!(!check("node_modules", "web/node_modules2/a.js"));
        assert!(!check("lib", "src/libs/a.rs"));
        assert!(check("lib", "lib"));
        assert!(check("*", "anything/at/all"));
        assert!(check("README*", "docs/README.md"));
        // Like `.gitignore`: a matching folder takes everything in it.
        assert!(check("*.rs", "src.rs/readme"));
        assert!(check("*.rs", "src.rs/main.rs"));
    }

    #[test]
    fn single_wildcards_stay_in_one_segment() {
        assert!(!check("src/*", "src/a/b.rs"));
        assert!(check("src/*", "src/a"));
        assert!(!check("a?b/c", "a/b/c"));
        assert!(check("src/*.rs", "src/.rs"), "`*` may match nothing");
        assert!(check("a/*b*/c", "a/xbx/c"));
        assert!(!check("a/*b*/c", "a/x/b/c"));
    }

    #[test]
    fn double_stars_cross_segments() {
        assert!(check("docs/**", "docs/a/b/c.md"));
        assert!(!check("docs/**", "docs"), "`docs/**` is what's under docs");
        assert!(check("**/test/*.rs", "test/a.rs"));
        assert!(check("**/test/*.rs", "x/y/test/a.rs"));
        assert!(!check("**/test/*.rs", "x/mytest/a.rs"));
        assert!(
            check("src/**.rs", "src/a/b.rs"),
            "`**` without `/` crosses segments too"
        );
        assert!(check("a/**/b/**/c", "a/b/c"));
        assert!(check("a/**/b/**/c", "a/x/b/y/z/c"));
    }

    #[test]
    fn patterns_are_trimmed_and_unanchored_by_a_leading_slash() {
        assert!(check("  *.lock \n", "yarn.lock"));
        assert!(check("/vendor/", "vendor/x"));
        assert!(!check("/vendor/", "lib/vendor/x"));
        assert!(
            !check("vendor/", "vendor"),
            "a folder pattern is not the file itself"
        );
    }

    #[test]
    fn matching_is_exact_and_case_sensitive() {
        assert!(!check("Makefile", "makefile"));
        assert!(!check("src/main.rs", "src/main.rs.bak"));
        assert!(!check("src/main.rs", "x/src/main.rs"));
        assert!(check("a.b", "a.b"));
        assert!(!check("a.b", "axb"), "`.` is not a wildcard");
    }

    #[test]
    fn authors_and_emails_match() {
        // Filters lowercase authors before matching, so patterns are lowercase too.
        assert!(check("*@bots.example.com", "ci@bots.example.com"));
        assert!(check("*[bot]", "renovate[bot]"));
        assert!(check("jane doe", "jane doe"));
        assert!(!check("jane", "jane doe"));
    }
}
