use anyhow::{Context, Result, bail};
use data_encoding::BASE32_NOPAD;
use reqwest::blocking::{Client, RequestBuilder, Response};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::io::{self, Write};

const SSO_URL: &str = "https://oauth.battle.net/oauth/sso";
const AUTH_BASE_URL: &str =
    "https://authenticator-rest-api.bnet-identity.blizzard.net/v1/authenticator";
const CLIENT_ID: &str = "baedda12fe054e4abdfc3ad7bdea970a";

struct ApiClient {
    client: Client,
    bearer_token: Option<String>,
}

#[derive(Serialize)]
struct SsoRequest<'a> {
    client_id: &'a str,
    grant_type: &'a str,
    scope: &'a str,
    token: &'a str,
}

#[derive(Deserialize)]
struct SsoResponse {
    access_token: Option<String>,
}

#[derive(Serialize)]
struct RestoreRequest<'a> {
    serial: &'a str,
    #[serde(rename = "restoreCode")]
    restore_code: &'a str,
}

#[derive(Deserialize)]
struct RestoreResponse {
    #[serde(rename = "deviceSecret")]
    device_secret: Option<String>,
}

impl ApiClient {
    fn new() -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("bnet-auth-export/0.1"));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

        let client = Client::builder()
            .default_headers(headers)
            .build()
            .context("failed to build HTTP client")?;

        Ok(Self {
            client,
            bearer_token: None,
        })
    }

    fn exchange_session_token(&mut self, session_token: &str) -> Result<()> {
        let token = normalize_session_token(session_token);
        let request = SsoRequest {
            client_id: CLIENT_ID,
            grant_type: "client_sso",
            scope: "auth.authenticator",
            token: &token,
        };

        let response = self
            .client
            .post(SSO_URL)
            .header(
                CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded; charset=utf-8"),
            )
            .form(&request)
            .send()
            .context("request failed for Battle.net SSO token exchange")?;

        let parsed: SsoResponse = parse_json_response(response, "SSO token exchange", 500)?;
        let access_token = parsed
            .access_token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("SSO response did not include access_token")?;

        self.bearer_token = Some(access_token.to_owned());
        Ok(())
    }

    fn restore_device_secret(&self, serial: &str, restore_code: &str) -> Result<String> {
        let bearer_token = self
            .bearer_token
            .as_deref()
            .context("bearer token not set; SSO token exchange must run first")?;

        let request = RestoreRequest {
            serial,
            restore_code,
        };
        let url = format!("{AUTH_BASE_URL}/device");

        let response = self
            .authorized_post(&url, bearer_token)
            .json(&request)
            .send()
            .with_context(|| format!("request failed for {url}"))?;

        let parsed: RestoreResponse = parse_json_response(response, "restore request", 1000)?;
        let device_secret = parsed
            .device_secret
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("restore response missing deviceSecret")?;

        Ok(device_secret.to_owned())
    }

    fn authorized_post<'a>(&'a self, url: &'a str, bearer_token: &str) -> RequestBuilder {
        self.client
            .post(url)
            .header(AUTHORIZATION, format!("Bearer {bearer_token}"))
    }
}

fn parse_json_response<T>(response: Response, label: &str, body_limit: usize) -> Result<T>
where
    T: DeserializeOwned,
{
    let status = response.status();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .unwrap_or_default();
    let body = response.text().context("failed reading response body")?;

    if !status.is_success() {
        bail!(
            "{label} failed with HTTP {}. Response: {}",
            status.as_u16(),
            truncate(&body, body_limit)
        );
    }

    if !is_json_content_type(&content_type) {
        bail!(
            "{label} returned non-JSON content (Content-Type: {}). Response: {}",
            display_content_type(&content_type),
            truncate(&body, body_limit)
        );
    }

    serde_json::from_str(&body).with_context(|| format!("failed to parse {label} JSON response"))
}

fn is_json_content_type(content_type: &str) -> bool {
    content_type.to_ascii_lowercase().contains("json")
}

fn display_content_type(content_type: &str) -> &str {
    if content_type.is_empty() {
        "(missing)"
    } else {
        content_type
    }
}

fn normalize_session_token(input: &str) -> String {
    let trimmed = input.trim();
    trimmed
        .strip_prefix("ST=")
        .or_else(|| trimmed.strip_prefix("st="))
        .unwrap_or(trimmed)
        .trim()
        .to_owned()
}

fn hex_to_base32_nopad_upper(hex_secret: &str) -> Result<String> {
    let bytes = hex::decode(hex_secret.trim()).context("deviceSecret is not valid hex")?;
    Ok(BASE32_NOPAD.encode(&bytes))
}

fn build_otpauth_uri(serial: &str, base32_secret: &str) -> String {
    format!(
        "otpauth://totp/Battle.net:{serial}?secret={base32_secret}&issuer=Battle.net&digits=8&algorithm=SHA1&period=30"
    )
}

fn prompt(label: &str) -> Result<String> {
    print!("{label}");
    io::stdout().flush().context("failed to flush stdout")?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("failed to read input")?;

    Ok(input.trim().to_owned())
}

fn truncate(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

fn ensure_non_empty(value: String, field_name: &str) -> Result<String> {
    if value.is_empty() {
        bail!("{field_name} is required");
    }
    Ok(value)
}

fn run() -> Result<()> {
    let session_token = ensure_non_empty(prompt("Session Token (ST=...): ")?, "session token")?;
    let serial = ensure_non_empty(prompt("Authenticator Serial: ")?, "authenticator serial")?;
    let restore_code = ensure_non_empty(prompt("Restore Code: ")?, "restore code")?;

    let mut api = ApiClient::new()?;
    api.exchange_session_token(&session_token)?;
    let device_secret = api.restore_device_secret(&serial, &restore_code)?;
    let base32_secret = hex_to_base32_nopad_upper(&device_secret)?;
    let otpauth = build_otpauth_uri(&serial, &base32_secret);

    println!("\nBattle.net export succeeded");
    println!("Serial: {serial}");
    println!("TOTP settings: SHA1 / 8 digits / 30s");
    println!("\notpauth URI (paste into your authenticator app):");
    println!("{otpauth}");

    Ok(())
}

fn main() -> Result<()> {
    run()
}
