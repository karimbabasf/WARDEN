// AccessTab.tsx: the grant management table (ShareMenu's ACCESS tab). Purely
// presentational: ShareMenu owns fetching, `observe:grants` refresh, and the revoke call.

import { grantStateMeta, type GrantRow } from './observeTypes';
import { isoRelative } from './observeFormat';

export function AccessTab({
  grants,
  error,
  revokingId,
  onRevoke,
}: {
  grants: GrantRow[];
  error: string | null;
  revokingId: string | null;
  onRevoke: (grantId: string) => void;
}) {
  return (
    <div className="wd-observe-access" data-tab="access">
      {error ? (
        <p className="wd-observe-error" role="alert">
          {error}
        </p>
      ) : null}
      {grants.length === 0 ? (
        <p className="wd-observe-empty-inline">No grants minted yet.</p>
      ) : (
        <table className="wd-observe-grants">
          <thead>
            <tr>
              <th scope="col">Friend</th>
              <th scope="col">State</th>
              <th scope="col">Created</th>
              <th scope="col">Expires</th>
              <th scope="col">Redeemed by</th>
              <th scope="col" aria-label="Connected" />
              <th scope="col" aria-label="Actions" />
            </tr>
          </thead>
          <tbody>
            {grants.map((g) => {
              const meta = grantStateMeta(g.state);
              const canRevoke = g.state === 'pending' || g.state === 'redeemed';
              return (
                <tr key={g.grantId} data-grant-row={g.grantId} data-grant-state={g.state}>
                  <td>{g.label}</td>
                  <td>
                    <span aria-hidden>{meta.glyph}</span> {meta.label}
                  </td>
                  <td>{isoRelative(g.createdAt) || 'n/a'}</td>
                  <td>{isoRelative(g.expiresAt) || 'n/a'}</td>
                  <td className="wd-observe-mono">{g.redeemedFingerprint ?? 'n/a'}</td>
                  <td>
                    <span
                      className={`wd-observe-dot${g.connected ? ' is-connected' : ''}`}
                      role="img"
                      aria-label={g.connected ? 'connected' : 'not connected'}
                      title={g.connected ? 'Connected' : 'Not connected'}
                    />
                  </td>
                  <td>
                    {canRevoke ? (
                      <button
                        type="button"
                        className="wd-observe-btn wd-observe-btn-ghost"
                        disabled={revokingId === g.grantId}
                        onClick={() => onRevoke(g.grantId)}
                      >
                        Revoke
                      </button>
                    ) : null}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </div>
  );
}

export default AccessTab;
