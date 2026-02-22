# bnet-auth-export

Small CLI to export a Battle.net authenticator into third-party TOTP apps.  

Enter your `session token`, `serial`, and `restore code` when prompted. The tool then prints an `otpauth://` URL you can paste into a third-party authenticator app.

## Run

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
5. Copy the `ST=...` value from the address bar and paste it into the tool.

## Why?

Because i don't like to have it inside the Battle.net mobile app.

## Inspiration

* [bnet_auth_tool](https://github.com/Nighthawk42/bnet_auth_tool) by Nighthawk42:
