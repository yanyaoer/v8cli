use cookie_scoop::{BrowserName, Cookie, GetCookiesOptions};
use std::path::{Path, PathBuf};
use url::Url;

#[derive(Clone, Copy)]
pub enum Browser {
    Safari,
    Chrome,
}

pub struct BrowserImport {
    pub user_agent: String,
    pub cookies: Vec<String>,
    pub warnings: Vec<String>,
}

pub async fn import_browser(browser: Browser, target: &Url) -> Result<BrowserImport, String> {
    if !matches!(target.scheme(), "http" | "https") || target.host_str().is_none() {
        return Err("browser cookies require an HTTP(S) URL with a host".into());
    }

    let user_agent = browser_user_agent(browser)?;
    let (browser_name, profile) = match browser {
        Browser::Safari => (BrowserName::Safari, None),
        Browser::Chrome => (BrowserName::Chrome, chrome_profile()),
    };
    let mut options = GetCookiesOptions::new(target.as_str()).browsers(vec![browser_name]);
    if let Some(profile) = &profile {
        options = options.chrome_profile(profile);
    }

    let result = cookie_scoop::get_cookies(options).await;
    let cookies: Vec<String> = result
        .cookies
        .iter()
        .filter_map(|cookie| scoped_cookie(cookie, target))
        .collect();
    let mut warnings = result.warnings;
    if cookies.is_empty() {
        warnings.push(format!(
            "no matching cookies were imported from {} for {}",
            browser_name,
            target.host_str().unwrap()
        ));
    }

    Ok(BrowserImport {
        user_agent,
        cookies,
        warnings,
    })
}

fn scoped_cookie(cookie: &Cookie, target: &Url) -> Option<String> {
    if !valid_cookie_name(&cookie.name) || !valid_cookie_value(&cookie.value) {
        return None;
    }
    let host = target.host_str()?.to_ascii_lowercase();
    let domain = cookie
        .domain
        .as_deref()
        .map(|domain| domain.trim_start_matches('.').to_ascii_lowercase())
        .filter(|domain| !domain.is_empty());
    let parent_domain = match domain {
        Some(domain) if domain == host => None,
        Some(domain) if valid_cookie_domain(&domain) && host.ends_with(&format!(".{domain}")) => {
            Some(domain)
        }
        Some(_) => return None,
        None => None,
    };
    let path = cookie.path.as_deref().unwrap_or("/");
    let path = if path.starts_with('/')
        && !path.contains(';')
        && !path.contains('\r')
        && !path.contains('\n')
    {
        path
    } else {
        "/"
    };
    let mut value = format!("{}={}", cookie.name, cookie.value);
    if let Some(domain) = parent_domain {
        value.push_str(&format!("; Domain={domain}"));
    }
    value.push_str(&format!("; Path={path}"));
    if cookie.secure.unwrap_or(false) {
        value.push_str("; Secure");
    }
    if cookie.http_only.unwrap_or(false) {
        value.push_str("; HttpOnly");
    }
    Some(value)
}

fn valid_cookie_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| {
            b > 0x20
                && b < 0x7f
                && !matches!(
                    b,
                    b'(' | b')'
                        | b'<'
                        | b'>'
                        | b'@'
                        | b','
                        | b';'
                        | b':'
                        | b'\\'
                        | b'"'
                        | b'/'
                        | b'['
                        | b']'
                        | b'?'
                        | b'='
                        | b'{'
                        | b'}'
                )
        })
}

fn valid_cookie_value(value: &str) -> bool {
    value.bytes().all(|b| {
        b == 0x21
            || (0x23..=0x2b).contains(&b)
            || (0x2d..=0x3a).contains(&b)
            || (0x3c..=0x5b).contains(&b)
            || (0x5d..=0x7e).contains(&b)
    })
}

fn valid_cookie_domain(domain: &str) -> bool {
    !domain.is_empty()
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains("..")
        && domain
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

fn browser_user_agent(browser: Browser) -> Result<String, String> {
    match browser {
        Browser::Chrome => chrome_version()
            .and_then(|version| chrome_user_agent(&version))
            .ok_or_else(|| "cannot determine the installed Chrome version".to_string()),
        Browser::Safari => safari_version()
            .and_then(|version| safari_user_agent(&version))
            .ok_or_else(|| "cannot determine the installed Safari version".to_string()),
    }
}

fn chrome_user_agent(version: &str) -> Option<String> {
    let major = version.trim().split('.').next()?;
    if major.is_empty() || !major.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(format!(
        "Mozilla/5.0 ({}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36",
        platform_user_agent_token()
    ))
}

fn safari_user_agent(version: &str) -> Option<String> {
    let version = version.trim();
    if version.is_empty() || !version.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    Some(format!(
        "Mozilla/5.0 ({}) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/{version} Safari/605.1.15",
        platform_user_agent_token()
    ))
}

fn platform_user_agent_token() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "Macintosh; Intel Mac OS X 10_15_7"
    }
    #[cfg(target_os = "windows")]
    {
        "Windows NT 10.0; Win64; x64"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "X11; Linux x86_64"
    }
}

fn chrome_profile() -> Option<String> {
    let root = chrome_data_root()?;
    let state = std::fs::read_to_string(root.join("Local State")).ok()?;
    let state: serde_json::Value = serde_json::from_str(&state).ok()?;
    state["profile"]["last_used"]
        .as_str()
        .filter(|s| valid_profile_name(s))
        .or_else(|| {
            state["profile"]["last_active_profiles"]
                .as_array()?
                .iter()
                .find_map(|p| p.as_str().filter(|s| valid_profile_name(s)))
        })
        .map(String::from)
}

fn valid_profile_name(profile: &str) -> bool {
    !profile.is_empty()
        && profile != "."
        && profile != ".."
        && !profile.contains('/')
        && !profile.contains('\\')
}

fn chrome_version() -> Option<String> {
    chrome_data_root()
        .and_then(|root| read_version(root.join("Last Version")))
        .or_else(|| {
            #[cfg(target_os = "macos")]
            {
                plist_version(Path::new(
                    "/Applications/Google Chrome.app/Contents/Info.plist",
                ))
            }
            #[cfg(not(target_os = "macos"))]
            {
                None
            }
        })
}

fn chrome_data_root() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        home_dir().map(|h| h.join("Library/Application Support/Google/Chrome"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|p| p.join("Google/Chrome/User Data"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|h| h.join(".config")))?;
        Some(base.join("google-chrome"))
    }
}

#[cfg(target_os = "macos")]
fn safari_version() -> Option<String> {
    plist_version(Path::new("/Applications/Safari.app/Contents/Info.plist"))
}

#[cfg(not(target_os = "macos"))]
fn safari_version() -> Option<String> {
    None
}

fn plist_version(path: &Path) -> Option<String> {
    let body = std::fs::read_to_string(path).ok()?;
    let after_key = body.split("<key>CFBundleShortVersionString</key>").nth(1)?;
    let start = after_key.find("<string>")? + "<string>".len();
    let end = after_key[start..].find("</string>")?;
    Some(after_key[start..start + end].trim().to_string())
}

fn read_version(path: PathBuf) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(path: &str, secure: bool) -> Cookie {
        Cookie {
            name: "session".into(),
            value: "secret".into(),
            domain: Some("example.com".into()),
            path: Some(path.into()),
            url: None,
            expires: None,
            secure: Some(secure),
            http_only: Some(true),
            same_site: None,
            source: None,
        }
    }

    #[test]
    fn browser_user_agents_use_installed_version_shape() {
        let chrome = chrome_user_agent("150.0.7871.187").unwrap();
        assert!(chrome.contains("Chrome/150.0.0.0"));
        assert!(!chrome.contains("7871"));

        let safari = safari_user_agent("26.5.2").unwrap();
        assert!(safari.contains("Version/26.5.2 Safari/605.1.15"));
    }

    #[test]
    fn imported_cookie_preserves_safe_parent_domain_and_flags() {
        let target = Url::parse("https://www.example.com/account").unwrap();
        let value = scoped_cookie(&cookie("/account", true), &target).unwrap();
        assert_eq!(
            value,
            "session=secret; Domain=example.com; Path=/account; Secure; HttpOnly"
        );
    }

    #[test]
    fn exact_host_cookie_stays_host_only() {
        let target = Url::parse("https://example.com/account").unwrap();
        let value = scoped_cookie(&cookie("/", false), &target).unwrap();
        assert_eq!(value, "session=secret; Path=/; HttpOnly");
    }

    #[test]
    fn cookie_for_unrelated_domain_is_rejected() {
        let target = Url::parse("https://example.com/").unwrap();
        let mut cookie = cookie("/", false);
        cookie.domain = Some("notexample.com".into());
        assert!(scoped_cookie(&cookie, &target).is_none());
    }
}
