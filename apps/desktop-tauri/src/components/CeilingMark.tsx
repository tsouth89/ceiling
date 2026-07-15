/**
 * The Ceiling app mark: a capacity level (white) filling under the ceiling
 * line (green). Self-contained dark tile so it reads on any theme, mirroring
 * the app/tray icon. Used in window title bars and the tray flyout header.
 */
export function CeilingMark({
  size = 16,
  className,
  title = "Ceiling",
  appearance = "tile",
}: {
  size?: number;
  className?: string;
  title?: string;
  appearance?: "tile" | "glass";
}) {
  const glass = appearance === "glass";
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 32 32"
      fill="none"
      role="img"
      aria-label={title}
      className={className}
    >
      <rect
        x={glass ? 0.5 : 0}
        y={glass ? 0.5 : 0}
        width={glass ? 31 : 32}
        height={glass ? 31 : 32}
        rx="8.5"
        fill={glass ? "rgba(255, 255, 255, 0.055)" : "#0f1216"}
        stroke={glass ? "rgba(255, 255, 255, 0.16)" : "none"}
      />
      <rect x="7" y="8" width="18" height="2.6" rx="1.3" fill={glass ? "#80e5ec" : "#a6e35c"} />
      <rect x="7" y="13.6" width="18" height="10.4" rx="2.4" fill={glass ? "#f6f9fd" : "#e8ecf1"} />
    </svg>
  );
}
