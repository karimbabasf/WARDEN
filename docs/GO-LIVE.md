# WARDEN go-live

Everything that could be built and verified is done and deployed. What is left
needs a credential or a purchase that no API can do on your behalf.

Site: https://warden.karimbabasf.com
Stripe account: `acct_1TxtpfEnDphWh4zv` (live)
Vercel project: `warden` under `kbkotes-projects`

## 1. The one step that turns the buy button on

`STRIPE_SECRET_KEY` is the only thing standing between this and taking money.

It is not recoverable from anywhere on this machine: Vercel stores sensitive
values write-only, so `vercel env pull` returns `""` even for you, and there is
no Stripe CLI config here. Get it from
https://dashboard.stripe.com/acct_1TxtpfEnDphWh4zv/apikeys and then:

```
cd ~/Developer/Apps/WARDEN
printf '%s' 'sk_live_...' | vercel env add STRIPE_SECRET_KEY production
vercel deploy --prod --yes
```

Verify it took, in one command. Before the key this returns 500; after, 400:

```
curl -s -X POST https://warden.karimbabasf.com/api/checkout \
  -H 'Content-Type: application/json' -d '{"sku":"warden-99"}'
```

Then buy your own copy with a real card and refund it in the dashboard. That is
the only way to exercise the full path (card, webhook, license mint, download)
end to end, and it costs you nothing but the Stripe fee.

## 2. Email receipts, optional

Without it the purchase still works: the success page shows the key and the
download, and the key is stored on the Stripe session either way. With it, the
buyer also gets a copy they can find again in six months.

```
printf '%s' 're_...' | vercel env add RESEND_API_KEY production
printf '%s' 'WARDEN <you@yourverifieddomain>' | vercel env add MAIL_FROM production
```

`MAIL_FROM` must be a domain verified in Resend. I deliberately did not guess one:
a wrong sender fails silently, which is worse than no email.

## 3. Downloads

`GITHUB_RELEASE_TOKEN` is a fine-grained PAT, `contents: read`, scoped to
`karimbabasf/WARDEN` only. GitHub does not expose PAT creation through an API, so
this is a web-UI step: https://github.com/settings/personal-access-tokens/new

```
printf '%s' 'github_pat_...' | vercel env add GITHUB_RELEASE_TOKEN production
```

Then publish the build. The script is a dry run until `--confirm`:

```
node scripts/release.mjs                 # prints the plan, touches nothing
node scripts/release.mjs --confirm       # creates the GitHub release + uploads
```

## 4. The Gatekeeper problem, read this before selling

There is no Apple Developer ID on this machine, so the DMG is ad-hoc signed.
macOS will tell buyers WARDEN "is damaged and can't be opened." It is not
damaged; that is what Gatekeeper says about any unsigned app from the internet.
`docs/INSTALL.md` and the success page both explain the right-click-Open fix.

This is the highest-value $99 you can spend on the product. With a Developer ID:

```
export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export APPLE_ID=... APPLE_PASSWORD=... APPLE_TEAM_ID=...
node scripts/release.mjs --confirm
```

Tauri then signs, notarizes and staples during the build. No code changes.

Also worth knowing: the build is **arm64 only**. Intel Macs cannot run it.

## 5. Things I decided, that you may want to change

- **Sales run through the FRONTIER Stripe account.** A separate Warden entity
  needs KYC that no API can do, and blocking revenue on paperwork is backwards.
  Card statements read `FRONTIER* WARDEN`. When you want a real Warden account,
  create it, then `node scripts/stripe-provision.mjs` against the new key and
  paste the three price ids it prints into `PRICE_WARDEN_1..3`. No code changes.
- **The repo is private, and its fork is not.** `david-kendrick` forked it on
  2026-07-16; going private detached that into an independent public repo which
  cannot be removed.
- **MIT cannot be taken back.** LICENSE is now a paid EULA, but anything
  published up to `0d55bf8` stays MIT for whoever already has it. The file says
  so rather than pretending otherwise.
- **The live origin is `warden.karimbabasf.com`**, attached 2026-07-29. No domain
  was bought: `karimbabasf.com` was already on Vercel DNS with a wildcard ALIAS,
  so the subdomain verified instantly and Let's Encrypt issued straight away.
  `warden-beryl.vercel.app` still resolves and is harmless.

  Three things move together whenever that origin changes, and two of them are
  invisible if you forget: `PUBLIC_SITE_URL` (it is baked into emailed download
  links, so a stale value only surfaces when a customer clicks one weeks later),
  the Stripe webhook endpoint URL (`we_1TycoiEnDphWh4zvPmtRApjP`; changing its
  URL does NOT rotate the signing secret, so `STRIPE_WEBHOOK_SECRET` survives),
  and a redeploy to pick the env var up. All three are done for this domain.
