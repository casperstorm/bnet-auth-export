# bnet-auth-export

Small CLI to export a Battle.net authenticator into third-party TOTP apps.  

Enter your `session token`, `serial`, and `restore code` when prompted. The tool then prints an `otpauth://` URL you can paste into a third-party authenticator app.

## Run

### Download a binary

Download the latest binary from the [Releases](https://github.com/casperstorm/bnet-auth-export/releases) page, extract it, and run `bnet-auth-export` (or `bnet-auth-export.exe` on Windows).

### Build from source (Rust)

```bash
cargo run
```

Dont have Rust? https://rust-lang.org/tools/install/.

## How to

### Serial and Restore Code

1. Open the Battle.net app on your phone.
2. Open **Authenticator**.
3. Open **Settings**.
4. Copy your **Serial** and **Restore Code**.

## Session Token (`ST=...`)

1. Open a private/incognito browser window.
2. Go to [https://account.battle.net/login/en/?ref=localhost](https://account.battle.net/login/en/?ref=localhost).
3. Log in to the Battle.net account that owns the authenticator.
4. After login, you should be redirected to a `localhost` URL (often an error page).
5. Copy the `ST=...` value from the address bar and paste it into the tool. It should look something like: 
`US-h6392c12...1kh10n2p7-531234`

## How does this work?

The CLI uses your Battle.net session token to request a temporary bearer token from Blizzard. It then uses Blizzard's authenticator restore flow (with your `serial` and `restore code`) as a trick to get the authenticator `deviceSecret`. That `deviceSecret` is converted into a standard `otpauth://` URL so you can import it into a normal TOTP app.

This does not remove, reset, or otherwise affect your existing Battle.net authenticator.

## Why?

Because i don't like to have it inside the Battle.net mobile app.

## Inspiration

* [bnet_auth_tool](https://github.com/Nighthawk42/bnet_auth_tool) by Nighthawk42.
