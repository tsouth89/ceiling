/**
 * The Ceiling app mark: a capacity level (white) filling under the ceiling
 * line (green). Self-contained dark tile so it reads on any theme, mirroring
 * the app/tray icon. Used in window title bars and the tray flyout header.
 */
export function CeilingMark({
  size = 16,
  className,
  title = "Ceiling",
}: {
  size?: number;
  className?: string;
  title?: string;
}) {
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
      <rect width="32" height="32" rx="8.5" fill="#0f1216" />
      <rect x="7" y="8" width="18" height="2.6" rx="1.3" fill="#a6e35c" />
      <rect x="7" y="13.6" width="18" height="10.4" rx="2.4" fill="#e8ecf1" />
    </svg>
  );
}
