# WARDEN launch spec

The contract every workstream builds against. If something here is ambiguous, the
ambiguity is a bug in this file: fix the file, do not guess locally.

Status: authoritative as of 2026-07-29.

## 0. What we are shipping

A one-time purchase (no subscriptions, ever) that gets a buyer:

1. a license key, offline-verifiable, delivered on the success page and by email
2. a signed, expiring download link to a macOS build
3. an app that checks the key locally and never phones home

Three SKUs, matching what the landing page already sells:

| SKU | Name | Price | Seats | Product | Price id |
|---|---|---|---|---|---|
| `warden-1` | One Mac | $15.00 | 1 | `prod_UyZTENqYDCUtrt` | `price_1TycKNEnDphWh4zvWeScnLkf` |
| `warden-2` | Two Macs | $25.00 | 2 | `prod_UyZTw8O7rsyQjk` | `price_1TycKSEnDphWh4zvvE85TJf5` |
| `warden-3` | Three Macs | $30.00 | 3 | `prod_UyZToa7Y1nPVMG` | `price_1TycKUEnDphWh4zvNSxOn2yI` |

Currency USD. `mode: "payment"`. Never `mode: "subscription"`.

These are LIVE ids in `acct_1TxtpfEnDphWh4zv`, created 2026-07-29. They are the
default lookup table, but they are overridable per SKU by env
(`PRICE_WARDEN_1` and so on) so that pointing the site at a different Stripe
account never needs a code change. `scripts/stripe-provision.mjs` prints the ids
it creates in a fresh account, ready to paste into those vars.

## 1. Decisions already made (do not relitigate)

**Stripe account.** The connected account is `acct_1TxtpfEnDphWh4zv` ("FRONTIER"),
live mode. A separate Warden legal entity needs KYC (EIN/SSN, bank, phone) that
cannot be done through an API, so launch runs on the existing account with
`statement_descriptor_suffix: "WARDEN"` so a buyer's card statement reads
`FRONTIER* WARDEN`. `scripts/stripe-provision.mjs` recreates every product, price
and webhook in a fresh account from one command, so moving to a dedicated Warden
account later is a key swap, not a rebuild. Everything reads
`STRIPE_SECRET_KEY` from env: no account id is ever hardcoded.

**No new database.** Stripe is the system of record. The license key is written
to the Checkout Session's `metadata.license_key` and to the PaymentIntent. Key
recovery searches Stripe by customer email. Rationale: it deletes a whole failure
domain and a monthly cost, and Stripe already holds the authoritative record of
who paid. Cost of the choice: revoking a license is a manual metadata edit in the
dashboard, which is acceptable at this volume. Do not add Supabase, KV, or Redis.

**Artifact storage = GitHub Releases on `karimbabasf/WARDEN`,** which is going
private as part of this launch. Release assets on a private repo are not publicly
fetchable, so `/api/download` is the only way in: it validates the token, then
streams the asset server-side with a fine-grained PAT. Free, versioned, no new
vendor. The PAT is `GITHUB_RELEASE_TOKEN`, contents:read on that one repo only.

**Email is optional at the edges, never on the critical path.** The success page
itself renders the license key and download button, so a buyer who never receives
an email still has the product. Email (Resend, `RESEND_API_KEY`) is a convenience
copy. If the key is unset, log a warning and carry on: the purchase must still
succeed.

## 2. License key format (BOTH sides must match byte for byte)

Producer: `site/api/_lib/license.ts`. Consumer: `src-tauri/src/license/mod.rs`.

Payload is compact JSON, keys in exactly this order, no whitespace:

```json
{"v":1,"id":"<stripe checkout session id>","email":"<lowercased>","seats":<1|2|3>,"iat":<unix seconds>}
```

Signature is Ed25519 over the exact UTF-8 payload bytes. The key string is:

```
WRDN-<base64url(payload) no padding>.<base64url(signature) no padding>
```

Verification is offline and total: decode, check the signature against the
compiled-in public key, then check `v == 1`. There is no expiry, no seat
enforcement over the network, no activation call. A valid signature is the whole
gate.

Keypair generation is `scripts/gen-license-keypair.mjs`. The private key lives
ONLY in the Vercel env as `LICENSE_SIGNING_KEY` (base64url, 32-byte seed). The
public key is committed, because it is public by definition. The private key is
never written to the repo, the vault, or chat.

Cross-language conformance is not optional: `docs/license-vectors.json` holds 8
vectors (5 valid, 3 tampered). The Node suite and the Rust suite both read that
same file, so a drift between producer and consumer fails a test rather than a
customer.

## 3. Download token

HMAC-SHA256 over `<license_id>:<exp>` with `DOWNLOAD_TOKEN_SECRET`, compared in
constant time. Format `<license_id>.<exp>.<base64url(mac)>`. Expiry 48h from
issue. `/api/download?token=` validates, then streams the release asset.

The token is not a bearer secret worth much: it is anti-hotlink and anti-scrape,
not DRM. The real product gate is the license key, which is why the token can be
stateless and short-lived without anyone losing access to what they bought.

## 4. Layout

```
site/                 Vercel project root
  api/                serverless functions (TypeScript)
    checkout.ts       POST -> creates a Checkout Session, returns url
    webhook.ts        POST -> Stripe webhook, mints + stores the license
    license.ts        GET  -> reads a completed session, returns key + download url
    download.ts       GET  -> validates token, streams the DMG
    recover.ts        POST -> email lookup, re-sends the key
    _lib/             license.ts, tokens.ts, stripe.ts, mail.ts
  public/             GENERATED by build.mjs from ../landing. Never edit by hand.
  build.mjs           copies ../landing -> public, injects PUBLIC_* env
  vercel.json
landing/              the page itself. Single source of truth, stays here.
src-tauri/src/license/  the Rust gate
scripts/              stripe-provision.mjs, gen-license-keypair.mjs, release.mjs
```

`landing/` does not move. `site/public` is generated, gitignored, and exists only
so Vercel has something to serve. Two copies of a 2200-line page would drift
within a day.

## 5. Env vars

| Name | Where | Purpose |
|---|---|---|
| `STRIPE_SECRET_KEY` | Vercel | server-side Stripe |
| `STRIPE_WEBHOOK_SECRET` | Vercel | webhook signature check |
| `LICENSE_SIGNING_KEY` | Vercel | Ed25519 seed, base64url |
| `DOWNLOAD_TOKEN_SECRET` | Vercel | HMAC secret, 32 random bytes |
| `GITHUB_RELEASE_TOKEN` | Vercel | fine-grained PAT, contents:read |
| `RESEND_API_KEY` | Vercel, optional | receipt email |
| `PUBLIC_STRIPE_KEY` | build time | publishable key for the buy button |

No secret is ever committed. `.env.local` is gitignored.

## 6. Security rules, non-negotiable

- Price is NEVER taken from the client. `/api/checkout` accepts a SKU id from the
  fixed set above and looks the price up server-side. A client that posts an
  amount gets a 400.
- The webhook verifies `stripe-signature` with the raw body before it parses
  anything. Vercel's default body parsing destroys the raw body: read the raw
  stream explicitly.
- Constant-time compare for every MAC and signature.
- The license private key never leaves the server. The app only ever holds the
  public key.
- No secret, key, or customer email is logged.
- `/api/*` returns no stack traces to the client.

## 7. Distribution reality

There is no Apple Developer ID on this machine (`security find-identity` returns
0 valid identities). An unsigned, un-notarized app downloaded from the web is
quarantined by Gatekeeper and macOS reports it as damaged, which reads as a
broken product rather than a security prompt.

So: the build is ad-hoc signed, the DMG ships with a first-run instruction that
is accurate for an unsigned app, and the pipeline is written so that setting
`APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD` and `APPLE_TEAM_ID` turns
on real signing plus notarization with no other change. Buying the $99/yr Apple
Developer Program is the single highest-value thing Karim can do after this
lands, and the download page says so honestly rather than pretending.
