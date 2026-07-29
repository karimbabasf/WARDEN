import './env.js'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { POST, handleLicense } from '../license.js'
import { verifyKey } from '../_lib/license.js'
import { TOKEN_TTL_SECONDS, verifyToken } from '../_lib/tokens.js'
import { asStripe, createFakeStripe, get } from './fake-stripe.js'

const URL_ = 'https://warden.test/api/license'
const SECRET = process.env.DOWNLOAD_TOKEN_SECRET as string
const PUBLIC_KEY = Buffer.from(
  (JSON.parse(readFileSync(join(import.meta.dirname, '../../../docs/license-pubkey.json'), 'utf8')) as {
    publicKey: string
  }).publicKey,
  'base64url',
)
const body = async (response: Response) => (await response.json()) as Record<string, any>

describe('only a paid session gets a key', () => {
  it.each(['unpaid', 'no_payment_required'])('refuses a session with payment_status %s', async (status) => {
    const fake = createFakeStripe([{ id: 'cs_pending', payment_status: status }])
    const response = await handleLicense(get(`${URL_}?session_id=cs_pending`), asStripe(fake))

    expect(response.status).toBe(409)
    expect(await body(response)).toEqual({ error: 'payment_not_complete' })
    expect(fake.calls.sessionUpdate).toHaveLength(0)
  })

  it('never puts a key in the body of a refusal', async () => {
    const fake = createFakeStripe([{ id: 'cs_pending', payment_status: 'unpaid' }])
    const text = await (await handleLicense(get(`${URL_}?session_id=cs_pending`), asStripe(fake))).text()
    expect(text).not.toContain('WRDN-')
  })

  it('404s an unknown session', async () => {
    const fake = createFakeStripe()
    const response = await handleLicense(get(`${URL_}?session_id=cs_nope`), asStripe(fake))
    expect(response.status).toBe(404)
    expect(await body(response)).toEqual({ error: 'not_found' })
  })

  it('404s a paid session belonging to another product on the shared account', async () => {
    const fake = createFakeStripe([{ id: 'cs_frontier', metadata: { sku: 'something-else' } }])
    const response = await handleLicense(get(`${URL_}?session_id=cs_frontier`), asStripe(fake))

    expect(response.status).toBe(404)
    expect(await body(response)).toEqual({ error: 'not_found' })
    expect(fake.calls.sessionUpdate).toHaveLength(0)
  })

  it.each(['', 'cs-with-hyphen', 'cs_test/../etc', 'cs_test%20', '../../secret', 'cs test'])(
    'rejects a malformed session_id %o before calling Stripe',
    async (id) => {
      const fake = createFakeStripe()
      const response = await handleLicense(get(`${URL_}?session_id=${encodeURIComponent(id)}`), asStripe(fake))
      expect(response.status).toBe(400)
      expect(fake.calls.sessionRetrieve).toHaveLength(0)
    },
  )

  it('rejects a missing session_id', async () => {
    const fake = createFakeStripe()
    expect((await handleLicense(get(URL_), asStripe(fake))).status).toBe(400)
  })
})

describe('a paid session', () => {
  it('returns the five fields the success page needs', async () => {
    const fake = createFakeStripe([{ id: 'cs_ok', email: 'Buyer@Example.com', metadata: { sku: 'warden-2' } }])
    const response = await handleLicense(get(`${URL_}?session_id=cs_ok`), asStripe(fake))
    const payload = await body(response)

    expect(response.status).toBe(200)
    expect(Object.keys(payload).sort()).toEqual(['downloadUrl', 'email', 'expiresAt', 'licenseKey', 'seats'])
    expect(payload.email).toBe('buyer@example.com')
    expect(payload.seats).toBe(2)
    expect(verifyKey(payload.licenseKey, PUBLIC_KEY)).toMatchObject({ v: 1, id: 'cs_ok', seats: 2 })
  })

  it('hands back a download link that actually validates', async () => {
    const fake = createFakeStripe([{ id: 'cs_dl' }])
    const payload = await body(await handleLicense(get(`${URL_}?session_id=cs_dl`), asStripe(fake)))

    const token = new URL(payload.downloadUrl).searchParams.get('token')
    expect(verifyToken(token, SECRET)).toMatchObject({ licenseId: 'cs_dl' })
  })

  it('reports an expiry 48 hours out', async () => {
    const fake = createFakeStripe([{ id: 'cs_exp' }])
    const payload = await body(await handleLicense(get(`${URL_}?session_id=cs_exp`), asStripe(fake)))

    const seconds = (Date.parse(payload.expiresAt) - Date.now()) / 1000
    expect(seconds).toBeGreaterThan(TOKEN_TTL_SECONDS - 60)
    expect(seconds).toBeLessThanOrEqual(TOKEN_TTL_SECONDS)
  })

  it('is not cached anywhere: it carries a license key', async () => {
    const fake = createFakeStripe([{ id: 'cs_cache' }])
    const response = await handleLicense(get(`${URL_}?session_id=cs_cache`), asStripe(fake))
    expect(response.headers.get('cache-control')).toBe('no-store')
  })

  it('mints when the webhook has not landed, then keeps returning the same key', async () => {
    const fake = createFakeStripe([{ id: 'cs_early' }])

    const first = await body(await handleLicense(get(`${URL_}?session_id=cs_early`), asStripe(fake)))
    const second = await body(await handleLicense(get(`${URL_}?session_id=cs_early`), asStripe(fake)))

    expect(first.licenseKey).toBe(second.licenseKey)
    expect(fake.calls.sessionUpdate).toHaveLength(1)
  })

  it('ignores a tampered seats field, exactly as the webhook does', async () => {
    const fake = createFakeStripe([{ id: 'cs_seats', metadata: { sku: 'warden-1', seats: '3' } }])
    const payload = await body(await handleLicense(get(`${URL_}?session_id=cs_seats`), asStripe(fake)))

    expect(payload.seats).toBe(1)
    expect(verifyKey(payload.licenseKey, PUBLIC_KEY)?.seats).toBe(1)
  })

  it('answers POST with 405', () => {
    expect(POST().status).toBe(405)
  })
})
