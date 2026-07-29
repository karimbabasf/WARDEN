// The producer half of the WARDEN license format. The consumer is
// src-tauri/src/license/mod.rs and the two must agree byte for byte, so this
// file mirrors scripts/gen-license-keypair.mjs exactly rather than reimplementing
// it. docs/license-vectors.json is the referee: both suites read that same file.
//
// Format (see docs/LAUNCH-SPEC.md section 2):
//   payload  {"v":1,"id":"<session>","email":"<lowercased>","seats":<n>,"iat":<unix>}
//   key      WRDN-<base64url(payload)>.<base64url(ed25519 signature)>   no padding

import { createPrivateKey, createPublicKey, sign, verify } from 'node:crypto'

/** Raw 32-byte Ed25519 keys are not a format node's KeyObject takes directly, so
 *  wrap them in the fixed DER prefixes. Both byte strings are constant because
 *  the algorithm and the key length are constant. */
const PKCS8_PREFIX = Buffer.from('302e020100300506032b657004220420', 'hex')
const SPKI_PREFIX = Buffer.from('302a300506032b6570032100', 'hex')

export const KEY_PREFIX = 'WRDN-'
const SEED_BYTES = 32
const SIGNATURE_BYTES = 64

export interface LicenseClaims {
  v: number
  id: string
  email: string
  seats: number
  iat: number
}

const b64u = (buf: Uint8Array): string => Buffer.from(buf).toString('base64url')

/** Reject non-canonical base64url before trusting a decode. Node's decoder is
 *  lenient: it silently drops invalid characters and accepts padding, so two
 *  different key strings can decode to identical bytes. Round-tripping the
 *  decode makes the key string itself canonical, which keeps "the key I emailed
 *  you" and "the key you pasted" the same string. */
const decodeStrict = (segment: string): Buffer | null => {
  if (segment.length === 0) return null
  const buf = Buffer.from(segment, 'base64url')
  return buf.toString('base64url') === segment ? buf : null
}

/** The canonical payload. Key order is part of the format: both sides serialize
 *  by hand rather than via JSON.stringify on an object literal, so that a future
 *  refactor cannot silently reorder and break every key in the wild. */
export const encodePayload = ({ v, id, email, seats, iat }: LicenseClaims): string =>
  `{"v":${v},"id":${JSON.stringify(id)},"email":${JSON.stringify(email)},"seats":${seats},"iat":${iat}}`

const seedToPrivate = (seed: Buffer) =>
  createPrivateKey({ key: Buffer.concat([PKCS8_PREFIX, seed]), format: 'der', type: 'pkcs8' })

const rawToPublic = (raw: Buffer) =>
  createPublicKey({ key: Buffer.concat([SPKI_PREFIX, raw]), format: 'der', type: 'spki' })

/** Decodes LICENSE_SIGNING_KEY. Throws on a malformed seed rather than minting
 *  keys nobody can verify: a bad env var must fail the request, loudly, at the
 *  first purchase, not silently produce garbage licenses. The seed itself is
 *  never included in the message. */
export const seedFromEnv = (value: string | undefined): Buffer => {
  if (!value) throw new Error('LICENSE_SIGNING_KEY is not set')
  const seed = Buffer.from(value.trim(), 'base64url')
  if (seed.length !== SEED_BYTES) {
    throw new Error(`LICENSE_SIGNING_KEY is ${seed.length} bytes, expected ${SEED_BYTES}`)
  }
  return seed
}

export const mintKey = (seed: Buffer, claims: LicenseClaims): string => {
  const payload = Buffer.from(encodePayload(claims), 'utf8')
  const signature = sign(null, payload, seedToPrivate(seed))
  return `${KEY_PREFIX}${b64u(payload)}.${b64u(signature)}`
}

/** The public half, derived from the seed. Used to self-check a freshly minted
 *  key before it is handed to a buyer. */
export const publicKeyFromSeed = (seed: Buffer): Buffer =>
  Buffer.from(
    createPublicKey(seedToPrivate(seed)).export({ format: 'der', type: 'spki' }),
  ).subarray(SPKI_PREFIX.length)

/**
 * Offline verification, mirroring the Rust gate: decode, check the signature
 * against the public key, then check v == 1. The shape checks below are a strict
 * superset of that (they reject payloads no signer of ours would ever produce),
 * which is safe in one direction only: anything this accepts, Rust accepts.
 *
 * Returns the claims, or null. Never throws, and never reports WHICH check
 * failed to a caller that might relay it: a verifier that explains itself is a
 * forgery oracle.
 */
export const verifyKey = (key: string, publicKeyRaw: Buffer): LicenseClaims | null => {
  if (typeof key !== 'string' || !key.startsWith(KEY_PREFIX)) return null

  const parts = key.slice(KEY_PREFIX.length).split('.')
  if (parts.length !== 2) return null
  const [payloadSeg, signatureSeg] = parts as [string, string]

  const payload = decodeStrict(payloadSeg)
  const signature = decodeStrict(signatureSeg)
  if (!payload || !signature || signature.length !== SIGNATURE_BYTES) return null

  let publicKey
  try {
    publicKey = rawToPublic(publicKeyRaw)
  } catch {
    return null
  }
  if (!verify(null, payload, publicKey, signature)) return null

  let claims: unknown
  try {
    claims = JSON.parse(payload.toString('utf8'))
  } catch {
    return null
  }
  if (!isClaims(claims)) return null
  if (claims.v !== 1) return null
  return claims
}

const isClaims = (value: unknown): value is LicenseClaims => {
  if (typeof value !== 'object' || value === null) return false
  const c = value as Record<string, unknown>
  return (
    Number.isInteger(c.v) &&
    typeof c.id === 'string' &&
    c.id.length > 0 &&
    typeof c.email === 'string' &&
    Number.isInteger(c.seats) &&
    (c.seats as number) >= 1 &&
    Number.isInteger(c.iat)
  )
}
