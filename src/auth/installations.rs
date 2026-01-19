//! auth::installations
//!
//! GitHub App installation and repository authorization queries.
//!
//! Per SPEC.md Section 8E.0.1, this module determines if the authenticated user
//! has access to a repository via an installed GitHub App.
//!
//! # Algorithm
//!
//! 1. Query `GET /user/installations` to list user's app installations
//! 2. For each installation, query `GET /user/installations/{id}/repositories`
//! 3. Continue until the repo is found or all installations are exhausted
//! 4. If found: return installation_id and repository_id
//! 5. If not found: return None (caller should generate AppNotInstalled error)
//!
//! # Security
//!
//! All API calls use bearer token authentication. Tokens are never logged
//! or included in error messages per SPEC.md Section 4.4.4.
//!
//! # Example
//!
//! ```ignore
//! use latticework::auth::{GitHubAuthManager, installations::check_repo_authorization};
//! use latticework::secrets;
//!
//! let store = secrets::create_store(secrets::DEFAULT_PROVIDER)?;
//! let auth_manager = GitHubAuthManager::new("github.com", store);
//!
//! match check_repo_authorization(&auth_manager, "github.com", "owner", "repo").await? {
//!     Some(result) => println!("Authorized: installation={}", result.installation_id),
//!     None => println!("App not installed for this repo"),
//! }
//! ```

use super::errors::AuthError;
use super::TokenProvider;
use serde::Deserialize;

/// Result of checking repository authorization.
///
/// Contains the IDs needed for API calls and caching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepoAuthResult {
    /// GitHub App installation ID for this organization/user
    pub installation_id: u64,
    /// Repository ID within the installation
    pub repository_id: u64,
}

/// Response from GET /user/installations
#[derive(Debug, Deserialize)]
struct InstallationsResponse {
    installations: Vec<Installation>,
}

/// A GitHub App installation
#[derive(Debug, Deserialize)]
struct Installation {
    id: u64,
    #[allow(dead_code)]
    account: InstallationAccount,
}

/// Account (user or org) that owns the installation
#[derive(Debug, Deserialize)]
struct InstallationAccount {
    #[allow(dead_code)]
    login: String,
}

/// Response from GET /user/installations/{id}/repositories
#[derive(Debug, Deserialize)]
struct RepositoriesResponse {
    repositories: Vec<Repository>,
}

/// A repository accessible through an installation
#[derive(Debug, Deserialize)]
struct Repository {
    id: u64,
    name: String,
    owner: RepositoryOwner,
}

/// Owner of a repository
#[derive(Debug, Deserialize)]
struct RepositoryOwner {
    login: String,
}

/// Check if the authenticated user has access to a repository via GitHub App.
///
/// Per SPEC.md 8E.0.1:
/// 1. GET /user/installations - list user's app installations
/// 2. GET /user/installations/{id}/repositories - check each installation
/// 3. Return installation_id and repository_id if found, None otherwise
///
/// # Arguments
///
/// * `token_provider` - Provider for bearer tokens
/// * `host` - GitHub host (e.g., "github.com")
/// * `owner` - Repository owner (user or org)
/// * `repo` - Repository name
///
/// # Returns
///
/// * `Ok(Some(RepoAuthResult))` - Repository is authorized
/// * `Ok(None)` - App not installed or repo not accessible
/// * `Err(AuthError)` - Network or authentication error
///
/// # Errors
///
/// * `AuthError::NotAuthenticated` - No valid token
/// * `AuthError::Network` - Network request failed
/// * `AuthError::GitHubApi` - GitHub API returned an error
pub async fn check_repo_authorization<T: TokenProvider>(
    token_provider: &T,
    host: &str,
    owner: &str,
    repo: &str,
) -> Result<Option<RepoAuthResult>, AuthError> {
    let token = token_provider.bearer_token().await?;
    let client = reqwest::Client::new();
    let base_url = api_base_url(host);

    // Step 1: Get user installations
    let installations = fetch_installations(&client, &base_url, &token).await?;

    // Step 2: For each installation, check repositories
    for installation in installations {
        if let Some(result) =
            find_repo_in_installation(&client, &base_url, &token, installation.id, owner, repo)
                .await?
        {
            return Ok(Some(result));
        }
    }

    // Not found in any installation
    Ok(None)
}

/// Get the API base URL for a GitHub host.
fn api_base_url(host: &str) -> String {
    if host == "github.com" {
        "https://api.github.com".to_string()
    } else {
        // GitHub Enterprise
        format!("https://{}/api/v3", host)
    }
}

/// Common headers for GitHub API requests.
fn github_headers(token: &str) -> Vec<(&'static str, String)> {
    vec![
        ("Authorization", format!("Bearer {}", token)),
        ("Accept", "application/vnd.github+json".to_string()),
        ("User-Agent", "lattice-cli".to_string()),
        ("X-GitHub-Api-Version", "2022-11-28".to_string()),
    ]
}

/// Fetch all installations for the authenticated user.
async fn fetch_installations(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
) -> Result<Vec<Installation>, AuthError> {
    let url = format!("{}/user/installations", base_url);
    let mut request = client.get(&url);

    for (key, value) in github_headers(token) {
        request = request.header(key, value);
    }

    let response = request.send().await?;
    let status = response.status();

    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(AuthError::NotAuthenticated(
            "token rejected by GitHub".to_string(),
        ));
    }

    if status == reqwest::StatusCode::FORBIDDEN {
        // This can happen if the token doesn't have the required scopes
        return Err(AuthError::GitHubApi {
            status: 403,
            message: "insufficient permissions to list installations".to_string(),
        });
    }

    if !status.is_success() {
        let message = response.text().await.unwrap_or_default();
        return Err(AuthError::GitHubApi {
            status: status.as_u16(),
            message,
        });
    }

    let data: InstallationsResponse = response.json().await?;
    Ok(data.installations)
}

/// Search for a repository within an installation, handling pagination.
async fn find_repo_in_installation(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    installation_id: u64,
    owner: &str,
    repo: &str,
) -> Result<Option<RepoAuthResult>, AuthError> {
    let mut page = 1u32;
    let per_page = 100;

    loop {
        let url = format!(
            "{}/user/installations/{}/repositories",
            base_url, installation_id
        );

        let mut request = client.get(&url).query(&[
            ("page", page.to_string()),
            ("per_page", per_page.to_string()),
        ]);

        for (key, value) in github_headers(token) {
            request = request.header(key, value);
        }

        let response = request.send().await?;

        // Skip this installation on non-success (e.g., installation was removed)
        if !response.status().is_success() {
            return Ok(None);
        }

        let data: RepositoriesResponse = response.json().await?;

        // Check if target repo is in this page
        for repository in &data.repositories {
            if repository.owner.login.eq_ignore_ascii_case(owner)
                && repository.name.eq_ignore_ascii_case(repo)
            {
                return Ok(Some(RepoAuthResult {
                    installation_id,
                    repository_id: repository.id,
                }));
            }
        }

        // Check if more pages
        if data.repositories.len() < per_page as usize {
            break;
        }
        page += 1;

        // Safety limit to prevent infinite loops
        if page > 100 {
            break;
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_base_url_github_com() {
        assert_eq!(api_base_url("github.com"), "https://api.github.com");
    }

    #[test]
    fn api_base_url_enterprise() {
        assert_eq!(
            api_base_url("github.example.com"),
            "https://github.example.com/api/v3"
        );
    }

    #[test]
    fn github_headers_includes_auth() {
        let headers = github_headers("test_token");

        // Find the Authorization header
        let auth_header = headers.iter().find(|(k, _)| *k == "Authorization");
        assert!(auth_header.is_some());
        assert_eq!(auth_header.unwrap().1, "Bearer test_token");
    }

    #[test]
    fn github_headers_includes_version() {
        let headers = github_headers("test_token");

        let version_header = headers.iter().find(|(k, _)| *k == "X-GitHub-Api-Version");
        assert!(version_header.is_some());
        assert_eq!(version_header.unwrap().1, "2022-11-28");
    }

    #[test]
    fn repo_auth_result_copy() {
        let result = RepoAuthResult {
            installation_id: 123,
            repository_id: 456,
        };
        let copied = result;
        assert_eq!(result, copied);
    }

    #[test]
    fn repo_auth_result_debug() {
        let result = RepoAuthResult {
            installation_id: 123,
            repository_id: 456,
        };
        let debug = format!("{:?}", result);
        assert!(debug.contains("123"));
        assert!(debug.contains("456"));
    }
}
