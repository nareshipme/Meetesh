import type { Permission, PermissionStatus } from "../api";
import { usePermissions } from "../hooks/usePermissions";

const STATUS: Record<PermissionStatus, { label: string; tone: "ok" | "warn" | "bad" | "muted" }> = {
  granted: { label: "Allowed", tone: "ok" },
  denied: { label: "Blocked", tone: "bad" },
  not_granted: { label: "Not allowed yet", tone: "warn" },
  off: { label: "Off in Meetesh", tone: "muted" },
  unknown: { label: "Cannot be checked", tone: "muted" },
  not_needed: { label: "No permission needed", tone: "ok" },
  unsupported: { label: "Not available here", tone: "muted" },
};

/**
 * What Meetesh needs, per feature, and what you can do about each. A permission the system owns
 * (microphone, screen and system audio recording) can be asked for here and is taken back in the
 * system's settings; the switches are Meetesh's own and stop it from using something at all.
 */
export default function PermissionsPanel() {
  const p = usePermissions();
  if (!p.list) return p.error ? <p className="error-text small">{p.error}</p> : null;

  return (
    <div className="perms">
      {p.list.map((row) => (
        <Row key={row.id} row={row} p={p} />
      ))}
      {p.error && (
        <p className="error-text small" role="alert">
          {p.error}
        </p>
      )}
      <p className="muted small">
        Meetesh does not ask for the camera, your location, contacts, calendar or notifications. A permission the system owns can only be taken back
        in the system&apos;s own settings; the switches here are Meetesh&apos;s own and stop it from doing the thing at all.
      </p>
    </div>
  );
}

function Row({ row, p }: { row: Permission; p: ReturnType<typeof usePermissions> }) {
  const s = STATUS[row.status];
  return (
    <section className="perm slot" aria-label={row.name} data-status={row.status}>
      <div className="slot-head">
        <h3>
          {row.name} <span className={`chip ${row.need === "required" ? "chip-busy" : ""}`}>{row.need === "required" ? "Required" : "Optional"}</span>
        </h3>
        <span className={`pill pill-${s.tone}`}>{s.label}</span>
      </div>
      <p className="small">
        <strong>Used for:</strong> {row.feature}
      </p>
      <p className="muted small">{row.detail}</p>
      <p className="muted small">
        <strong>Without it:</strong> {row.without}
      </p>
      <div className="row">
        {row.switch !== null && (
          <label className="check">
            <input type="checkbox" checked={row.switch} onChange={(e) => p.setSwitch(row.id, e.target.checked)} />
            Allow Meetesh to {row.id === "network" ? "use the internet" : "record the other participants"}
          </label>
        )}
        {row.can_request && (
          <button className="btn btn-sm btn-primary" onClick={() => p.request(row.id)}>
            Allow…
          </button>
        )}
        {row.opens_settings && (
          <button className="btn btn-sm" onClick={() => p.openSettings(row.id)}>
            Open system settings
          </button>
        )}
        {row.id === "keychain" && row.status === "granted" && (
          <button className="btn btn-sm" onClick={p.removeKey}>
            Remove the stored key
          </button>
        )}
      </div>
    </section>
  );
}
