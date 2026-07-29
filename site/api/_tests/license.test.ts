// Conformance against the frozen vectors. These are the same 8 vectors the Rust
// verifier reads, so a drift between producer and consumer fails here rather
// than at a customer.

import './env.js'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import {
  encodePayload,
  mintKey,
  publicKeyFromSeed,
  seedFromEnv,
  verifyKey,
  type LicenseClaims,
} from '../_lib/license.js'

const ROOT = join(import.meta.dirname, '../../..')
const vectors = JSON.parse(readFileSync(join(ROOT, 'docs/license-vectors.json'), 'utf8')) as {
  publicKey: string
  valid: { name: string; claims: LicenseClaims; key: string }[]
  invalid: { name: string; key: string; reason: string }[]
}
const pubkey = JSON.parse(readFileSync(join(ROOT, 'docs/license-pubkey.json'), 'utf8')) as {
  alg: string
  publicKey: string
}

const PUBLIC_KEY = Buffer.from(vectors.publicKey, 'base64url')

describe('conformance vectors', () => {
  it('has the 8 vectors the spec promises', () => {
    expect(vectors.valid).toHaveLength(5)
    expect(vectors.invalid).toHaveLength(3)
  })

  it('agrees with the committed public key', () => {
    expect(pubkey.publicKey).toBe(vectors.publicKey)
    expect(pubkey.alg).toBe('ed25519')
  })

  it.each(vectors.valid)('accepts: $name', ({ claims, key }) => {
    const verified = verifyKey(key, PUBLIC_KEY)
    expect(verified).toEqual(claims)
  })

  it.each(vectors.invalid)('rejects: $name', ({ key }) => {
    expect(verifyKey(key, PUBLIC_KEY)).toBeNull()
  })
})

describe('producer matches the frozen minter byte for byte', () => {
  const seed = seedFromEnv(process.env.LICENSE_SIGNING_KEY)

  it('derives the committed public key from the signing seed', () => {
    expect(publicKeyFromSeed(seed).toString('base64url')).toBe(vectors.publicKey)
  })

  it.each(vectors.valid)('reproduces the exact key string for: $name', ({ claims, key }) => {
    expect(mintKey(seed, claims)).toBe(key)
  })

  it('serializes the payload with the spec key order and no whitespace', () => {
    expect(encodePayload({ v: 1, id: 'cs_x', email: 'a@b.co', seats: 2, iat: 1785700000 })).toBe(
      '{"v":1,"id":"cs_x","email":"a@b.co","seats":2,"iat":1785700000}',
    )
  })

  it('round-trips a unicode local part through UTF-8 without escaping it', () => {
    const claims = { v: 1, id: 'cs_u', email: 'josé@example.com', seats: 1, iat: 1785700000 }
    expect(verifyKey(mintKey(seed, claims), PUBLIC_KEY)).toEqual(claims)
  })
})

describe('tamper resistance', () => {
  const seed = seedFromEnv(process.env.LICENSE_SIGNING_KEY)
  const claims: LicenseClaims = {
    v: 1,
    id: 'cs_live_seatcheck',
    email: 'buyer@example.com',
    seats: 1,
    iat: 1785700000,
  }
  const key = mintKey(seed, claims)
  const [head, sig] = key.split('.') as [string, string]
  const reseat = (seats: number) =>
    `WRDN-${Buffer.from(encodePayload({ ...claims, seats }), 'utf8').toString('base64url')}.${sig}`

  it('rejects seats escalated 1 -> 3 with the signature untouched', () => {
    expect(verifyKey(reseat(3), PUBLIC_KEY)).toBeNull()
  })

  it('rejects every seat count other than the one that was signed', () => {
    for (const seats of [2, 3, 10, 999]) expect(verifyKey(reseat(seats), PUBLIC_KEY)).toBeNull()
    expect(verifyKey(reseat(1), PUBLIC_KEY)).toEqual(claims)
  })

  it('rejects a swapped email at the same signature', () => {
    const swapped = `WRDN-${Buffer.from(
      encodePayload({ ...claims, email: 'attacker@example.com' }),
      'utf8',
    ).toString('base64url')}.${sig}`
    expect(verifyKey(swapped, PUBLIC_KEY)).toBeNull()
  })

  it('rejects a signature from a different signing identity', () => {
    const otherSeed = Buffer.alloc(32, 7)
    expect(verifyKey(mintKey(otherSeed, claims), PUBLIC_KEY)).toBeNull()
  })

  it('rejects every single-bit flip in the signature', () => {
    const bytes = Buffer.from(sig, 'base64url')
    for (const i of [0, 17, 31, 63]) {
      const flipped = Buffer.from(bytes)
      flipped.writeUInt8(flipped.readUInt8(i) ^ 0x01, i)
      expect(verifyKey(`${head}.${flipped.toString('base64url')}`, PUBLIC_KEY)).toBeNull()
    }
  })

  it('rejects malformed shapes', () => {
    expect(verifyKey('', PUBLIC_KEY)).toBeNull()
    expect(verifyKey(key.slice('WRDN-'.length), PUBLIC_KEY)).toBeNull()
    expect(verifyKey(`WRDN-${head}`, PUBLIC_KEY)).toBeNull()
    expect(verifyKey(`${key}.extra`, PUBLIC_KEY)).toBeNull()
    expect(verifyKey(`WRDN-!!!.${sig}`, PUBLIC_KEY)).toBeNull()
    expect(verifyKey(key.replace('WRDN-', 'wrdn-'), PUBLIC_KEY)).toBeNull()
  })

  it('rejects a truncated signature that would otherwise pass a length-blind check', () => {
    const short = Buffer.from(sig, 'base64url').subarray(0, 32)
    expect(verifyKey(`${head}.${short.toString('base64url')}`, PUBLIC_KEY)).toBeNull()
  })

  it('rejects non-canonical base64 that decodes to a valid payload', () => {
    const padded = `${head}=.${sig}`
    expect(verifyKey(padded, PUBLIC_KEY)).toBeNull()
  })

  it('rejects v != 1 even when the signature is ours', () => {
    const v2 = mintKey(seed, { ...claims, v: 2 })
    expect(verifyKey(v2, PUBLIC_KEY)).toBeNull()
  })
})

describe('signing key handling', () => {
  it('refuses a seed that is not 32 bytes', () => {
    expect(() => seedFromEnv(Buffer.alloc(16).toString('base64url'))).toThrow(/expected 32/)
    expect(() => seedFromEnv(undefined)).toThrow(/not set/)
  })

  it('never puts the seed in the error message', () => {
    const bad = Buffer.alloc(31, 9).toString('base64url')
    try {
      seedFromEnv(bad)
      expect.unreachable('should have thrown')
    } catch (err) {
      expect((err as Error).message).not.toContain(bad)
    }
  })
})
