import './env.js'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('../_lib/mail.js', () => ({ sendLicenseEmail: vi.fn(async () => 'sent' as const) }))

import { GET, handleWebhook } from '../webhook.js'
import { sendLicenseEmail } from '../_lib/mail.js'
import { seedFromEnv, verifyKey } from '../_lib/license.js'
import {
  asStripe,
  checkoutCompletedEvent,
  createFakeStripe,
  post,
  signedRequest,
  webhooks,
} from './fake-stripe.js'

const URL_ = 'https://warden.test/api/webhook'
const SECRET = process.env.STRIPE_WEBHOOK_SECRET as string
const PUBLIC_KEY = Buffer.from(
  (JSON.parse(readFileSync(join(import.meta.dirname, '../../../docs/license-pubkey.json'), 'utf8')) as {
    publicKey: string
  }).publicKey,
  'base64url',
)
const body = async (response: Response) => (await response.json()) as Record<string, unknown>
const mail = vi.mocked(sendLicenseEmail)

beforeEach(() => {
  mail.mockReset()
  mail.mockResolvedValue('sent')
})

describe('signature verification', () => {
  it('rejects a request with no stripe-signature header', async () => {
    const fake = createFakeStripe([{ id: 'cs_a' }])
    const response = await handleWebhook(post(URL_, checkoutCompletedEvent('cs_a')), asStripe(fake))

    expect(response.status).toBe(400)
    expect(await body(response)).toEqual({ error: 'missing_signature' })
    expect(fake.calls.sessionUpdate).toHaveLength(0)
  })

  it('rejects a garbage signature', async () => {
    const fake = createFakeStripe([{ id: 'cs_a' }])
    const response = await handleWebhook(
      post(URL_, checkoutCompletedEvent('cs_a'), { 'stripe-signature': 't=1,v1=deadbeef' }),
      asStripe(fake),
    )

    expect(response.status).toBe(400)
    expect(await body(response)).toEqual({ error: 'invalid_signature' })
    expect(fake.calls.sessionUpdate).toHaveLength(0)
  })

  it('rejects a signature made with a different secret', async () => {
    const fake = createFakeStripe([{ id: 'cs_a' }])
    const payload = checkoutCompletedEvent('cs_a')
    const response = await handleWebhook(signedRequest(URL_, payload, 'whsec_attacker_secret'), asStripe(fake))

    expect(response.status).toBe(400)
    expect(fake.calls.sessionUpdate).toHaveLength(0)
  })

  it('rejects a body mutated after signing', async () => {
    const fake = createFakeStripe([{ id: 'cs_victim' }, { id: 'cs_attacker' }])
    const signed = checkoutCompletedEvent('cs_victim')
    const header = webhooks.generateTestHeaderString({ payload: signed, secret: SECRET })

    // Same signature, different body: the classic parse-then-verify bug.
    const mutated = checkoutCompletedEvent('cs_attacker')
    const response = await handleWebhook(post(URL_, mutated, { 'stripe-signature': header }), asStripe(fake))

    expect(response.status).toBe(400)
    expect(await body(response)).toEqual({ error: 'invalid_signature' })
    expect(fake.calls.sessionUpdate).toHaveLength(0)
    expect(fake.calls.sessionRetrieve).toHaveLength(0)
  })

  it('rejects a single flipped byte in the payload', async () => {
    const fake = createFakeStripe([{ id: 'cs_a' }])
    const payload = checkoutCompletedEvent('cs_a')
    const header = webhooks.generateTestHeaderString({ payload, secret: SECRET })
    const tweaked = payload.replace('"created":1785700000', '"created":1785700001')

    const response = await handleWebhook(post(URL_, tweaked, { 'stripe-signature': header }), asStripe(fake))
    expect(response.status).toBe(400)
  })

  it('rejects a stale capture outside the timestamp tolerance', async () => {
    const fake = createFakeStripe([{ id: 'cs_a' }])
    const payload = checkoutCompletedEvent('cs_a')
    const old = Math.floor(Date.now() / 1000) - 60 * 60

    const response = await handleWebhook(signedRequest(URL_, payload, SECRET, old), asStripe(fake))
    expect(response.status).toBe(400)
  })

  it('never tells the caller which check failed', async () => {
    const fake = createFakeStripe([{ id: 'cs_a' }])
    const responses = await Promise.all([
      handleWebhook(post(URL_, checkoutCompletedEvent('cs_a'), { 'stripe-signature': 't=1,v1=00' }), asStripe(fake)),
      handleWebhook(signedRequest(URL_, checkoutCompletedEvent('cs_a'), 'whsec_wrong'), asStripe(fake)),
    ])
    const bodies = await Promise.all(responses.map(body))
    expect(bodies[0]).toEqual(bodies[1])
  })

  it('reads the raw body before parsing it', () => {
    const source = readFileSync(join(import.meta.dirname, '../webhook.ts'), 'utf8')
    const rawAt = source.indexOf('await request.text()')
    const verifyAt = source.indexOf('constructEvent')
    const parseAt = source.indexOf('event.data.object')
    expect(rawAt).toBeGreaterThan(-1)
    expect(rawAt).toBeLessThan(verifyAt)
    expect(verifyAt).toBeLessThan(parseAt)
    expect(source).not.toContain('request.json()')
  })
})

describe('minting', () => {
  it('mints, stores and emails on a paid session', async () => {
    const fake = createFakeStripe([{ id: 'cs_paid', email: 'Buyer@Example.com', payment_intent: 'pi_1' }])
    const response = await handleWebhook(
      signedRequest(URL_, checkoutCompletedEvent('cs_paid'), SECRET),
      asStripe(fake),
    )

    expect(response.status).toBe(200)
    expect(await body(response)).toMatchObject({ received: true, minted: true })

    const stored = fake.sessions.get('cs_paid')?.metadata.license_key as string
    expect(stored).toMatch(/^WRDN-/)
    expect(verifyKey(stored, PUBLIC_KEY)).toEqual({
      v: 1,
      id: 'cs_paid',
      email: 'buyer@example.com',
      seats: 1,
      iat: 1785700000,
    })

    expect(fake.calls.paymentIntentUpdate).toHaveLength(1)
    expect(fake.calls.paymentIntentUpdate[0]?.id).toBe('pi_1')
    expect((fake.calls.paymentIntentUpdate[0]?.params as { metadata: Record<string, string> }).metadata.license_key).toBe(
      stored,
    )
    expect(mail).toHaveBeenCalledTimes(1)
  })

  it('lowercases the email into the key', async () => {
    const fake = createFakeStripe([{ id: 'cs_case', email: '  MiXeD@Example.COM ' }])
    await handleWebhook(signedRequest(URL_, checkoutCompletedEvent('cs_case'), SECRET), asStripe(fake))
    const claims = verifyKey(fake.sessions.get('cs_case')?.metadata.license_key as string, PUBLIC_KEY)
    expect(claims?.email).toBe('mixed@example.com')
  })

  it.each([
    ['warden-1', 1],
    ['warden-2', 2],
    ['warden-3', 3],
  ])('gives %s exactly %i seats', async (sku, seats) => {
    // Stripe object ids are [A-Za-z0-9_]+, never hyphenated, so the id here is
    // not built from the sku string.
    const id = `cs_seats_${seats}`
    const fake = createFakeStripe([{ id, metadata: { sku } }])
    await handleWebhook(signedRequest(URL_, checkoutCompletedEvent(id), SECRET), asStripe(fake))
    const claims = verifyKey(fake.sessions.get(id)?.metadata.license_key as string, PUBLIC_KEY)
    expect(claims?.seats).toBe(seats)
  })

  it('ignores a tampered seats field in the session metadata', async () => {
    // Seats are re-derived from the SKU through the server table. A metadata
    // edit (dashboard slip, or a compromised key with write access) buys nothing.
    const fake = createFakeStripe([{ id: 'cs_tamper', metadata: { sku: 'warden-1', seats: '99' } }])
    await handleWebhook(signedRequest(URL_, checkoutCompletedEvent('cs_tamper'), SECRET), asStripe(fake))

    const claims = verifyKey(fake.sessions.get('cs_tamper')?.metadata.license_key as string, PUBLIC_KEY)
    expect(claims?.seats).toBe(1)
  })

  it.each(['warden-99', '', undefined])(
    'acknowledges another product on the shared account (sku %o) without minting',
    async (sku) => {
      // The account is shared with FRONTIER, so this endpoint receives that
      // business's checkout events. Answering 500 would make Stripe retry them
      // forever; answering 200 makes it stop.
      const fake = createFakeStripe([
        { id: 'cs_other', metadata: sku === undefined ? { sku: '' } : { sku } },
      ])
      const response = await handleWebhook(
        signedRequest(URL_, checkoutCompletedEvent('cs_other'), SECRET),
        asStripe(fake),
      )

      expect(response.status).toBe(200)
      expect(await body(response)).toMatchObject({ handled: false, reason: 'not_warden' })
      expect(fake.calls.sessionUpdate).toHaveLength(0)
      expect(mail).not.toHaveBeenCalled()
    },
  )

  it('does not mint for a session that is not paid', async () => {
    const fake = createFakeStripe([{ id: 'cs_unpaid', payment_status: 'unpaid' }])
    const response = await handleWebhook(
      signedRequest(URL_, checkoutCompletedEvent('cs_unpaid'), SECRET),
      asStripe(fake),
    )

    expect(response.status).toBe(200)
    expect(await body(response)).toMatchObject({ handled: false, reason: 'not_paid' })
    expect(fake.calls.sessionUpdate).toHaveLength(0)
    expect(mail).not.toHaveBeenCalled()
  })

  it('ignores event types it does not fulfil', async () => {
    const fake = createFakeStripe([{ id: 'cs_a' }])
    const payload = JSON.stringify({
      id: 'evt_x',
      object: 'event',
      type: 'payment_intent.created',
      data: { object: { id: 'pi_1' } },
    })

    const response = await handleWebhook(signedRequest(URL_, payload, SECRET), asStripe(fake))
    expect(response.status).toBe(200)
    expect(await body(response)).toMatchObject({ handled: false })
    expect(fake.calls.sessionRetrieve).toHaveLength(0)
  })
})

describe('idempotency', () => {
  it('a replayed event does not mint a second key', async () => {
    const fake = createFakeStripe([{ id: 'cs_replay' }])
    const payload = checkoutCompletedEvent('cs_replay')

    const first = await handleWebhook(signedRequest(URL_, payload, SECRET), asStripe(fake))
    const key = fake.sessions.get('cs_replay')?.metadata.license_key

    const second = await handleWebhook(signedRequest(URL_, payload, SECRET), asStripe(fake))
    const third = await handleWebhook(signedRequest(URL_, payload, SECRET), asStripe(fake))

    expect(await body(first)).toMatchObject({ minted: true })
    expect(await body(second)).toMatchObject({ minted: false })
    expect(await body(third)).toMatchObject({ minted: false })

    expect(fake.calls.sessionUpdate).toHaveLength(1)
    expect(fake.calls.paymentIntentUpdate).toHaveLength(1)
    expect(fake.sessions.get('cs_replay')?.metadata.license_key).toBe(key)
    expect(mail).toHaveBeenCalledTimes(1)
  })

  it('two concurrent deliveries converge on the identical key', async () => {
    // The mint is a pure function of (session id, email, seats, session.created),
    // so even if both calls read before either writes, they write the same string.
    const fake = createFakeStripe([{ id: 'cs_race' }])
    const payload = checkoutCompletedEvent('cs_race')

    await Promise.all([
      handleWebhook(signedRequest(URL_, payload, SECRET), asStripe(fake)),
      handleWebhook(signedRequest(URL_, payload, SECRET), asStripe(fake)),
    ])

    const written = fake.calls.sessionUpdate.map(
      (call) => (call.params as { metadata: Record<string, string> }).metadata.license_key,
    )
    expect(new Set(written).size).toBe(1)
  })

  it('re-mints a key that this signing identity cannot verify', async () => {
    const fake = createFakeStripe([{ id: 'cs_stale', metadata: { license_key: 'WRDN-not.avalidkey' } }])
    const response = await handleWebhook(
      signedRequest(URL_, checkoutCompletedEvent('cs_stale'), SECRET),
      asStripe(fake),
    )

    expect(await body(response)).toMatchObject({ minted: true })
    expect(verifyKey(fake.sessions.get('cs_stale')?.metadata.license_key as string, PUBLIC_KEY)).not.toBeNull()
  })
})

describe('mail failures never fail the purchase', () => {
  it('still reports success when Resend is down', async () => {
    mail.mockResolvedValue('failed')
    const fake = createFakeStripe([{ id: 'cs_mailfail' }])
    const response = await handleWebhook(
      signedRequest(URL_, checkoutCompletedEvent('cs_mailfail'), SECRET),
      asStripe(fake),
    )

    expect(response.status).toBe(200)
    expect(fake.sessions.get('cs_mailfail')?.metadata.license_key).toMatch(/^WRDN-/)
  })

  it('emails a working download link', async () => {
    const fake = createFakeStripe([{ id: 'cs_link' }])
    await handleWebhook(signedRequest(URL_, checkoutCompletedEvent('cs_link'), SECRET), asStripe(fake))

    const sent = mail.mock.calls[0]?.[0]
    expect(sent?.downloadUrl).toMatch(/^https:\/\/warden\.test\/api\/download\?token=cs_link\./)
  })
})

describe('method guard', () => {
  it('answers GET with 405', () => {
    expect(GET().status).toBe(405)
  })

  it('seedFromEnv is what the webhook signs with', () => {
    expect(() => seedFromEnv(process.env.LICENSE_SIGNING_KEY)).not.toThrow()
  })
})
