use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::SearchFailed;
use crate::install_target;

const BASE_PUBLIC_URL: &str = "https://www.skills.sh/api/search";
const BASE_AUTH_URL: &str = "https://skills.sh/api/v1/skills/search";
const MAX_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SkillSearchResult {
    pub id: String,
    pub name: String,
    pub source: String,
    pub skill: String,
    pub repo: Option<String>,
    pub installs: u64,
    pub url: Option<String>,
    pub install_target: Option<String>,
    pub installable: bool,
}

#[cfg(not(tarpaulin_include))]
pub fn search<F>(
    query: &str,
    limit: usize,
    mut notify: F,
) -> Result<Vec<SkillSearchResult>, Box<dyn std::error::Error>>
where
    F: FnMut(String),
{
    let api_key = std::env::var("KTESIO_SKILLS_SH_API_KEY")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let transport = UreqTransport::new();
    let sleeper = ThreadSleeper;
    search_with_transport(
        &transport,
        &sleeper,
        query,
        limit,
        api_key.as_deref(),
        &mut notify,
    )
}

#[cfg(tarpaulin_include)]
pub fn search<F>(
    _query: &str,
    _limit: usize,
    _notify: F,
) -> Result<Vec<SkillSearchResult>, Box<dyn std::error::Error>>
where
    F: FnMut(String),
{
    Err(SearchFailed {
        message: "Live skills.sh search is disabled during coverage runs".to_string(),
    }
    .into())
}

fn search_with_transport<T, S, F>(
    transport: &T,
    sleeper: &S,
    query: &str,
    limit: usize,
    api_key: Option<&str>,
    notify: &mut F,
) -> Result<Vec<SkillSearchResult>, Box<dyn std::error::Error>>
where
    T: HttpTransport,
    S: Sleeper,
    F: FnMut(String),
{
    let query = query.trim();
    if query.len() < 2 {
        return Err(SearchFailed {
            message: "Search query must be at least 2 characters".to_string(),
        }
        .into());
    }

    let authenticated = api_key.is_some();
    let limit = normalized_limit(limit, authenticated);
    let url = search_url(query, limit, authenticated);

    for attempt in 1..=MAX_ATTEMPTS {
        match transport.get(&url, api_key) {
            Ok(response) if response.status == 200 => {
                return parse_search_response(&response.body, authenticated);
            }
            Ok(response) if is_retryable_status(response.status) => {
                if attempt == MAX_ATTEMPTS {
                    return Err(retry_exhausted(response.status, &response.headers).into());
                }

                let delay = retry_delay(&response.headers, response.status, attempt);
                notify(retry_message(response.status, delay, attempt + 1));
                sleeper.sleep(delay);
            }
            Ok(response) => return Err(non_retryable_status(response, authenticated).into()),
            Err(error) if error.retryable => {
                if attempt == MAX_ATTEMPTS {
                    return Err(SearchFailed {
                        message: format!(
                            "skills.sh search could not be reached after 3 attempts. Retry later or configure KTESIO_SKILLS_SH_API_KEY when access is available. Last error: {}",
                            error.message
                        ),
                    }
                    .into());
                }

                let delay = backoff_delay(attempt);
                notify(format!(
                    "skills.sh search connection failed; retrying in {}s (attempt {}/{MAX_ATTEMPTS}).",
                    display_seconds(delay),
                    attempt + 1
                ));
                sleeper.sleep(delay);
            }
            Err(error) => {
                return Err(SearchFailed {
                    message: format!("skills.sh search failed: {}", error.message),
                }
                .into())
            }
        }
    }

    Err(SearchFailed {
        message: "skills.sh search failed unexpectedly".to_string(),
    }
    .into())
}

fn search_url(query: &str, limit: usize, authenticated: bool) -> String {
    let encoded = urlencoding::encode(query);
    if authenticated {
        format!("{BASE_AUTH_URL}?q={encoded}&limit={limit}")
    } else {
        format!("{BASE_PUBLIC_URL}?q={encoded}&limit={limit}")
    }
}

fn normalized_limit(limit: usize, authenticated: bool) -> usize {
    let limit = limit.max(1);
    if authenticated {
        limit.min(200)
    } else {
        limit.min(100)
    }
}

fn parse_search_response(
    body: &str,
    authenticated: bool,
) -> Result<Vec<SkillSearchResult>, Box<dyn std::error::Error>> {
    if authenticated {
        let response: V1SearchResponse = serde_json::from_str(body).map_err(|_| SearchFailed {
            message: "skills.sh returned an unexpected authenticated search response".to_string(),
        })?;
        return Ok(response
            .data
            .into_iter()
            .map(normalize_v1_skill)
            .collect::<Vec<_>>());
    }

    let response: PublicSearchResponse = serde_json::from_str(body).map_err(|_| SearchFailed {
        message: "skills.sh returned an unexpected public search response".to_string(),
    })?;
    Ok(response
        .skills
        .into_iter()
        .map(normalize_public_skill)
        .collect::<Vec<_>>())
}

fn normalize_public_skill(skill: PublicSkill) -> SkillSearchResult {
    let source = skill.source;
    let skill_slug = skill
        .skill_id
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            skill
                .id
                .rsplit('/')
                .next()
                .unwrap_or(&skill.name)
                .to_string()
        });
    let repo = install_target::github_repo_from_source(&source, false);
    let install_target = install_target::install_target_from_source(&source, &skill_slug);
    let installable = install_target.is_some();
    let url = Some(format!("https://skills.sh/{source}/{skill_slug}"));

    SkillSearchResult {
        id: skill.id,
        name: skill.name,
        source,
        skill: skill_slug,
        repo,
        installs: skill.installs,
        url,
        install_target,
        installable,
    }
}

fn normalize_v1_skill(skill: V1Skill) -> SkillSearchResult {
    let source = skill.source;
    let skill_slug = skill.slug;
    let is_github = skill
        .source_type
        .as_deref()
        .map(|source_type| source_type == "github")
        .unwrap_or_else(|| install_target::github_repo_from_source(&source, false).is_some());
    let repo = if is_github {
        install_target::github_repo_from_source(&source, false)
    } else {
        None
    };
    let install_target = if is_github {
        install_target::install_target_from_source(&source, &skill_slug)
    } else {
        None
    };
    let installable = install_target.is_some();

    SkillSearchResult {
        id: skill.id,
        name: skill.name,
        source,
        skill: skill_slug,
        repo,
        installs: skill.installs,
        url: skill.url,
        install_target,
        installable,
    }
}

fn is_retryable_status(status: u16) -> bool {
    matches!(status, 429 | 503)
}

fn retry_delay(headers: &HashMap<String, String>, status: u16, attempt: usize) -> Duration {
    if status == 429 {
        if let Some(seconds) = header_seconds(headers, "retry-after") {
            return Duration::from_secs(seconds.max(1));
        }
        if let Some(seconds) = header_seconds(headers, "x-ratelimit-reset") {
            return Duration::from_secs(seconds.max(1));
        }
    }

    backoff_delay(attempt)
}

fn header_seconds(headers: &HashMap<String, String>, name: &str) -> Option<u64> {
    headers
        .get(name)
        .and_then(|value| value.trim().parse::<u64>().ok())
}

fn backoff_delay(attempt: usize) -> Duration {
    let seconds = 1_u64
        .checked_shl((attempt.saturating_sub(1)) as u32)
        .unwrap_or(8);
    let jitter_ms = ((attempt as u64) * 137) % 250;
    Duration::from_secs(seconds.min(8)) + Duration::from_millis(jitter_ms)
}

fn retry_message(status: u16, delay: Duration, next_attempt: usize) -> String {
    match status {
        429 => format!(
            "skills.sh rate limit reached; retrying in {}s (attempt {next_attempt}/{MAX_ATTEMPTS}).",
            display_seconds(delay)
        ),
        503 => format!(
            "skills.sh is temporarily unavailable; retrying in {}s (attempt {next_attempt}/{MAX_ATTEMPTS}).",
            display_seconds(delay)
        ),
        _ => format!(
            "skills.sh search failed temporarily; retrying in {}s (attempt {next_attempt}/{MAX_ATTEMPTS}).",
            display_seconds(delay)
        ),
    }
}

fn display_seconds(duration: Duration) -> u64 {
    duration.as_secs().max(1)
}

fn retry_exhausted(status: u16, headers: &HashMap<String, String>) -> SearchFailed {
    let retry_hint = retry_delay(headers, status, MAX_ATTEMPTS);
    match status {
        429 => SearchFailed {
            message: format!(
                "skills.sh rate limit reached after 3 attempts. Please retry in about {}s, search less frequently, or configure KTESIO_SKILLS_SH_API_KEY when access is available.",
                display_seconds(retry_hint)
            ),
        },
        503 => SearchFailed {
            message:
                "skills.sh is temporarily unavailable after 3 attempts. Please retry later."
                    .to_string(),
        },
        _ => SearchFailed {
            message: "skills.sh search failed after 3 attempts. Please retry later.".to_string(),
        },
    }
}

fn non_retryable_status(response: HttpResponse, authenticated: bool) -> SearchFailed {
    match response.status {
        400 => SearchFailed {
            message: "skills.sh rejected the search query. Try a different query or lower limit."
                .to_string(),
        },
        401 if authenticated => SearchFailed {
            message:
                "skills.sh rejected KTESIO_SKILLS_SH_API_KEY. Check the API key or unset it to use public search."
                    .to_string(),
        },
        401 => SearchFailed {
            message:
                "skills.sh search now requires authentication. Configure KTESIO_SKILLS_SH_API_KEY after receiving API access."
                    .to_string(),
        },
        403 => SearchFailed {
            message:
                "skills.sh search access was denied. Retry later or configure KTESIO_SKILLS_SH_API_KEY when access is available."
                    .to_string(),
        },
        404 => SearchFailed {
            message: "skills.sh search endpoint was not found. Please update Ktesio.".to_string(),
        },
        status => SearchFailed {
            message: format!("skills.sh search failed with HTTP status {status}."),
        },
    }
}

#[derive(Debug, Clone)]
struct HttpResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: String,
}

#[derive(Debug, Clone)]
struct HttpTransportError {
    message: String,
    retryable: bool,
}

trait HttpTransport {
    fn get(&self, url: &str, api_key: Option<&str>) -> Result<HttpResponse, HttpTransportError>;
}

trait Sleeper {
    fn sleep(&self, duration: Duration);
}

#[cfg(not(tarpaulin_include))]
struct ThreadSleeper;

#[cfg(not(tarpaulin_include))]
impl Sleeper for ThreadSleeper {
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

#[cfg(not(tarpaulin_include))]
struct UreqTransport {
    agent: ureq::Agent,
}

#[cfg(not(tarpaulin_include))]
impl UreqTransport {
    fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(15)))
            .http_status_as_error(false)
            .build();
        Self {
            agent: config.into(),
        }
    }
}

#[cfg(not(tarpaulin_include))]
impl HttpTransport for UreqTransport {
    fn get(&self, url: &str, api_key: Option<&str>) -> Result<HttpResponse, HttpTransportError> {
        let mut request = self
            .agent
            .get(url)
            .header("Accept", "application/json")
            .header("User-Agent", concat!("ktesio/", env!("CARGO_PKG_VERSION")));

        let auth_header;
        if let Some(api_key) = api_key {
            auth_header = format!("Bearer {api_key}");
            request = request.header("Authorization", &auth_header);
        }

        match request.call() {
            Ok(mut response) => {
                let status = response.status().as_u16();
                let headers = ["Retry-After", "X-RateLimit-Reset"]
                    .into_iter()
                    .filter_map(|name| {
                        response
                            .headers()
                            .get(name)
                            .and_then(|value| value.to_str().ok())
                            .map(|value| (name.to_ascii_lowercase(), value.to_string()))
                    })
                    .collect();
                let body =
                    response
                        .body_mut()
                        .read_to_string()
                        .map_err(|error| HttpTransportError {
                            message: error.to_string(),
                            retryable: false,
                        })?;

                Ok(HttpResponse {
                    status,
                    headers,
                    body,
                })
            }
            Err(error) => Err(HttpTransportError {
                message: friendly_transport_error(&error.to_string()),
                retryable: true,
            }),
        }
    }
}

fn friendly_transport_error(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    if lower.contains("dns") || lower.contains("resolve") || lower.contains("lookup") {
        "DNS lookup failed".to_string()
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "connection timed out".to_string()
    } else if lower.contains("connection refused") {
        "connection refused".to_string()
    } else {
        "temporary network error".to_string()
    }
}

#[derive(Debug, Deserialize)]
struct PublicSearchResponse {
    skills: Vec<PublicSkill>,
}

#[derive(Debug, Deserialize)]
struct PublicSkill {
    id: String,
    #[serde(rename = "skillId")]
    skill_id: Option<String>,
    name: String,
    installs: u64,
    source: String,
}

#[derive(Debug, Deserialize)]
struct V1SearchResponse {
    data: Vec<V1Skill>,
}

#[derive(Debug, Deserialize)]
struct V1Skill {
    id: String,
    slug: String,
    name: String,
    source: String,
    installs: u64,
    #[serde(rename = "sourceType")]
    source_type: Option<String>,
    url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    struct FakeTransport {
        responses: RefCell<Vec<Result<HttpResponse, HttpTransportError>>>,
        attempts: Cell<usize>,
        urls: RefCell<Vec<String>>,
    }

    impl FakeTransport {
        fn new(responses: Vec<Result<HttpResponse, HttpTransportError>>) -> Self {
            Self {
                responses: RefCell::new(responses),
                attempts: Cell::new(0),
                urls: RefCell::new(Vec::new()),
            }
        }
    }

    impl HttpTransport for FakeTransport {
        fn get(
            &self,
            url: &str,
            _api_key: Option<&str>,
        ) -> Result<HttpResponse, HttpTransportError> {
            self.attempts.set(self.attempts.get() + 1);
            self.urls.borrow_mut().push(url.to_string());
            self.responses.borrow_mut().remove(0)
        }
    }

    struct FakeSleeper {
        delays: RefCell<Vec<Duration>>,
    }

    impl FakeSleeper {
        fn new() -> Self {
            Self {
                delays: RefCell::new(Vec::new()),
            }
        }
    }

    impl Sleeper for FakeSleeper {
        fn sleep(&self, duration: Duration) {
            self.delays.borrow_mut().push(duration);
        }
    }

    fn response(status: u16, body: &str) -> HttpResponse {
        HttpResponse {
            status,
            headers: HashMap::new(),
            body: body.to_string(),
        }
    }

    fn response_with_header(status: u16, name: &str, value: &str) -> HttpResponse {
        let mut headers = HashMap::new();
        headers.insert(name.to_string(), value.to_string());
        HttpResponse {
            status,
            headers,
            body: "{}".to_string(),
        }
    }

    #[test]
    fn test_public_search_normalizes_github_results() {
        let body = r#"{
            "query": "tests",
            "searchType": "fuzzy",
            "skills": [{
                "id": "hashicorp/agent-skills/run-acceptance-tests",
                "skillId": "run-acceptance-tests",
                "name": "run-acceptance-tests",
                "installs": 1468,
                "source": "hashicorp/agent-skills"
            }],
            "count": 1
        }"#;
        let transport = FakeTransport::new(vec![Ok(response(200, body))]);
        let sleeper = FakeSleeper::new();
        let mut messages = Vec::new();

        let results =
            search_with_transport(&transport, &sleeper, "tests", 5, None, &mut |message| {
                messages.push(message)
            })
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].repo.as_deref(),
            Some("https://github.com/hashicorp/agent-skills.git")
        );
        assert_eq!(
            results[0].install_target.as_deref(),
            Some("hashicorp/agent-skills/run-acceptance-tests")
        );
        assert!(results[0].installable);
        assert!(messages.is_empty());
    }

    #[test]
    fn test_authenticated_search_uses_v1_shape_and_url() {
        let body = r#"{
            "data": [{
                "id": "expo/skills/react-native",
                "slug": "react-native",
                "name": "React Native",
                "source": "expo/skills",
                "installs": 3842,
                "sourceType": "github",
                "installUrl": "https://github.com/expo/skills",
                "url": "https://skills.sh/expo/skills/react-native"
            }],
            "query": "react native",
            "searchType": "semantic",
            "count": 1
        }"#;
        let transport = FakeTransport::new(vec![Ok(response(200, body))]);
        let sleeper = FakeSleeper::new();
        let mut messages = Vec::new();

        let results = search_with_transport(
            &transport,
            &sleeper,
            "react native",
            500,
            Some("sk_live_test"),
            &mut |message| messages.push(message),
        )
        .unwrap();

        assert_eq!(
            results[0].install_target.as_deref(),
            Some("expo/skills/react-native")
        );
        assert!(transport.urls.borrow()[0].starts_with(BASE_AUTH_URL));
        assert!(transport.urls.borrow()[0].contains("limit=200"));
    }

    #[test]
    fn test_rate_limit_retries_three_total_attempts_with_retry_after() {
        let success = r#"{"skills": [], "count": 0}"#;
        let transport = FakeTransport::new(vec![
            Ok(response_with_header(429, "retry-after", "3")),
            Ok(response_with_header(429, "x-ratelimit-reset", "4")),
            Ok(response(200, success)),
        ]);
        let sleeper = FakeSleeper::new();
        let mut messages = Vec::new();

        let results =
            search_with_transport(&transport, &sleeper, "tests", 10, None, &mut |message| {
                messages.push(message)
            })
            .unwrap();

        assert!(results.is_empty());
        assert_eq!(transport.attempts.get(), 3);
        assert_eq!(sleeper.delays.borrow()[0], Duration::from_secs(3));
        assert_eq!(sleeper.delays.borrow()[1], Duration::from_secs(4));
        assert!(messages[0].contains("rate limit reached"));
    }

    #[test]
    fn test_retryable_transport_error_retries() {
        let success = r#"{"skills": [], "count": 0}"#;
        let transport = FakeTransport::new(vec![
            Err(HttpTransportError {
                message: "timeout".to_string(),
                retryable: true,
            }),
            Ok(response(200, success)),
        ]);
        let sleeper = FakeSleeper::new();
        let mut messages = Vec::new();

        let results =
            search_with_transport(&transport, &sleeper, "tests", 10, None, &mut |message| {
                messages.push(message)
            })
            .unwrap();

        assert!(results.is_empty());
        assert_eq!(transport.attempts.get(), 2);
        assert_eq!(sleeper.delays.borrow().len(), 1);
        assert!(messages[0].contains("connection failed"));
    }

    #[test]
    fn test_non_retryable_status_fails_once() {
        let transport = FakeTransport::new(vec![Ok(response(401, "{}"))]);
        let sleeper = FakeSleeper::new();
        let mut messages = Vec::new();

        let result = search_with_transport(
            &transport,
            &sleeper,
            "tests",
            10,
            Some("bad"),
            &mut |message| messages.push(message),
        );

        assert!(result.is_err());
        assert_eq!(transport.attempts.get(), 1);
        assert!(sleeper.delays.borrow().is_empty());
        assert!(messages.is_empty());
    }

    #[test]
    fn test_malformed_success_response_does_not_retry() {
        let transport = FakeTransport::new(vec![Ok(response(200, "not json"))]);
        let sleeper = FakeSleeper::new();
        let mut messages = Vec::new();

        let result =
            search_with_transport(&transport, &sleeper, "tests", 10, None, &mut |message| {
                messages.push(message)
            });

        assert!(result.is_err());
        assert_eq!(transport.attempts.get(), 1);
    }

    #[test]
    fn test_short_query_fails_without_request() {
        let transport = FakeTransport::new(Vec::new());
        let sleeper = FakeSleeper::new();
        let mut messages = Vec::new();

        let result = search_with_transport(&transport, &sleeper, " x ", 10, None, &mut |message| {
            messages.push(message)
        });

        assert!(result.is_err());
        assert_eq!(transport.attempts.get(), 0);
        assert!(messages.is_empty());
    }

    #[test]
    fn test_public_search_falls_back_to_id_slug() {
        let body = r#"{
            "skills": [{
                "id": "catalog/vendors/write-tests",
                "skillId": null,
                "name": "Write Tests",
                "installs": 9,
                "source": "catalog"
            }]
        }"#;
        let transport = FakeTransport::new(vec![Ok(response(200, body))]);
        let sleeper = FakeSleeper::new();
        let mut messages = Vec::new();

        let results =
            search_with_transport(&transport, &sleeper, "tests", 0, None, &mut |message| {
                messages.push(message)
            })
            .unwrap();

        assert_eq!(results[0].skill, "write-tests");
        assert_eq!(results[0].repo, None);
        assert_eq!(results[0].install_target, None);
        assert!(!results[0].installable);
        assert!(transport.urls.borrow()[0].contains("limit=1"));
    }

    #[test]
    fn test_public_search_caps_large_limit() {
        let transport = FakeTransport::new(vec![Ok(response(200, r#"{"skills": []}"#))]);
        let sleeper = FakeSleeper::new();
        let mut messages = Vec::new();

        search_with_transport(&transport, &sleeper, "tests", 500, None, &mut |message| {
            messages.push(message)
        })
        .unwrap();

        assert!(transport.urls.borrow()[0].contains("limit=100"));
    }

    #[test]
    fn test_authenticated_nongithub_result_is_not_installable() {
        let body = r#"{
            "data": [{
                "id": "external/write-tests",
                "slug": "write-tests",
                "name": "Write Tests",
                "source": "external/package",
                "installs": 3,
                "sourceType": "registry",
                "url": null
            }]
        }"#;
        let transport = FakeTransport::new(vec![Ok(response(200, body))]);
        let sleeper = FakeSleeper::new();
        let mut messages = Vec::new();

        let results = search_with_transport(
            &transport,
            &sleeper,
            "tests",
            10,
            Some("key"),
            &mut |message| messages.push(message),
        )
        .unwrap();

        assert_eq!(results[0].repo, None);
        assert_eq!(results[0].install_target, None);
        assert!(!results[0].installable);
    }

    #[test]
    fn test_malformed_authenticated_response_does_not_retry() {
        let transport = FakeTransport::new(vec![Ok(response(200, "not json"))]);
        let sleeper = FakeSleeper::new();
        let mut messages = Vec::new();

        let result = search_with_transport(
            &transport,
            &sleeper,
            "tests",
            10,
            Some("key"),
            &mut |message| messages.push(message),
        );

        assert!(result.is_err());
        assert_eq!(transport.attempts.get(), 1);
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("authenticated search response"));
    }

    #[test]
    fn test_service_unavailable_exhaustion_uses_clean_error() {
        let transport = FakeTransport::new(vec![
            Ok(response(503, "{}")),
            Ok(response(503, "{}")),
            Ok(response(503, "{}")),
        ]);
        let sleeper = FakeSleeper::new();
        let mut messages = Vec::new();

        let result =
            search_with_transport(&transport, &sleeper, "tests", 10, None, &mut |message| {
                messages.push(message)
            });

        assert!(result.is_err());
        assert_eq!(transport.attempts.get(), 3);
        assert_eq!(sleeper.delays.borrow().len(), 2);
        assert!(messages[0].contains("temporarily unavailable"));
        assert!(result.unwrap_err().to_string().contains("retry later"));
    }

    #[test]
    fn test_retryable_transport_error_exhaustion_mentions_last_error() {
        let transport = FakeTransport::new(vec![
            Err(HttpTransportError {
                message: "timeout".to_string(),
                retryable: true,
            }),
            Err(HttpTransportError {
                message: "timeout".to_string(),
                retryable: true,
            }),
            Err(HttpTransportError {
                message: "connection refused".to_string(),
                retryable: true,
            }),
        ]);
        let sleeper = FakeSleeper::new();
        let mut messages = Vec::new();

        let result =
            search_with_transport(&transport, &sleeper, "tests", 10, None, &mut |message| {
                messages.push(message)
            });

        assert!(result.is_err());
        assert_eq!(transport.attempts.get(), 3);
        assert_eq!(messages.len(), 2);
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("connection refused"));
    }

    #[test]
    fn test_non_retryable_transport_error_fails_once() {
        let transport = FakeTransport::new(vec![Err(HttpTransportError {
            message: "bad response body".to_string(),
            retryable: false,
        })]);
        let sleeper = FakeSleeper::new();
        let mut messages = Vec::new();

        let result =
            search_with_transport(&transport, &sleeper, "tests", 10, None, &mut |message| {
                messages.push(message)
            });

        assert!(result.is_err());
        assert_eq!(transport.attempts.get(), 1);
        assert!(sleeper.delays.borrow().is_empty());
        assert!(messages.is_empty());
    }

    #[test]
    fn test_retry_delay_and_messages_cover_fallbacks() {
        let mut headers = HashMap::new();
        headers.insert("retry-after".to_string(), "not-a-number".to_string());

        let delay = retry_delay(&headers, 429, 2);

        assert!(delay >= Duration::from_secs(2));
        assert!(retry_message(429, Duration::from_secs(12), 2).contains("rate limit"));
        assert!(retry_message(503, Duration::from_secs(2), 3).contains("temporarily unavailable"));
        assert!(retry_message(500, Duration::from_secs(2), 3).contains("failed temporarily"));
        assert_eq!(display_seconds(Duration::from_millis(5)), 1);
    }

    #[test]
    fn test_retry_exhausted_status_messages() {
        let mut headers = HashMap::new();
        headers.insert("retry-after".to_string(), "7".to_string());

        assert!(retry_exhausted(429, &headers)
            .to_string()
            .contains("retry in about 7s"));
        assert!(retry_exhausted(503, &HashMap::new())
            .to_string()
            .contains("temporarily unavailable"));
        assert!(retry_exhausted(500, &HashMap::new())
            .to_string()
            .contains("retry later"));
    }

    #[test]
    fn test_non_retryable_status_messages() {
        let statuses = [
            (400, false, "rejected the search query"),
            (401, true, "rejected KTESIO_SKILLS_SH_API_KEY"),
            (401, false, "requires authentication"),
            (403, false, "access was denied"),
            (404, false, "endpoint was not found"),
            (418, false, "HTTP status 418"),
        ];

        for (status, authenticated, expected) in statuses {
            let error = non_retryable_status(response(status, "{}"), authenticated);
            assert!(
                error.to_string().contains(expected),
                "{} did not contain {}",
                error,
                expected
            );
        }
    }

    #[test]
    fn test_friendly_transport_error_variants() {
        assert_eq!(
            friendly_transport_error("DNS lookup failed"),
            "DNS lookup failed"
        );
        assert_eq!(
            friendly_transport_error("operation timed out"),
            "connection timed out"
        );
        assert_eq!(
            friendly_transport_error("connection refused by peer"),
            "connection refused"
        );
        assert_eq!(
            friendly_transport_error("unexpected reset"),
            "temporary network error"
        );
    }

    #[cfg(tarpaulin_include)]
    #[test]
    fn test_live_search_is_disabled_for_coverage() {
        let result = search("tests", 10, |_| {});

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("disabled during coverage runs"));
    }
}
