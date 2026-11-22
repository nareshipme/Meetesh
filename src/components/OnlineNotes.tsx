import { useEffect, useState } from "react";
import type { OnlineSettings } from "../api";
import { useOnline } from "../hooks/useOnline";

const BLANK: OnlineSettings = { enabled: false, auto: false, preset: "openrouter", base_url: "", auth_header: "", model: "", };

/**
 * Optional: write meeting notes and titles with an online model instead of the one on this device.
 * This is the only place where meeting text can leave the computer, so it is off by default, says so
 * plainly, and needs an explicit tick before it can be turned on.
 */
export default function OnlineNotes() {
  const o = useOnline();
  const [form, setForm] = useState<OnlineSettings>(BLANK);
  const [key, setKey] = useState("");
  const [consent, setConsent] = useState(false);
  const [models, setModels] = useState<string[] | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // Start from what is saved.
  useEffect(() => {
    if (o.status) {
      const { enabled, auto, preset, base_url, auth_header, model } = o.status;
      setForm({ enabled, auto, preset: preset || "openrouter", base_url, auth_header, model });
      setConsent(enabled); // it could only have been turned on with the tick
    }
  }, [o.status?.enabled, o.status?.auto, o.status?.preset, o.status?.base_url, o.status?.auth_header, o.status?.model, o.status === null]);

  if (!o.status) return null;
  const st = o.status;
  const preset = st.presets.find((p) => p.id === form.preset) ?? st.presets[0];
  const custom = preset.id === "custom";
  const set = (patch: Partial<OnlineSettings>) => {
    setForm((f) => ({ ...f, ...patch }));
    setMsg(null);
  };
  const pickPreset = (id: string) => {
    const p = st.presets.find((x) => x.id === id)!;
    set({ preset: id, model: p.model, base_url: id === "custom" ? form.base_url : "", auth_header: "" });
    setModels(null);
  };

  async function withBusy(f: () => Promise<void>) {
    setBusy(true);
    try {
      await f();
    } finally {
      setBusy(false);
    }
  }

  const saveKey = () =>
    withBusy(async () => {
      if (await o.setKey(key)) {
        setKey("");
        setMsg("Key saved in the system keychain.");
      }
    });
  const load = () =>
    withBusy(async () => {
      const m = await o.models(form);
      if (m) {
        setModels(m);
        setMsg(m.length ? `${m.length} model${m.length === 1 ? "" : "s"} found${preset.free_only ? " (free ones only)" : ""}.` : "The provider listed no models.");
      }
    });
  const test = () =>
    withBusy(async () => {
      if (await o.test(form)) setMsg("Connected: the provider answered. (The test sent no meeting text.)");
    });
  const save = () =>
    withBusy(async () => {
      if (await o.save(form, consent)) setMsg(form.enabled ? "Saved. Notes will now be written online." : "Saved. Notes are written on this device.");
    });

  return (
    <div className="slot online" aria-label="Online meeting notes">
      <div className="slot-head">
        <h3>Meeting notes written online (optional)</h3>
        <span className={`pill ${st.enabled ? "pill-warn" : ""}`}>{st.enabled ? "On: text is sent online" : "Off: nothing leaves this device"}</span>
      </div>
      <p className="muted small">
        Use a bigger online model for meeting notes and titles. Hints and live points always stay on this device. It runs when you click
        &quot;Write meeting notes&quot; (or after every meeting, if you choose that below).
      </p>
      <p className="warn small">
        <strong>When this is on, the transcript text of the meeting is sent to the provider you choose</strong> (not the audio). Their terms decide what
        they keep. Free models in particular may store or learn from what you send. Do not use it for meetings you must keep private.
      </p>

      <label className="field">
        <span>Provider</span>
        <select aria-label="Online provider" value={form.preset} onChange={(e) => pickPreset(e.target.value)}>
          {st.presets.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
      </label>
      <p className="muted small">{preset.note}</p>

      {custom && (
        <>
          <label className="field">
            <span>Web address</span>
            <input aria-label="Provider web address" type="text" placeholder="https://api.example.com/v1" value={form.base_url} onChange={(e) => set({ base_url: e.target.value })} />
          </label>
          <label className="field">
            <span>Key header (optional)</span>
            <input aria-label="Key header name" type="text" placeholder="Authorization: Bearer (default)" value={form.auth_header} onChange={(e) => set({ auth_header: e.target.value })} />
          </label>
        </>
      )}

      <label className="field">
        <span>API key {st.has_key && <em className="muted">(saved)</em>}</span>
        <span className="row">
          <input aria-label="API key" type="password" autoComplete="off" placeholder={st.has_key ? "Saved in the keychain" : "Paste your key"} value={key} onChange={(e) => setKey(e.target.value)} />
          <button className="btn btn-sm" onClick={saveKey} disabled={busy || !key.trim()}>
            Save key
          </button>
          {st.has_key && (
            <button className="btn btn-sm" onClick={() => withBusy(async () => void (await o.clearKey()))} disabled={busy}>
              Remove key
            </button>
          )}
        </span>
      </label>
      <p className="muted small">The key is kept in this computer&apos;s keychain, never in a file or in the app&apos;s database.</p>
      {st.keychain_error && <p className="error-text small">{st.keychain_error}</p>}

      <label className="field">
        <span>Model</span>
        <span className="row">
          {models && models.length > 0 ? (
            <select aria-label="Online model" value={form.model} onChange={(e) => set({ model: e.target.value })}>
              {!models.includes(form.model) && <option value={form.model}>{form.model || "Choose a model"}</option>}
              {models.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </select>
          ) : (
            <input aria-label="Online model" type="text" placeholder="model name" value={form.model} onChange={(e) => set({ model: e.target.value })} />
          )}
          <button className="btn btn-sm" onClick={load} disabled={busy || !st.has_key} title="Ask the provider which models it offers">
            List models
          </button>
        </span>
      </label>

      <label className="check">
        <input type="checkbox" checked={form.auto} onChange={(e) => set({ auto: e.target.checked })} />
        Write notes online automatically after every meeting (otherwise only when I click the button)
      </label>
      <label className="check">
        <input type="checkbox" checked={consent} onChange={(e) => setConsent(e.target.checked)} />I understand that the transcript text of my meetings will be sent to this provider.
      </label>
      <label className="check">
        <input type="checkbox" checked={form.enabled} onChange={(e) => set({ enabled: e.target.checked })} disabled={!consent} />
        Use the online model for meeting notes and titles
      </label>

      <div className="row">
        <button className="btn btn-sm" onClick={test} disabled={busy || !st.has_key || !form.model.trim()}>
          Test connection
        </button>
        <button className="btn btn-sm btn-primary" onClick={save} disabled={busy}>
          Save
        </button>
      </div>
      {msg && <p className="small" role="status">{msg}</p>}
      {o.error && <p className="error-text small" role="alert">{o.error}</p>}
    </div>
  );
}
