# Internal Linux release verification — 8 October 2026

Account page: https://www.roviumlabs.me/products/comrade

Gateway: https://comradeserver.vercel.app

Both applications were built, deployed to Vercel, checked before promotion,
and promoted to their existing domains. Supabase's built-in `auth.users`
provides email/password accounts. No custom database migration was applied.
Usage enforcement is disabled; account request records go to server logs.
The optional migration prepares durable tracking and browser pairing later.

## Completed checks

- 97 core tests and 27 desktop UI checks passed. UI checks use a mocked Tauri
  bridge; the final account suite also passed after the email-first screen changes.
- Browser controller tests, frontend build, and native workspace check passed.
- Gateway race tests and Go vet passed, including desktop embedding/speech
  alias routing to configured provider models.
- Website production build, types and browser fixtures passed.
- Temporary live Supabase users verified password sign-in, refresh, public
  gateway identity checks, anonymous 401 responses and unlimited account mode.
  Test accounts were deleted afterward; verification did not send emails.
- The live public website signed in, verified the account through the gateway
  with CORS, showed unlimited testing and cleared its session on sign-out.
- Hosted GPT-OSS chat and memory embeddings passed live requests. Embeddings
  returned 1,536 dimensions. DeepSeek's provider returned a transient 429 from
  its shared upstream pool; select GPT-OSS if that provider remains throttled.
- Linux release build and Debian/portable unpacking checks passed. Private
  voice libraries resolve from the package, runtime paths are relative, license
  notices are included and both artifact checksums verify.
- The extracted app and portable launcher each stayed running for 12 seconds
  with isolated state. Existing account/history/memory/config files were unchanged.
  No browser or model download occurred during the signed-out startup checks.
- Server secrets are absent from tracked environment files and website browser
  chunks. Release packaging scans for secret keys and private state filenames.

## Remaining verification boundaries

The native startup check establishes process stability; it does not verify the
rendered native account screen, microphone/speaker behavior or full browser/tool
workflows against real user projects. Local voice inference and live hosted audio
were not exercised in this run. Core/UI fixture checks cover those contracts.

Artifacts are unsigned Linux x86_64 internal builds from an Arch host. The
executable's required GLIBC symbol floor is 2.34; compatibility on other
distributions has not been established. System WebKit/GTK/ALSA dependencies are
required. No Windows/macOS package or installation on another Linux machine
was performed. See `RELEASE.md` and the artifact `BUILD.json`/dependency reports.

Supabase's Auth Site URL and redirect allow list are managed separately in its
dashboard. A live recovery-link generation initially fell back to
`http://localhost:3000`; the user is updating it to the hosted account page.
Email delivery itself remains untested.

## Artifacts

`target/release/distributions/Comrade-1.0.0-linux-x86_64.deb`

`target/release/distributions/Comrade-1.0.0-linux-x86_64.tar.gz`

The archive includes `comrade`; extract it and run `./comrade` inside the
extracted folder. Verify `SHA256SUMS` before sharing either artifact.
