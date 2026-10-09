# Accounts and unlimited internal testing

Create an account on https://www.roviumlabs.me/products/comrade, confirm the
email using Supabase's confirmation message, and sign in in the desktop app
with the same email and password. Password reset is available on that page.
Desktop tools, chat, history, browser and coding controls remain behind the
native account gate. Sign-out stops active work and removes the native session.

Supabase Auth already provides the account table `auth.users`: `email` and
`encrypted_password` are managed by Auth. Do not create an application table
containing raw passwords. Sign-up is enabled for this project; email confirmation
is currently required. Configure the Supabase Auth Site URL and redirect allow
list to include `https://www.roviumlabs.me/products/comrade` for confirmation and
recovery links. The project API keys do not grant Auth configuration or arbitrary
database schema changes. Email delivery and recovery redirects require separate
live verification after this dashboard configuration is confirmed.

The browser uses only the publishable Supabase key. Native credentials are kept
in `auth-session.json` under Comrade home, with mode 0600 on Unix. IPC exposes
account metadata, never access or refresh tokens. The account manager verifies
and refreshes the session, binds it to the configured gateway URL, and sends the
user access token on service requests. Transient network errors retain the
session but prevent operations that need account validation; invalid sessions
return the UI to sign-in. The gateway verifies project identity and signatures.

Server code lives in `../comrade_server`. Its ignored `.env` holds server-only
Supabase and provider keys. The website code lives in `../Rovium_Labs_web`.
The hosted gateway URL is `https://comradeserver.vercel.app`.

## Tracking seam

There are no user quotas or credit deductions. `GET /v1/auth/me` reports
`usage.mode=unlimited` and `enforcement_enabled=false`. `USAGE_SINK=log` records
request ID, account ID, endpoint, model, status, duration and token counts when
the provider returns them. It does not record passwords, tokens or message
content. Logs are not a permanent credit ledger. Normal generation token and
task safety settings remain independent from account usage limits.

The server's optional `migrations/001_accounts.sql` prepares profiles, usage
events and encrypted desktop pairings with RLS and atomic claims. It has not
been applied for this phase. Future credit enforcement belongs in the gateway
after identity verification; aggregate and reconcile durable usage before
deducting credits. Keep any pricing/enforcement decision server-side.

## Optional browser pairing

Hosted `DEVICE_STORE=disabled` uses direct desktop email sign-in and needs no
custom tables. Browser pairing stays hidden by default. After applying the
optional migration, set `DEVICE_STORE=supabase` on the server and
`COMRADE_BROWSER_LOGIN_ENABLED=true` for the desktop. Pairing displays a code
and requires explicit website consent. Session tokens are transferred once
through native polling, never URL parameters. Memory pairing is for a single
persistent development process and must not be used on Vercel or replicas.

## Verification

Core HTTP fixtures cover sign-in, refresh, revocation, URL binding, cancellation
and protected storage. Desktop UI checks use a mocked Tauri bridge. Website
checks use API fixtures. Server tests cover JWT validation, protected endpoints,
pairing and usage recording. These checks do not establish email delivery,
microphone/speaker behavior, or install compatibility on other operating systems.
