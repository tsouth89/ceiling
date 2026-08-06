import type { PaceSnapshot } from "../../../../types/bridge";
import type { LocaleKey } from "../../../../i18n/keys";

interface Props {
  pace: PaceSnapshot | null;
  t: (key: LocaleKey) => string;
}

const STAGE_TO_KEY: Record<PaceSnapshot["stage"], LocaleKey> = {
  on_track: "DetailPaceOnTrack",
  slightly_ahead: "DetailPaceSlightlyAhead",
  ahead: "DetailPaceAhead",
  far_ahead: "DetailPaceFarAhead",
  slightly_behind: "DetailPaceSlightlyBehind",
  behind: "DetailPaceBehind",
  far_behind: "DetailPaceFarBehind",
};

/**
 * Pace stage + auxiliary copy. Port of the pace rows in
 * `rust/src/native_ui/preferences.rs::render_provider_detail_panel`.
 */
export function PaceSection({ pace, t }: Props) {
  if (!pace) return null;

  const stageLabel = t(STAGE_TO_KEY[pace.stage]);
  const aux = pace.willLastToReset
    ? t("DetailPaceWillLastToReset")
    : pace.etaSeconds !== null
      ? `${t("DetailPaceRunsOutIn")} ${formatEta(pace.etaSeconds)}`
      : null;

  return (
    <section className="provider-detail-section provider-detail-pace">
      <h4>{t("DetailPaceTitle")}</h4>
      <div className="provider-detail-pace__stage" data-stage={pace.stage}>
        {stageLabel}
      </div>
      {aux && <div className="provider-detail-pace__aux">{aux}</div>}
    </section>
  );
}

export function formatEta(seconds: number): string {
  const mins = Math.max(0, Math.round(seconds / 60));
  if (mins < 60) return `${mins}m`;
  const hrs = Math.round(mins / 60);
  if (hrs < 24) return `${hrs}h`;
  return `${Math.round(hrs / 24)}d`;
}
