// GET /api/download?token=  -> streams the macOS build.
//
// The repo is private, so release assets are not publicly fetchable and this is
// the only way in (LAUNCH-SPEC section 1). The asset is streamed THROUGH this
// function: the client is never redirected, so neither the download token nor
// the GitHub PAT ever appears in a URL the browser follows, a Referer header, or
// a CDN log.

import { fail, guard, json, methodNotAllowed } from './_lib/http.js'
import { verifyToken } from './_lib/tokens.js'

const DEFAULT_REPO = 'karimbabasf/WARDEN'
const GITHUB_API = 'https://api.github.com'
const ASSET_SUFFIXES = ['.dmg', '.zip', '.tar.gz']

type Fetch = typeof fetch

interface ReleaseAsset {
  url: string
  name: string
  size: number
  content_type: string
}

const pickAsset = (assets: ReleaseAsset[]): ReleaseAsset | undefined => {
  for (const suffix of ASSET_SUFFIXES) {
    const match = assets.find((asset) => asset.name.toLowerCase().endsWith(suffix))
    if (match) return match
  }
  return assets[0]
}

export const handleDownload = async (request: Request, fetchImpl: Fetch = fetch): Promise<Response> => {
  const token = new URL(request.url).searchParams.get('token')
  const secret = process.env.DOWNLOAD_TOKEN_SECRET
  if (!secret) return fail(500, 'not_configured')

  // Same answer for a forged MAC, a mangled token and an expired one. The
  // caller learns only that this token does not work now.
  const claims = verifyToken(token, secret)
  if (!claims) return fail(403, 'invalid_token')

  const pat = process.env.GITHUB_RELEASE_TOKEN
  if (!pat) return fail(500, 'not_configured')

  const repo = process.env.GITHUB_RELEASE_REPO?.trim() || DEFAULT_REPO
  const tag = process.env.GITHUB_RELEASE_TAG?.trim()
  const releaseUrl = tag
    ? `${GITHUB_API}/repos/${repo}/releases/tags/${encodeURIComponent(tag)}`
    : `${GITHUB_API}/repos/${repo}/releases/latest`

  const releaseResponse = await fetchImpl(releaseUrl, {
    headers: {
      authorization: `Bearer ${pat}`,
      accept: 'application/vnd.github+json',
      'x-github-api-version': '2022-11-28',
      'user-agent': 'warden-site',
    },
  })
  if (!releaseResponse.ok) {
    console.error(`[download] github release lookup failed: ${releaseResponse.status}`)
    return fail(502, 'release_unavailable')
  }

  const release = (await releaseResponse.json()) as { assets?: ReleaseAsset[] }
  const asset = pickAsset(release.assets ?? [])
  if (!asset) return fail(404, 'no_asset')

  // redirect: 'manual' on purpose. GitHub answers an asset request with a 302 to
  // a pre-signed object-store URL, and following it automatically risks
  // forwarding the Authorization header to a third-party host. The redirect is
  // followed here, explicitly, with no credentials attached.
  const assetResponse = await fetchImpl(asset.url, {
    redirect: 'manual',
    headers: {
      authorization: `Bearer ${pat}`,
      accept: 'application/octet-stream',
      'user-agent': 'warden-site',
    },
  })

  let body = assetResponse
  if (assetResponse.status >= 300 && assetResponse.status < 400) {
    const location = assetResponse.headers.get('location')
    if (!location) return fail(502, 'release_unavailable')
    body = await fetchImpl(location, { headers: { 'user-agent': 'warden-site' } })
  }

  if (!body.ok || !body.body) {
    console.error(`[download] asset fetch failed: ${body.status}`)
    return fail(502, 'release_unavailable')
  }

  // The asset name comes from the release, which is ours, but it lands in a
  // response header: anything outside this set (quotes, backslashes, CR/LF)
  // would be header injection if a release were ever named carelessly.
  const filename = asset.name.replace(/[^A-Za-z0-9._-]/g, '_').slice(0, 128) || 'WARDEN.dmg'

  const headers = new Headers({
    'content-type': asset.content_type || 'application/octet-stream',
    'content-disposition': `attachment; filename="${filename}"`,
    'cache-control': 'private, no-store',
    'x-content-type-options': 'nosniff',
  })
  const length = body.headers.get('content-length') ?? (asset.size ? String(asset.size) : null)
  if (length) headers.set('content-length', length)

  return new Response(body.body, { status: 200, headers })
}

export const GET = (request: Request): Promise<Response> =>
  guard('download', () => handleDownload(request))

export const POST = (): Response => methodNotAllowed('GET')

export const HEAD = async (request: Request): Promise<Response> => {
  const secret = process.env.DOWNLOAD_TOKEN_SECRET
  if (!secret) return fail(500, 'not_configured')
  const claims = verifyToken(new URL(request.url).searchParams.get('token'), secret)
  return claims ? json({ ok: true }) : fail(403, 'invalid_token')
}
