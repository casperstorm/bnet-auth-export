use anyhow::{Context, Result};
use data_encoding::BASE32_NOPAD;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};
use std::io::{self, Write};

const SSO_URL: &str = "https://oauth.battle.net/oauth/sso";
const AUTH_BASE_URL: &str =
    "https://authenticator-rest-api.bnet-identity.blizzard.net/v1/authenticator";
const CLIENT_ID: &str = "baedda12fe054e4abdfc3ad7bdea970a";

struct Api(Client);

impl Api {
    fn new() -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("bnet-auth-export/0.1"));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

        let client = Client::builder()
            .default_headers(headers)
            .build()
            .context("failed to build HTTP client")?;

        Ok(Self(client))
    }

    // Exchange the user session token for an OAuth bearer token.
    fn exchange_session_token(&self, session_token: &str) -> Result<String> {
        let response = self
            .0
            .post(SSO_URL)
            .header(
                CONTENT_TYPE,
                HeaderValue::from_static("application/x-www-form-urlencoded; charset=utf-8"),
            )
            .form(&[
                ("client_id", CLIENT_ID),
                ("grant_type", "client_sso"),
                ("scope", "auth.authenticator"),
                ("token", session_token),
            ])
            .send()
            .context("request failed for Battle.net SSO token exchange")?;

        let parsed: serde_json::Value = response
            .error_for_status()
            .context("SSO token exchange failed")?
            .json()
            .context("failed to parse SSO token exchange response")?;
        let access_token = parsed
            .get("access_token")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("SSO response did not include access_token")?;

        Ok(access_token.to_owned())
    }

    // Restore the authenticator and return the device secret using a bearer token.
    fn device_secret(
        &self,
        bearer_token: &str,
        serial: &str,
        restore_code: &str,
    ) -> Result<String> {
        let url = format!("{AUTH_BASE_URL}/device");

        let response = self
            .0
            .post(&url)
            .header(AUTHORIZATION, format!("Bearer {bearer_token}"))
            .json(&serde_json::json!({
                "serial": serial,
                "restoreCode": restore_code,
            }))
            .send()
            .with_context(|| format!("request failed for {url}"))?;

        let parsed: serde_json::Value = response
            .error_for_status()
            .context("restore request failed")?
            .json()
            .context("failed to parse restore response")?;
        let device_secret = parsed
            .get("deviceSecret")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("restore response missing deviceSecret")?;

        Ok(device_secret.to_owned())
    }
}

// Convert Blizzard's hex device secret into Base32 for otpauth URIs.
fn to_base32_secret(hex_secret: &str) -> Result<String> {
    let bytes = hex::decode(hex_secret.trim()).context("deviceSecret is not valid hex")?;
    Ok(BASE32_NOPAD.encode(&bytes))
}

// Build the otpauth URI with Battle.net's TOTP parameters.
fn build_otpauth_uri(serial: &str, base32_secret: &str) -> String {
    format!(
        "otpauth://totp/Battle.net:{serial}?secret={base32_secret}&issuer=Battle.net&digits=8&algorithm=SHA1&period=30"
    )
}

// Prompt for a single line of input and return the trimmed value.
fn prompt(label: &str) -> Result<String> {
    print!("{label}");
    io::stdout().flush().context("failed to flush stdout")?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("failed to read input")?;

    Ok(input.trim().to_owned())
}

fn main() -> Result<()> {
    let session_token = prompt("Session Token (ST=...): ")?;
    let serial = prompt("Authenticator Serial: ")?;
    let restore_code = prompt("Restore Code: ")?;

    let api = Api::new()?;

    let bearer_token = api.exchange_session_token(&session_token)?;
    let device_secret = api.device_secret(&bearer_token, &serial, &restore_code)?;
    let base32_secret = to_base32_secret(&device_secret)?;

    let otpauth = build_otpauth_uri(&serial, &base32_secret);

    println!("\nBattle.net export succeeded");
    println!("\notpauth URI (paste into your authenticator app):");
    println!("{otpauth}");

    Ok(())
}
