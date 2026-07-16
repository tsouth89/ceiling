import { useState } from "react";
import { Dropdown } from "../../../../components/Dropdown";
import type { RegionOption } from "../../../../types/bridge";
import type { LocaleKey } from "../../../../i18n/keys";
import { setProviderRegion } from "../../../../lib/tauri";

interface Props {
  providerId: string;
  currentValue: string | null;
  options: RegionOption[];
  t: (key: LocaleKey) => string;
  onChanged: () => void;
}

/**
 * API-region dropdown for Alibaba / Z.ai / MiniMax.
 *
 * Port of the region ComboBox rows in
 * `rust/src/native_ui/preferences.rs::render_provider_detail_panel`.
 * Hidden when the provider has no region options.
 */
export function RegionSection({
  providerId,
  currentValue,
  options,
  t,
  onChanged,
}: Props) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (options.length === 0) return null;

  const selected = currentValue ?? options[0]?.value ?? "";

  const handleChange = async (value: string) => {
    if (value === selected || busy) return;
    setBusy(true);
    setError(null);
    try {
      await setProviderRegion(providerId, value);
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="provider-detail-section provider-detail-region">
      <h4>{t("ProviderRegion")}</h4>
      <Dropdown
        className="dropdown--provider-detail"
        value={selected}
        options={options}
        onChange={(next) => void handleChange(next)}
        disabled={busy}
        ariaLabel={t("ProviderRegion")}
      />
      {error && <p className="provider-detail-error">{error}</p>}
    </section>
  );
}
