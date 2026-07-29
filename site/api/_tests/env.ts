// Test-only env loading. The signing key lives in the repo-root .env.local
// (gitignored); in production it is a Vercel env var and nothing reads a file.
// Import this before anything that touches process.env.
//
// Nothing here ever prints a value. A test that logs a secret is a leak with a
// green checkmark next to it.

import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const ROOT = join(import.meta.dirname, '../../..')

const load = (file: string) => {
  let text: string
  try {
    text = readFileSync(join(ROOT, file), 'utf8')
  } catch {
    return
  }
  for (const line of text.split('\n')) {
    const match = /^\s*([A-Z0-9_]+)\s*=\s*(.*)$/.exec(line)
    if (!match) continue
    const [, key, raw] = match as unknown as [string, string, string]
    if (process.env[key] !== undefined) continue
    process.env[key] = raw.trim().replace(/^["']|["']$/g, '')
  }
}

load('.env.local')

/** Deterministic stand-ins for the secrets that have no local source. Never used
 *  against a real Stripe account: every handler test drives a fake client. */
process.env.STRIPE_SECRET_KEY ??= 'sk_test_dummy_for_unit_tests'
process.env.STRIPE_WEBHOOK_SECRET ??= 'whsec_test_dummy_for_unit_tests'
process.env.DOWNLOAD_TOKEN_SECRET ??= 'download_token_secret_for_unit_tests_only'
process.env.GITHUB_RELEASE_TOKEN ??= 'ghp_dummy_for_unit_tests'
process.env.PUBLIC_SITE_URL ??= 'https://warden.test'
