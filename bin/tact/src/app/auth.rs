//! Authentication selection and shared ChatGPT credential management.

use crate::app::{
    config::{AnthropicConfig, AuthConfig, AuthMode},
    error::{AuthError, AuthResult, SecretError},
    secret::SecretString,
};
use nanocodex::{
    claude::ClaudeClient,
    oai::auth::{
        ChatGptAuthStatus, ChatGptLogin, OpenAiAuth, load_chatgpt_auth, logout_chatgpt,
        resolve_chatgpt_auth_status,
    },
};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use std::{path::Path, result::Result as StdResult};

const OPENAI_API_KEY: &str = "OPENAI_API_KEY";
const ANTHROPIC_BASE_URL: &str = "ANTHROPIC_BASE_URL";
const ANTHROPIC_AUTH_TOKEN: &str = "ANTHROPIC_AUTH_TOKEN";
const ANTHROPIC_API_KEY: &str = "ANTHROPIC_API_KEY";
const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

/// Anthropic credentials, using the same environment variables as Claude Code.
enum AnthropicCredential {
    /// `ANTHROPIC_AUTH_TOKEN`, sent as a bearer token. Proxies such as Valet use this.
    AuthToken(SecretString),
    /// `ANTHROPIC_API_KEY`, sent as `x-api-key`.
    ApiKey(SecretString),
}

enum SelectedAuth {
    ChatGpt,
    ApiKey(SecretString),
}

impl AuthConfig {
    pub(crate) async fn login(&self) -> AuthResult<()> {
        let login = ChatGptLogin::start(self.file()).await?;

        eprintln!(
            "Open this URL to sign in with ChatGPT:\n\n{}\n",
            login.authorization_url()
        );
        if let Err(error) = crate::app::browser::open(login.authorization_url()).await {
            eprintln!(
                "Could not open a browser automatically ({error}). Open the URL above manually."
            );
        }

        let account = login.complete().await?;
        eprintln!("{}", self.login_success(&account));
        Ok(())
    }

    pub(crate) fn load(&self) -> AuthResult<OpenAiAuth> {
        let selected = self.select_auth(|| SecretString::from_environment(OPENAI_API_KEY))?;

        selected.into_openai_auth(self.file())
    }

    pub(crate) async fn status(&self) -> AuthResult<()> {
        match self.select_auth(|| SecretString::from_environment(OPENAI_API_KEY))? {
            SelectedAuth::ChatGpt => self.print_chatgpt_status().await?,
            SelectedAuth::ApiKey(_api_key) => {
                println!("Authentication: OpenAI API key");
                println!("Source: {OPENAI_API_KEY}");
            }
        }

        Ok(())
    }

    pub(crate) fn logout(&self) -> AuthResult<()> {
        if logout_chatgpt(self.file())? {
            eprintln!(
                "Removed shared ChatGPT credentials from {}. Tact and Codex are logged out.",
                self.file().display()
            );
            return Ok(());
        }

        eprintln!(
            "No ChatGPT credentials were stored at {}.",
            self.file().display()
        );
        Ok(())
    }

    fn select_auth<F>(&self, read_api_key: F) -> AuthResult<SelectedAuth>
    where
        F: FnOnce() -> StdResult<Option<SecretString>, SecretError>,
    {
        match self.mode() {
            AuthMode::ChatGpt => Ok(SelectedAuth::ChatGpt),
            AuthMode::ApiKey => read_api_key()?
                .map(SelectedAuth::ApiKey)
                .ok_or(AuthError::ApiKeyUnavailable),
            AuthMode::Auto => {
                if self
                    .file()
                    .try_exists()
                    .map_err(|source| AuthError::InspectCredentialFile {
                        path: self.file().to_path_buf(),
                        source,
                    })?
                {
                    return Ok(SelectedAuth::ChatGpt);
                }

                read_api_key()?.map(SelectedAuth::ApiKey).ok_or_else(|| {
                    AuthError::CredentialsUnavailable {
                        path: self.file().to_path_buf(),
                    }
                })
            }
        }
    }

    async fn print_chatgpt_status(&self) -> AuthResult<()> {
        let account = resolve_chatgpt_auth_status(self.file()).await?;
        println!("Authentication: ChatGPT");
        if let Some(email) = account.email {
            println!("Email: {email}");
        }
        if let Some(plan) = account.plan {
            println!("Plan: {plan}");
        }
        println!("Account: {}", account.account_id);
        println!("FedRAMP: {}", account.fedramp);
        println!("Credentials: {}", self.file().display());
        Ok(())
    }

    fn login_success(&self, account: &ChatGptAuthStatus) -> String {
        let identity = account
            .email
            .as_deref()
            .map_or(String::new(), |email| format!(" as {email}"));
        format!(
            "Tact and Codex are logged in{identity} (account {}). Credentials saved to {}.",
            account.account_id,
            self.file().display()
        )
    }
}

impl AnthropicConfig {
    /// Builds a Messages client from `[anthropic]` settings and the Anthropic environment.
    ///
    /// The base URL comes from `base_url`, then `ANTHROPIC_BASE_URL`, then Anthropic's API.
    pub(crate) fn client(&self) -> AuthResult<ClaudeClient> {
        let base_url = match self.base_url() {
            Some(base_url) => base_url.to_owned(),
            None => std::env::var(ANTHROPIC_BASE_URL)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_ANTHROPIC_BASE_URL.to_owned()),
        };
        let credential = AnthropicCredential::from_environment(
            || SecretString::from_environment(ANTHROPIC_AUTH_TOKEN),
            || SecretString::from_environment(ANTHROPIC_API_KEY),
        )?;
        credential.into_client(reqwest::Client::new(), messages_endpoint(&base_url))
    }
}

fn messages_endpoint(base_url: &str) -> String {
    format!("{}/v1/messages", base_url.trim_end_matches('/'))
}

impl AnthropicCredential {
    fn from_environment<T, K>(read_auth_token: T, read_api_key: K) -> AuthResult<Self>
    where
        T: FnOnce() -> StdResult<Option<SecretString>, SecretError>,
        K: FnOnce() -> StdResult<Option<SecretString>, SecretError>,
    {
        if let Some(token) = read_auth_token()? {
            return Ok(Self::AuthToken(token));
        }
        read_api_key()?
            .map(Self::ApiKey)
            .ok_or(AuthError::AnthropicCredentialsUnavailable)
    }

    // Nanocodex and reqwest retain non-zeroizing copies of the credential after this boundary.
    // The application-owned buffer is still zeroized when the secret is dropped.
    fn into_client(self, http: reqwest::Client, endpoint: String) -> AuthResult<ClaudeClient> {
        match self {
            Self::AuthToken(token) => {
                let mut authorization =
                    HeaderValue::from_str(&format!("Bearer {}", token.expose_secret()))
                        .map_err(|_| AuthError::InvalidAnthropicAuthToken)?;
                authorization.set_sensitive(true);
                let mut headers = HeaderMap::new();
                headers.insert(AUTHORIZATION, authorization);
                Ok(ClaudeClient::with_auth_headers(http, endpoint, headers))
            }
            Self::ApiKey(api_key) => Ok(ClaudeClient::new(http, endpoint, api_key.expose_secret())),
        }
    }
}

impl SelectedAuth {
    fn into_openai_auth(self, auth_file: &Path) -> AuthResult<OpenAiAuth> {
        match self {
            Self::ChatGpt => load_chatgpt_auth(auth_file).map_err(Into::into),
            Self::ApiKey(api_key) => {
                // Nanocodex owns the retained key after this boundary. The application-owned
                // buffer is still zeroized when `api_key` is dropped.
                Ok(OpenAiAuth::api_key(api_key.expose_secret()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AnthropicCredential, SelectedAuth, messages_endpoint};
    use crate::app::{
        config::{AuthConfig, AuthMode},
        error::AuthError,
        secret::SecretString,
    };
    use nanocodex::oai::auth::OpenAiAuthMode;
    use std::{cell::Cell, fs};
    use tempfile::tempdir;

    #[test]
    fn auto_prefers_an_existing_chatgpt_file_without_reading_the_api_key() {
        let directory = tempdir().unwrap();
        let auth_file = directory.path().join("auth.json");
        fs::write(&auth_file, "invalid but present").unwrap();
        let api_key_read = Cell::new(false);

        let config = AuthConfig::new(AuthMode::Auto, auth_file);
        let selected = config
            .select_auth(|| {
                api_key_read.set(true);
                Ok(Some(SecretString::new("api-key".into())))
            })
            .unwrap();

        assert!(matches!(selected, SelectedAuth::ChatGpt));
        assert!(!api_key_read.get());
    }

    #[test]
    fn auto_falls_back_to_an_api_key_when_chatgpt_is_absent() {
        let directory = tempdir().unwrap();
        let config = AuthConfig::new(AuthMode::Auto, directory.path().join("auth.json"));
        let selected = config
            .select_auth(|| Ok(Some(SecretString::new("api-key".into()))))
            .unwrap();

        assert!(matches!(selected, SelectedAuth::ApiKey(_)));
    }

    #[test]
    fn forced_chatgpt_does_not_read_the_api_key() {
        let api_key_read = Cell::new(false);
        let config = AuthConfig::new(AuthMode::ChatGpt, "missing.json".into());
        let selected = config
            .select_auth(|| {
                api_key_read.set(true);
                Ok(Some(SecretString::new("api-key".into())))
            })
            .unwrap();

        assert!(matches!(selected, SelectedAuth::ChatGpt));
        assert!(!api_key_read.get());
    }

    #[test]
    fn forced_api_key_reports_a_missing_environment_value() {
        let config = AuthConfig::new(AuthMode::ApiKey, "unused.json".into());
        let result = config.select_auth(|| Ok(None));

        assert!(matches!(result, Err(AuthError::ApiKeyUnavailable)));
    }

    #[test]
    fn selected_api_key_constructs_nanocodex_authorization() {
        let selected = SelectedAuth::ApiKey(SecretString::new("api-key".into()));
        let auth = selected.into_openai_auth("unused.json".as_ref()).unwrap();

        assert_eq!(auth.mode(), OpenAiAuthMode::ApiKey);
    }

    #[test]
    fn anthropic_auth_token_takes_precedence_over_an_api_key() {
        let api_key_read = Cell::new(false);
        let credential = AnthropicCredential::from_environment(
            || Ok(Some(SecretString::new("token".into()))),
            || {
                api_key_read.set(true);
                Ok(Some(SecretString::new("api-key".into())))
            },
        )
        .unwrap();

        assert!(matches!(credential, AnthropicCredential::AuthToken(_)));
        assert!(!api_key_read.get());
    }

    #[test]
    fn anthropic_api_key_is_used_without_an_auth_token() {
        let credential = AnthropicCredential::from_environment(
            || Ok(None),
            || Ok(Some(SecretString::new("api-key".into()))),
        )
        .unwrap();

        assert!(matches!(credential, AnthropicCredential::ApiKey(_)));
    }

    #[test]
    fn missing_anthropic_credentials_are_reported() {
        let result = AnthropicCredential::from_environment(|| Ok(None), || Ok(None));

        assert!(matches!(
            result,
            Err(AuthError::AnthropicCredentialsUnavailable)
        ));
    }

    #[test]
    fn messages_endpoint_joins_proxy_base_urls() {
        assert_eq!(
            messages_endpoint("https://proxy.example/proxy/anthropic/"),
            "https://proxy.example/proxy/anthropic/v1/messages"
        );
        assert_eq!(
            messages_endpoint("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn logout_is_idempotent() {
        let directory = tempdir().unwrap();
        let auth_file = directory.path().join("auth.json");
        fs::write(&auth_file, "credentials").unwrap();
        let config = AuthConfig::new(AuthMode::ChatGpt, auth_file.clone());

        config.logout().unwrap();
        assert!(!auth_file.exists());
        config.logout().unwrap();
    }
}
