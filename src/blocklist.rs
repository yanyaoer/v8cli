use adblock::lists::{FilterSet, ParseOptions};
use adblock::request::Request;
use adblock::Engine;

const DEFAULT_RULES: &str = include_str!("blocklist.txt");

/// Adblock Plus-syntax matcher for requests a page makes. Built once per
/// session: matching is a few microseconds, building is tens of milliseconds.
pub struct Blocklist {
    engine: Engine,
}

impl Blocklist {
    /// `extra` are paths to additional filter lists (EasyPrivacy, Peter Lowe,
    /// uBO). Passing `use_default = false` drops the built-in rules so a caller
    /// can rely purely on its own lists.
    pub fn new(extra: &[String], use_default: bool) -> Result<Self, String> {
        let mut set = FilterSet::new(false);
        if use_default {
            set.add_filter_list(DEFAULT_RULES.to_string(), ParseOptions::default());
        }
        for path in extra {
            let rules = std::fs::read_to_string(path)
                .map_err(|e| format!("cannot read filter list {path}: {e}"))?;
            set.add_filter_list(rules, ParseOptions::default());
        }
        Ok(Self {
            engine: Engine::new_with_filter_set(set),
        })
    }

    /// `resource_type` follows the Adblock Plus vocabulary: "script", "xhr",
    /// "image", "sub_frame", ... An unparseable URL is never blocked, so a
    /// malformed request still reaches the network layer's own validation.
    pub fn blocks(&self, url: &str, page_url: &str, resource_type: &str) -> bool {
        let Ok(request) = Request::new(url, page_url, resource_type, "GET") else {
            return false;
        };
        let result = self.engine.check_network_request(&request);
        result.filter.is_some() && result.exception.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_list() -> Blocklist {
        Blocklist::new(&[], true).unwrap()
    }

    #[test]
    fn blocks_third_party_ad_and_tracker_entry_points() {
        let list = default_list();
        let page = "https://www.theverge.com/";
        for url in [
            "https://www.googletagservices.com/tag/js/gpt.js",
            "https://client.aps.amazon-adsystem.com/publisher.js",
            "https://pub.doubleverify.com/dvtag/21236410/DV464041/pub.js",
            "https://www.google-analytics.com/analytics.js",
            "https://connect.facebook.net/en_US/fbevents.js",
            "https://cdn.cookielaw.org/scripttemplates/otSDKStub.js",
        ] {
            assert!(list.blocks(url, page, "script"), "should block {url}");
        }
    }

    #[test]
    fn allows_first_party_application_bundles() {
        let list = default_list();
        for (url, page) in [
            (
                "https://github.githubassets.com/assets/behaviors-1b7c43389cca1102.js",
                "https://github.com/",
            ),
            (
                "https://www.theverge.com/_next/static/chunks/main-fc7a6b72b1ad075e.js",
                "https://www.theverge.com/",
            ),
            (
                "https://abs.twimg.com/responsive-web/client-web/main.abc.js",
                "https://x.com/",
            ),
        ] {
            assert!(!list.blocks(url, page, "script"), "should allow {url}");
        }
    }

    #[test]
    fn allows_code_cdns_and_captcha_that_can_carry_content() {
        let list = default_list();
        let page = "https://example.com/";
        for url in [
            "https://cdn.jsdelivr.net/npm/vue@3/dist/vue.global.js",
            "https://unpkg.com/react@18/umd/react.production.min.js",
            "https://cdnjs.cloudflare.com/ajax/libs/jquery/3.6.0/jquery.min.js",
            "https://www.google.com/recaptcha/api.js",
        ] {
            assert!(!list.blocks(url, page, "script"), "should allow {url}");
        }
    }

    #[test]
    fn blocks_tracker_xhr_beacons_not_just_scripts() {
        let list = default_list();
        assert!(list.blocks(
            "https://www.google-analytics.com/g/collect?v=2",
            "https://www.theverge.com/",
            "xhr",
        ));
    }

    #[test]
    fn malformed_url_is_not_blocked() {
        let list = default_list();
        assert!(!list.blocks("not a url", "https://example.com/", "script"));
    }

    #[test]
    fn extra_list_rules_are_applied_and_missing_file_is_an_error() {
        let dir = std::env::temp_dir().join("v8cli-blocklist-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("extra.txt");
        std::fs::write(&path, "||first-party-cdn.example^\n").unwrap();

        let extra = vec![path.to_string_lossy().into_owned()];
        let list = Blocklist::new(&extra, true).unwrap();
        let page = "https://example.com/";
        assert!(list.blocks("https://first-party-cdn.example/a.js", page, "script"));
        // built-in rules survive alongside the extra list
        assert!(list.blocks("https://www.google-analytics.com/analytics.js", page, "script"));

        assert!(Blocklist::new(&["/nonexistent/list.txt".to_string()], true).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_rules_can_be_disabled() {
        let list = Blocklist::new(&[], false).unwrap();
        assert!(!list.blocks(
            "https://www.google-analytics.com/analytics.js",
            "https://example.com/",
            "script",
        ));
    }
}
