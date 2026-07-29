#!/usr/bin/env node
// Provisions the WARDEN catalog and webhook in whatever account STRIPE_SECRET_KEY
// points at. Idempotent: re-running changes nothing and duplicates nothing, so
// moving to a dedicated Warden account later is a key swap, not a rebuild
// (LAUNCH-SPEC section 1).
//
//   node scripts/stripe-provision.mjs --url https://warden.app/api/webhook
//   node scripts/stripe-provision.mjs --url ... --dry-run
//
// Talks to the REST API over fetch rather than the SDK, so it has no
// dependencies and runs from a bare checkout. No account id is ever hardcoded:
// the key decides the account.
//
// Idempotency comes from matching on metadata.sku. The three products already
// exist live with Stripe-generated ids, so existence cannot be a GET on a guessed
// id. It is deliberately a paginated LIST rather than /products/search: search is
// eventually consistent and lags a write by up to a minute, which is precisely
// how a re-run would create a second copy of something it just made.

import { fileURLToPath } from 'node:url'

const API = 'https://api.stripe.com/v1'

/** The price list from LAUNCH-SPEC section 0. Must stay identical to SKUS in
 *  site/api/_lib/stripe.ts, which is asserted by a test in that package
 *  (api/_tests/provision.test.ts) rather than left to memory. */
export const SKUS = [
  {
    id: 'warden-1',
    name: 'WARDEN, one Mac',
    description:
      'WARDEN on a single Mac. A live board for every coding agent running on your machine. One payment, yours forever.',
    amount: 1500,
    seats: 1,
  },
  {
    id: 'warden-2',
    name: 'WARDEN, two Macs',
    description:
      'WARDEN on two Macs. A live board for every coding agent running on your machines. One payment, yours forever.',
    amount: 2500,
    seats: 2,
  },
  {
    id: 'warden-3',
    name: 'WARDEN, three Macs',
    description:
      'WARDEN on three Macs. A live board for every coding agent running on your machines. One payment, yours forever.',
    amount: 3000,
    seats: 3,
  },
]

export const CURRENCY = 'usd'

export const WEBHOOK_EVENTS = [
  'checkout.session.completed',
  'checkout.session.async_payment_succeeded',
  'checkout.session.async_payment_failed',
]

/** Stripe takes form encoding, including for nested keys: metadata[x]=y and
 *  enabled_events[]=z. */
const encode = (params, prefix = '') => {
  const parts = []
  for (const [key, value] of Object.entries(params)) {
    if (value === undefined || value === null) continue
    const name = prefix ? `${prefix}[${key}]` : key
    if (Array.isArray(value)) {
      for (const item of value) parts.push(`${encodeURIComponent(`${name}[]`)}=${encodeURIComponent(item)}`)
    } else if (typeof value === 'object') {
      parts.push(encode(value, name))
    } else {
      parts.push(`${encodeURIComponent(name)}=${encodeURIComponent(value)}`)
    }
  }
  return parts.filter(Boolean).join('&')
}

const call = async (key, method, path, params) => {
  const url = method === 'GET' && params ? `${API}${path}?${encode(params)}` : `${API}${path}`
  const response = await fetch(url, {
    method,
    headers: {
      authorization: `Bearer ${key}`,
      'content-type': 'application/x-www-form-urlencoded',
      'stripe-version': '2026-06-24.dahlia',
    },
    body: method === 'GET' ? undefined : encode(params ?? {}),
  })

  const payload = await response.json()
  if (!response.ok) {
    const error = payload?.error ?? {}
    const err = new Error(error.message ?? `stripe ${response.status}`)
    err.code = error.code
    err.statusCode = response.status
    throw err
  }
  return payload
}

/** Walks every page of a list endpoint. Stripe caps a page at 100. */
const listAll = async (key, path, params = {}) => {
  const items = []
  let startingAfter
  for (;;) {
    const page = await call(key, 'GET', path, { ...params, limit: 100, starting_after: startingAfter })
    items.push(...page.data)
    if (!page.has_more || page.data.length === 0) return items
    startingAfter = page.data[page.data.length - 1].id
  }
}

const productMetadata = (sku) => ({ sku: sku.id, seats: String(sku.seats), product: 'warden' })

const needsUpdate = (product, sku) =>
  product.name !== sku.name ||
  product.description !== sku.description ||
  product.metadata?.seats !== String(sku.seats) ||
  product.metadata?.product !== 'warden'

const ensureProduct = async (key, sku, products, dryRun) => {
  // metadata.sku is the identity. The account is shared with another business,
  // so "a product named WARDEN something" is not a safe match and an id cannot
  // be guessed: Stripe assigned these.
  const existing = products.find((product) => product.metadata?.sku === sku.id)

  if (existing) {
    if (!needsUpdate(existing, sku)) return { product: existing, created: false, updated: false }
    if (dryRun) return { product: existing, created: false, updated: true }

    const product = await call(key, 'POST', `/products/${existing.id}`, {
      name: sku.name,
      description: sku.description,
      metadata: productMetadata(sku),
    })
    return { product, created: false, updated: true }
  }

  if (dryRun) return { product: { id: '(new)', name: sku.name }, created: true, updated: false }

  const product = await call(key, 'POST', '/products', {
    name: sku.name,
    description: sku.description,
    metadata: productMetadata(sku),
  })
  return { product, created: true, updated: false }
}

const ensurePrice = async (key, sku, product, dryRun) => {
  if (product.id === '(new)') return { price: { id: `(new, ${sku.amount})` }, created: true, strays: [] }

  const active = await listAll(key, '/prices', { product: product.id, active: 'true' })
  const match = active.find((price) => price.unit_amount === sku.amount && price.currency === CURRENCY)
  const strays = active.filter((price) => price.id !== match?.id)

  if (match) return { price: match, created: false, strays }
  if (dryRun) return { price: { id: `(new, ${sku.amount})` }, created: true, strays }

  const price = await call(key, 'POST', '/prices', {
    product: product.id,
    currency: CURRENCY,
    unit_amount: sku.amount,
    metadata: { sku: sku.id },
  })

  // Give a freshly built account the same shape as the live one, where each
  // product's default_price is its WARDEN price.
  if (!product.default_price) {
    await call(key, 'POST', `/products/${product.id}`, { default_price: price.id })
  }

  return { price, created: true, strays }
}

const ensureWebhook = async (key, url, dryRun) => {
  const existing = await call(key, 'GET', '/webhook_endpoints', { limit: 100 })
  const match = existing.data.find((endpoint) => endpoint.url === url)

  if (match) {
    const sameEvents =
      match.enabled_events.length === WEBHOOK_EVENTS.length &&
      WEBHOOK_EVENTS.every((event) => match.enabled_events.includes(event))

    if (!sameEvents && !dryRun) {
      const updated = await call(key, 'POST', `/webhook_endpoints/${match.id}`, {
        enabled_events: WEBHOOK_EVENTS,
      })
      return { endpoint: updated, created: false, updated: true }
    }
    return { endpoint: match, created: false, updated: !sameEvents }
  }

  if (dryRun) return { endpoint: { id: '(new)', url }, created: true, updated: false }

  const endpoint = await call(key, 'POST', '/webhook_endpoints', {
    url,
    enabled_events: WEBHOOK_EVENTS,
    description: 'WARDEN license fulfillment',
    api_version: '2026-06-24.dahlia',
  })
  return { endpoint, created: true, updated: false }
}

const flag = (argv, name) => {
  const index = argv.indexOf(name)
  return index === -1 ? undefined : argv[index + 1]
}

export const main = async (argv = process.argv.slice(2)) => {
  const key = process.env.STRIPE_SECRET_KEY
  if (!key) {
    console.error('STRIPE_SECRET_KEY is not set.')
    process.exitCode = 1
    return
  }

  const dryRun = argv.includes('--dry-run')
  const webhookUrl = flag(argv, '--url') ?? process.env.WEBHOOK_URL
  if (!webhookUrl) {
    console.error('Pass the webhook URL: --url https://<site>/api/webhook (or set WEBHOOK_URL).')
    process.exitCode = 1
    return
  }
  if (!webhookUrl.startsWith('https://')) {
    console.error('The webhook URL must be https.')
    process.exitCode = 1
    return
  }

  // Writing to a live account is a real-money action, so it needs saying out
  // loud. --dry-run is always allowed.
  const live = key.startsWith('sk_live_')
  if (live && !dryRun && !argv.includes('--yes')) {
    console.error('That is a LIVE key. Re-run with --yes to write to the live account, or --dry-run to preview.')
    process.exitCode = 1
    return
  }

  console.log(`${dryRun ? 'DRY RUN, no writes. ' : ''}Mode: ${live ? 'LIVE' : 'test'}`)
  console.log(`Webhook: ${webhookUrl}\n`)

  // One list for all three SKUs: the match is done in memory against
  // metadata.sku, so the account is read once rather than per product.
  const products = await listAll(key, '/products', { active: 'true' })

  const envLines = []
  for (const sku of SKUS) {
    const { product, created: productCreated, updated } = await ensureProduct(key, sku, products, dryRun)
    const { price, created: priceCreated, strays } = await ensurePrice(key, sku, product, dryRun)

    const productState = productCreated ? '(created)' : updated ? '(updated in place)' : '(exists, unchanged)'
    console.log(`${sku.id}  $${(sku.amount / 100).toFixed(2)}  ${sku.seats} seat${sku.seats === 1 ? '' : 's'}`)
    console.log(`  product  ${product.id} ${productState}`)
    console.log(`  price    ${price.id} ${priceCreated ? '(created)' : '(exists, unchanged)'}`)
    for (const stray of strays ?? []) {
      console.log(`  note     another active price on this product: ${stray.id} (${stray.unit_amount} ${stray.currency})`)
    }
    envLines.push(`PRICE_${sku.id.toUpperCase().replace(/-/g, '_')}=${price.id}`)
  }

  const { endpoint, created, updated } = await ensureWebhook(key, webhookUrl, dryRun)
  console.log(`\nwebhook  ${endpoint.id} ${created ? '(created)' : updated ? '(events updated)' : '(exists)'}`)

  console.log('\nPut these in the Vercel env:')
  for (const line of envLines) console.log(`  ${line}`)

  if (endpoint.secret) {
    console.log(`  STRIPE_WEBHOOK_SECRET=${endpoint.secret}`)
    console.log('\nThat signing secret is shown once, at creation. Copy it now.')
  } else if (!dryRun) {
    console.log(
      '  STRIPE_WEBHOOK_SECRET=<the endpoint already existed, so Stripe did not return its secret.\n' +
        '                         Reveal it in Dashboard > Developers > Webhooks > this endpoint.>',
    )
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  main().catch((err) => {
    // Never print the key, and never a raw object that might carry it.
    console.error(`Provisioning failed: ${err.message}`)
    process.exitCode = 1
  })
}
