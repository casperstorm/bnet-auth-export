use anyhow::{Context, Result};
use data_encoding::BASE32_NOPAD;
use reqwest::StatusCode;
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

        let parsed = read_json(response).context("SSO token exchange failed")?;
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

        let parsed = read_json(response)
            .map_err(|error| error.context(restore_hint(serial, restore_code)))
            .context("restore request failed")?;

        if parsed
            .get("requireHealup")
            .and_then(|value| value.as_bool())
            == Some(true)
        {
            anyhow::bail!(
                "Blizzard requires a 'heal up' step for this account before it will return the \
                 authenticator secret. Open the Battle.net app, check the Authenticator for any \
                 pending prompts, then try again."
            );
        }

        let device_secret = parsed
            .get("deviceSecret")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("restore response missing deviceSecret")?;

        Ok(device_secret.to_owned())
    }
}

// An HTTP error from Blizzard with the fields that explain it. The raw body is
// not kept because it can echo submitted values.
#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    url: String,
    detail: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {} from {}", self.status, self.url)?;
        if !self.detail.is_empty() {
            write!(f, " ({})", self.detail)?;
        }
        Ok(())
    }
}

impl std::error::Error for ApiError {}

// Parse a JSON response, or turn an error status into an `ApiError` that
// includes Blizzard's `errorCode`/`message`.
fn read_json(response: reqwest::blocking::Response) -> Result<serde_json::Value> {
    let status = response.status();
    let url = response.url().to_string();
    let body = response.text().context("failed to read response body")?;

    if status.is_success() {
        return serde_json::from_str(&body).context("failed to parse response as JSON");
    }

    let json = serde_json::from_str::<serde_json::Value>(&body).ok();
    let field = |key: &str| json.as_ref()?.get(key)?.as_str().map(str::to_owned);
    let detail = match &json {
        Some(_) => ["errorCode", "message", "error", "error_description"]
            .iter()
            .filter_map(|key| Some(format!("{key}: {}", field(key)?)))
            .collect::<Vec<_>>()
            .join(", "),
        None => body.trim().chars().take(200).collect(),
    };

    Err(ApiError {
        status,
        url,
        detail,
    }
    .into())
}

// The `ApiError` somewhere in an error chain, if any.
fn api_error(error: &anyhow::Error) -> Option<&ApiError> {
    error
        .chain()
        .find_map(|cause| cause.downcast_ref::<ApiError>())
}

// Tips shown when the restore call is rejected, based on what the user typed.
fn restore_hint(serial: &str, restore_code: &str) -> String {
    let mut tips = vec![
        "Make sure you logged in to the same Battle.net account that owns this authenticator."
            .to_owned(),
    ];

    // Restore codes never use I, L, O or S; they are easy to confuse with 1, 1, 0 and 5.
    let confusable: String = restore_code
        .chars()
        .filter(|c| matches!(c.to_ascii_uppercase(), 'I' | 'L' | 'O' | 'S'))
        .collect();
    if !confusable.is_empty() {
        tips.push(format!(
            "Your restore code contains '{confusable}'. Restore codes never use the letters I, L, O or S; did you mean 1, 1, 0 or 5?"
        ));
    }
    if restore_code.len() != 10 {
        tips.push(format!(
            "Your restore code is {} characters long; it is normally 10.",
            restore_code.len()
        ));
    }
    if !serial.contains('-') {
        tips.push(
            "Copy the serial exactly as the app shows it, including the region prefix (e.g. US)."
                .to_owned(),
        );
    }

    format!("Hints:\n  - {}", tips.join("\n  - "))
}

// Pull the session token out of whatever the user pasted: the bare value, the
// `ST=...` part of the redirect URL (or the whole URL), or a `BA-tassadar=...`
// cookie, either on its own or inside a full copied cookie header.
fn clean_session_token(input: &str) -> String {
    let input = input.trim();
    let (rest, terminators): (&str, &[char]) =
        if let Some((_, rest)) = input.split_once("BA-tassadar=") {
            (rest, &[';', ' ', '\t'])
        } else if let Some((_, rest)) = input.rsplit_once("ST=") {
            (rest, &['&', '#', ' ', '\t'])
        } else {
            (input, &[';', '&'])
        };
    let value = rest.split(terminators).next().unwrap_or(rest);
    value.trim().trim_matches('"').to_owned()
}

// Restore codes are shown in upper case without separators.
fn clean_restore_code(input: &str) -> String {
    input
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

// Keep dashes (the user may have typed the format the app shows), but drop
// whitespace and normalize case.
fn clean_serial(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

const TOKEN_HELP: &str = "\
Blizzard rejected that session token. The `ST=` value in the URL does not work for
everyone; the `BA-tassadar` cookie usually does:
  1. Stay on the 404 page you were redirected to after logging in.
  2. Open the developer tools (Chrome: Cmd/Ctrl+Option/Shift+I, Safari: enable the
     Develop menu first).
  3. Chrome: Application > Cookies > https://account.battle.net
     Safari/Firefox: Storage > Cookies > https://account.battle.net
  4. Copy the value of `BA-tassadar` (starts with US-, EU-, KR- ...).
If that also fails, log in again in a new private window and paste the new value right away.";

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

// Print an error and its causes, one per line.
fn print_error(error: &anyhow::Error) {
    eprintln!("\nError: {error}");
    for cause in error.chain().skip(1) {
        eprintln!("  {cause}");
    }
    eprintln!();
}

// Exchange the session token, asking for another one (e.g. the BA-tassadar
// cookie) until it works or the user gives up.
fn bearer_token(api: &Api) -> Result<String> {
    let mut input = prompt("Session Token (ST=... or BA-tassadar cookie): ")?;
    let mut shown_help = false;
    loop {
        let token = clean_session_token(&input);
        let error = match api.exchange_session_token(&token) {
            Ok(bearer) => return Ok(bearer),
            Err(error) => error,
        };
        print_error(&error);

        // Only a rejected token is worth retrying; network errors etc. are not.
        let rejected = api_error(&error).is_some_and(|e| e.status.is_client_error());
        if !rejected {
            return Err(error);
        }
        if !shown_help {
            eprintln!("{TOKEN_HELP}\n");
            shown_help = true;
        }

        input = prompt("Session Token (or press Enter to quit): ")?;
        if input.is_empty() {
            std::process::exit(1);
        }
    }
}

// Restore with the given serial, and if Blizzard rejects it and the serial
// has dashes, try once more without them.
fn restore(api: &Api, bearer: &str, serial: &str, restore_code: &str) -> Result<(String, String)> {
    let error = match api.device_secret(bearer, serial, restore_code) {
        Ok(secret) => return Ok((serial.to_owned(), secret)),
        Err(error) => error,
    };
    let rejected = api_error(&error).is_some_and(|e| e.status == StatusCode::BAD_REQUEST);
    if rejected && serial.contains('-') {
        let undashed = serial.replace('-', "");
        if let Ok(secret) = api.device_secret(bearer, &undashed, restore_code) {
            return Ok((undashed, secret));
        }
    }
    Err(error)
}

fn main() -> Result<()> {
    let api = Api::new()?;

    // Exchange the token right away; it can expire within minutes.
    let bearer = bearer_token(&api)?;

    let mut serial = clean_serial(&prompt("Authenticator Serial: ")?);
    let mut restore_code = clean_restore_code(&prompt("Restore Code: ")?);
    let (serial, device_secret) = loop {
        let error = match restore(&api, &bearer, &serial, &restore_code) {
            Ok(result) => break result,
            Err(error) => error,
        };
        print_error(&error);

        // A 400 means Blizzard did not accept the serial/restore code; let the
        // user correct them without logging in again.
        let rejected = api_error(&error).is_some_and(|e| e.status == StatusCode::BAD_REQUEST);
        if !rejected {
            return Err(error);
        }
        eprintln!("Check the serial and restore code, or press Enter to quit.");
        let input = prompt("Authenticator Serial: ")?;
        if input.is_empty() {
            std::process::exit(1);
        }
        serial = clean_serial(&input);
        restore_code = clean_restore_code(&prompt("Restore Code: ")?);
    };

    let base32_secret = to_base32_secret(&device_secret)?;
    let otpauth = build_otpauth_uri(&serial, &base32_secret);

    println!("\nBattle.net export succeeded");
    println!("\notpauth URI (paste into your authenticator app):");
    println!("{otpauth}");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_session_token() {
        let token = "US-d2662eb54f9d6fd3a07c6953117fe276-52135628";
        let cases = [
            token.to_owned(),
            format!(" ST={token}&flowTrackingId= "),
            format!("https://account.battle.net/login/en/localhost?ST={token}&flowTrackingId="),
            format!("BA-tassadar={token}"),
            format!("BA-tassadar=\"{token}\""),
            format!("Cookie: foo=bar; BA-tassadar={token}; BA-tassadar-login-key=xyz"),
            format!("{token};"),
        ];
        for case in cases {
            assert_eq!(clean_session_token(&case), token, "input: {case}");
        }
    }

    #[test]
    fn cleans_restore_code_and_serial() {
        assert_eq!(clean_restore_code(" ab12-cd 34ef "), "AB12CD34EF");
        assert_eq!(clean_serial(" us-2111 2222-3333 "), "US-21112222-3333");
    }

    #[test]
    fn flags_confusable_restore_code_characters() {
        let hint = restore_hint("US123", "AB0O1ISLX9");
        assert!(hint.contains("'OISL'"));
        assert!(!restore_hint("US123", "AB0C1DEFX9").contains("never use"));
        assert!(restore_hint("US123", "ABC").contains("3 characters"));
    }
}
